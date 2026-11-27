//! Physical-width filtering: disk opening of raster ink, and pen-footprint clipping.
use crate::{Point, Stroke};
const INF: f32 = 1e20;

// Exact squared Euclidean distance transform of samples on a regular grid.
fn edt(field: &mut [f32], w: usize, h: usize) {
    let n = w.max(h);
    let mut line = vec![INF; n];
    let mut out = vec![INF; n];
    let mut sites = vec![0usize; n];
    let mut cuts = vec![0f64; n + 1];
    for vertical in [false, true] {
        let (count, len) = if vertical { (w, h) } else { (h, w) };
        for row in 0..count {
            for i in 0..len {
                line[i] = field[if vertical { i * w + row } else { row * w + i }];
            }
            let mut size = 0;
            for q in 0..len {
                if line[q] >= INF {
                    continue;
                }
                let mut crossing = f64::NEG_INFINITY;
                while size > 0 {
                    let p = sites[size - 1];
                    crossing = ((f64::from(line[q]) + (q * q) as f64)
                        - (f64::from(line[p]) + (p * p) as f64))
                        / (2. * (q - p) as f64);
                    if crossing > cuts[size - 1] {
                        break;
                    }
                    size -= 1;
                }
                sites[size] = q;
                cuts[size] = if size == 0 {
                    f64::NEG_INFINITY
                } else {
                    crossing
                };
                size += 1;
                cuts[size] = f64::INFINITY;
            }
            if size == 0 {
                out[..len].fill(INF);
            } else {
                let mut k = 0;
                for (q, value) in out[..len].iter_mut().enumerate() {
                    while cuts[k + 1] < q as f64 {
                        k += 1;
                    }
                    let delta = q as f64 - sites[k] as f64;
                    *value = (delta * delta + f64::from(line[sites[k]])) as f32;
                }
            }
            for i in 0..len {
                field[if vertical { i * w + row } else { row * w + i }] = out[i];
            }
        }
    }
}

/// Remove parts of raster ink which cannot contain a disk of the specified width.
/// Boundary samples include pixel corners and edge midpoints. Half-pixel seeds
/// preserve both odd and even pixel-wide strokes at the exact width threshold.
/// Reconstruction is intersected with the original ink: holes never get painted.
pub(crate) fn open_mask(
    mask: &mut [u8],
    w: usize,
    h: usize,
    width_px: f64,
) -> Result<usize, String> {
    if !width_px.is_finite() || width_px < 0. {
        return Err("Invalid pen width".into());
    }
    if width_px <= 1. {
        return Ok(0);
    }
    let (gw, gh) = (2 * w + 1, 2 * h + 1);
    if gw.saturating_mul(gh) > 33_000_000 || mask.len() != w * h {
        return Err("Pen-width filter image exceeds its limit".into());
    }
    let mut field = vec![INF; gw * gh];
    for y in 0..h {
        for x in 0..w {
            if mask[y * w + x] == 0 {
                continue;
            }
            for (ex, ey, dx, dy, exposed) in [
                (2 * x, 2 * y, 1, 0, y == 0 || mask[(y - 1) * w + x] == 0),
                (
                    2 * x,
                    2 * y + 2,
                    1,
                    0,
                    y + 1 == h || mask[(y + 1) * w + x] == 0,
                ),
                (2 * x, 2 * y, 0, 1, x == 0 || mask[y * w + x - 1] == 0),
                (
                    2 * x + 2,
                    2 * y,
                    0,
                    1,
                    x + 1 == w || mask[y * w + x + 1] == 0,
                ),
            ] {
                if exposed {
                    for k in 0..=2 {
                        field[(ey + k * dy) * gw + ex + k * dx] = 0.;
                    }
                }
            }
        }
    }
    edt(&mut field, gw, gh);
    // Grid spacing is half a source pixel, so disk radius in grid units = width_px.
    let radius2 = width_px * width_px;
    for y in 0..gh {
        for x in 0..gw {
            let inside = x / 2 < w && y / 2 < h && mask[(y / 2) * w + x / 2] != 0;
            let i = y * gw + x;
            field[i] = if inside && f64::from(field[i]) + 1e-6 >= radius2 {
                0.
            } else {
                INF
            };
        }
    }
    edt(&mut field, gw, gh);
    let mut removed = 0;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if mask[i] != 0 && f64::from(field[(2 * y + 1) * gw + 2 * x + 1]) > radius2 + 1e-6 {
                mask[i] = 0;
                removed += 1;
            }
        }
    }
    Ok(removed)
}

fn dot(a: Point, b: Point) -> f64 {
    a.x * b.x + a.y * b.y
}
fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}
fn interval(v: f64, d: f64, lo: f64, hi: f64) -> Option<(f64, f64)> {
    if d.abs() < 1e-15 {
        return (v >= lo && v <= hi).then_some((0., 1.));
    }
    let (a, b) = ((lo - v) / d, (hi - v) / d);
    let range = (a.min(b).max(0.), a.max(b).min(1.));
    (range.0 < range.1).then_some(range)
}
fn circle(p: Point, d: Point, c: Point, r: f64) -> Option<(f64, f64)> {
    let q = sub(p, c);
    let a = dot(d, d);
    let b = dot(q, d);
    let disc = b * b - a * (dot(q, q) - r * r);
    if a <= 1e-20 || disc <= 0. {
        return None;
    }
    let root = disc.sqrt();
    let range = ((-b - root) / a, (-b + root) / a);
    let range = (range.0.max(0.), range.1.min(1.));
    (range.0 < range.1).then_some(range)
}

