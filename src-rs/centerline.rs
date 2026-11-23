//! Convert a binary ink mask to non-retracing pixel-centre paths.
use crate::{Artwork, MAX_POINTS, MAX_STROKES, Point, Stroke};

// North clockwise. A one-cell white border permits unchecked coordinate arithmetic,
// but every array access still uses Rust's checked indexing.
fn neighbors(p: usize, stride: usize) -> [usize; 8] {
    [
        p - stride,
        p - stride + 1,
        p + 1,
        p + stride + 1,
        p + stride,
        p + stride - 1,
        p - 1,
        p - stride - 1,
    ]
}

pub(crate) fn trace(
    ink: &[u8],
    width: usize,
    height: usize,
    step: f64,
    origin: Point,
    layer: &str,
) -> Result<Artwork, String> {
    debug_assert_eq!(ink.len(), width * height);
    let stride = width + 2;
    let mut mask = vec![0; stride * (height + 2)];
    let mut scheduled = vec![0u8; mask.len()];
    for (y, row) in ink.chunks_exact(width).enumerate() {
        let start = (y + 1) * stride + 1;
        mask[start..start + width].copy_from_slice(row);
    }
    // Zhang–Suen's two simultaneous deletion passes. Only initial boundary
    // cells and cells neighbouring a deletion need to be rechecked. A failed
    // candidate's rule cannot change without a change in its eight neighbours.
    let mut pending = [Vec::new(), Vec::new()];
    for y in 1..=height {
        for x in 1..=width {
            let p = y * stride + x;
            if mask[p] != 0 && neighbors(p, stride).iter().any(|&n| mask[n] == 0) {
                scheduled[p] = 3;
                pending[0].push(p);
                pending[1].push(p);
            }
        }
    }
    let mut remove = Vec::new();
    let mut budget = 100_000_000usize;
    while !pending[0].is_empty() || !pending[1].is_empty() {
        for phase in 0..2 {
            budget = budget.checked_sub(pending[phase].len()).ok_or(
                "Centerline trace is too complex. Reduce resolution or clean up the input.",
            )?;
            for p in pending[phase].drain(..) {
                scheduled[p] &= !(1 << phase);
                if mask[p] == 0 {
                    continue;
                }
                let n = neighbors(p, stride).map(|n| mask[n]);
                let count = n.into_iter().sum::<u8>();
                let transitions = (0..8).filter(|&i| n[i] == 0 && n[(i + 1) % 8] == 1).count();
                let keep = if phase == 0 {
                    n[0] * n[2] * n[4] != 0 || n[2] * n[4] * n[6] != 0
                } else {
                    n[0] * n[2] * n[6] != 0 || n[0] * n[4] * n[6] != 0
                };
                if (2..=6).contains(&count) && transitions == 1 && !keep {
                    remove.push(p);
                }
            }
            for &p in &remove {
                mask[p] = 0;
            }
            for p in remove.drain(..) {
                for n in neighbors(p, stride) {
                    if mask[n] == 0 {
                        continue;
                    }
                    for (side, queue) in pending.iter_mut().enumerate() {
                        if scheduled[n] & (1 << side) == 0 {
                            scheduled[n] |= 1 << side;
                            queue.push(n);
                        }
                    }
                }
            }
        }
    }

    // Form the undirected pixel graph. Diagonal edges whose endpoints
    // already communicate through an adjacent orthogonal pixel are redundant.
    let mut graph = scheduled;
    for y in 1..=height {
        for x in 1..=width {
            let p = y * stride + x;
            if mask[p] == 0 {
                continue;
            }
            let n = neighbors(p, stride).map(|n| mask[n]);
            for i in 0..8 {
                if n[i] == 1 && (i % 2 == 0 || n[(i + 7) % 8] + n[(i + 1) % 8] == 0) {
                    graph[p] |= 1 << i;
                }
            }
        }
    }
    let mut remaining = graph.clone();
    let mut artwork = Artwork::default();
    let mut point_count = 0;
    let map = |p: usize| {
        Point::new(
            origin.x + (p % stride) as f64 * step - 0.5 * step,
            origin.y + (height as f64 + 0.5 - (p / stride) as f64) * step,
        )
    };
    for leftover_cycles in [false, true] {
        for start in 0..graph.len() {
            if (graph[start].count_ones() == 2) != leftover_cycles {
                continue;
            }
            while remaining[start] != 0 {
                let mut points = vec![map(start)];
                let mut p = start;
                let mut prior = None;
                let closed = loop {
                    let direction = remaining[p].trailing_zeros() as usize;
                    let next = neighbors(p, stride)[direction];
                    remaining[p] &= !(1 << direction);
                    remaining[next] &= !(1 << ((direction + 4) % 8));
                    if next == start {
                        if prior == Some(direction) {
                            points.pop();
                        }
                        break true;
                    }
                    if prior == Some(direction) {
                        *points.last_mut().expect("direction implies last point") = map(next);
                    } else {
                        points.push(map(next));
                    }
                    if points.len() > MAX_POINTS {
                        return Err("Centerline trace exceeds the point limit.".into());
                    }
                    if graph[next].count_ones() != 2 || remaining[next] == 0 {
                        break false;
                    }
                    prior = Some(direction);
                    p = next;
                };
                point_count += points.len();
                if point_count > MAX_POINTS || artwork.strokes.len() == MAX_STROKES {
                    return Err("Centerline trace exceeds the geometry limit.".into());
                }
                artwork.strokes.push(Stroke {
                    group: artwork.strokes.len() as u32,
                    layer: layer.into(),
                    points,
                    closed,
                    filled: false,
                });
            }
        }
    }
    Ok(artwork)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn paths(rows: &[&str]) -> Vec<Stroke> {
        let mask: Vec<_> = rows
            .iter()
            .flat_map(|r| r.bytes().map(|c| u8::from(c == b'#')))
            .collect();
        trace(
            &mask,
            rows[0].len(),
            rows.len(),
            1.,
            Point::default(),
            "ink",
        )
        .unwrap()
        .strokes
    }

    #[test]
    fn thin_and_thick_bars_are_one_unfilled_open_path() {
        for mask in [&["#######"][..], &["#######", "#######", "#######"][..]] {
            let strokes = paths(mask);
            assert_eq!(strokes.len(), 1);
            let s = &strokes[0];
            assert!(!s.closed && !s.filled);
            assert_eq!(s.points.len(), 2);
            assert_eq!(s.points[0].y, s.points[1].y);
            assert!(s.points[0].distance(s.points[1]) >= 3.);
        }
    }

    #[test]
    fn elbow_is_open_and_ring_closes_once() {
        let elbow = paths(&["###", "..#", "..#"]);
        assert_eq!(elbow.len(), 1);
        assert!(!elbow[0].closed);
        assert_eq!(elbow[0].points.len(), 3);
        let ring = paths(&["#####", "#...#", "#...#", "#####"]);
        assert_eq!(ring.len(), 1);
        assert!(ring[0].closed && !ring[0].filled);
        assert_eq!(ring[0].points.len(), 4);
    }

    #[test]
    fn junctions_use_each_undirected_segment_once() {
        for (mask, count) in [
            (&["#####", "..#..", "..#.."][..], 3),
            (&["#...#", ".#.#.", "..#..", ".#.#.", "#...#"][..], 4),
        ] {
            let strokes = paths(mask);
            let mut seen = BTreeSet::new();
            assert_eq!(strokes.len(), count);
            for s in strokes {
                assert!(!s.closed);
                for edge in s.points.windows(2) {
                    let coord = |p: Point| ((p.x * 2.) as i32, (p.y * 2.) as i32);
                    let (a, b) = (coord(edge[0]), coord(edge[1]));
                    assert_ne!(a, b);
                    assert!(seen.insert((a.min(b), a.max(b))));
                }
            }
        }
    }
}