/// Clip a hatch centerline against the radius-neighborhood of every boundary.
/// Input segments must already be inside the region (including hole subtraction).
/// The remaining centerlines keep the entire circular pen footprint in the ink.
pub(crate) fn clip_hatch(
    stroke: &Stroke,
    rings: &[&Stroke],
    width: f64,
    budget: &mut usize,
) -> Result<Vec<Stroke>, String> {
    if width <= 0. {
        return Ok(vec![stroke.clone()]);
    }
    let (p, end) = (stroke.points[0], stroke.points[1]);
    let d = sub(end, p);
    let r = width * 0.5;
    let mut blocked = Vec::new();
    for ring in rings {
        for edge in ring.points.windows(2) {
            *budget = budget
                .checked_sub(1)
                .ok_or("Pen-width clipping too complex; simplify artwork or reduce fill density")?;
            let (a, b) = (edge[0], edge[1]);
            let v = sub(b, a);
            let len = dot(v, v).sqrt();
            for c in [a, b] {
                if let Some(i) = circle(p, d, c, r) {
                    blocked.push(i);
                }
            }
            if len <= 1e-12 {
                continue;
            }
            let u = Point::new(v.x / len, v.y / len);
            let n = Point::new(-u.y, u.x);
            let q = sub(p, a);
            if let (Some(along), Some(across)) = (
                interval(dot(q, u), dot(d, u), 0., len),
                interval(dot(q, n), dot(d, n), -r, r),
            ) {
                let i = (along.0.max(across.0), along.1.min(across.1));
                if i.0 < i.1 {
                    blocked.push(i);
                }
            }
        }
    }
    blocked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut cursor = 0.;
    let point = |t: f64| Point::new(p.x + d.x * t, p.y + d.y * t);
    for (a, b) in blocked.into_iter().chain(std::iter::once((1., 1.))) {
        if a > cursor + 1e-10 {
            out.push(Stroke {
                points: vec![point(cursor), point(a)],
                ..stroke.clone()
            });
        }
        cursor = cursor.max(b);
        if cursor >= 1. {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_removes_long_thin_ink_keeps_exact_width_and_never_fills_holes() {
        let (w, h) = (70, 40);
        let mut mask = vec![0; w * h];
        // A long, one-pixel line; a shorter four-pixel line; an eight-pixel box with a hole.
        for x in 2..65 {
            mask[3 * w + x] = 1;
        }
        for y in 10..14 {
            for x in 5..35 {
                mask[y * w + x] = 1;
            }
        }
        for y in 20..38 {
            for x in 40..66 {
                mask[y * w + x] = 1;
            }
        }
        for y in 25..32 {
            for x in 48..57 {
                mask[y * w + x] = 0;
            }
        }
        let original = mask.clone();
        assert!(open_mask(&mut mask, w, h, 4.).unwrap() > 0);
        assert!((2..65).all(|x| mask[3 * w + x] == 0));
        assert!((10..14).all(|y| mask[y * w + 20] == 1));
        assert!(mask.iter().zip(&original).all(|(a, b)| a <= b));
        assert_eq!(mask[28 * w + 52], 0);
        // Odd-width thresholds and narrow branches attached to a thick region.
        let mut odd = vec![0; w * h];
        for y in 10..15 {
            for x in 5..30 {
                odd[y * w + x] = 1;
            }
        }
        for x in 30..65 {
            odd[12 * w + x] = 1;
        }
        open_mask(&mut odd, w, h, 5.).unwrap();
        assert!((10..15).all(|y| odd[y * w + 15] == 1));
        assert_eq!(odd[12 * w + 60], 0);
    }
    #[test]
    fn diagonal_hatch_footprint_clears_holes_and_drops_long_narrow_region() {
        let rect = |x0, y0, x1, y1| Stroke {
            points: vec![
                Point::new(x0, y0),
                Point::new(x1, y0),
                Point::new(x1, y1),
                Point::new(x0, y1),
                Point::new(x0, y0),
            ],
            layer: "ink".into(),
            group: 0,
            closed: true,
            filled: true,
        };
        let thin = rect(0., 0., 40., 0.2);
        let line = Stroke {
            points: vec![Point::new(0., 0.1), Point::new(40., 0.1)],
            closed: false,
            filled: false,
            ..thin.clone()
        };
        assert!(
            clip_hatch(&line, &[&thin], 0.3, &mut 1000)
                .unwrap()
                .is_empty()
        );
        let outer = rect(0., 0., 20., 20.);
        let hole = rect(7., 7., 13., 13.);
        // Hatch generation first subtracts the hole; clearance clips those segments.
        let mut parts = Vec::new();
        for (a, b) in [(0., 7.), (13., 20.)] {
            let line = Stroke {
                points: vec![Point::new(a, a), Point::new(b, b)],
                closed: false,
                filled: false,
                ..outer.clone()
            };
            parts.extend(clip_hatch(&line, &[&outer, &hole], 2., &mut 1000).unwrap());
        }
        assert_eq!(parts.len(), 2);
        assert!(parts[0].points[0].x >= 1. - 1e-9 && parts[1].points[1].x <= 19. + 1e-9);
        assert!(parts[0].points[1].distance(Point::new(7., 7.)) >= 1. - 1e-9);
        assert!(parts[1].points[0].distance(Point::new(13., 13.)) >= 1. - 1e-9);
    }
}
