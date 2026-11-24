//! Bounded quadratic bows between already validated pen-up positions.
use crate::Point;

const MIN_CHORD_MM: f64 = 2.0;
const MAX_CONTROL_OFFSET_MM: f64 = 1.5;
const MIN_CONTROL_OFFSET_MM: f64 = 0.10;
const MAX_APPROX_ERROR_MM: f64 = 0.02;
const MIN_SEGMENT_MM: f64 = 0.40;
const MAX_SEGMENTS: usize = 8;

fn dot(a: Point, b: Point) -> f64 {
    a.x * b.x + a.y * b.y
}

fn forward_hint(from: Point, to: Point, chord: Point, normal: Point) -> f64 {
    let length = from.distance(to);
    if length < 1e-6 {
        return 0.0;
    }
    let direction = Point::new((to.x - from.x) / length, (to.y - from.y) / length);
    // Reversals or perpendicular strokes do not imply overshooting or a U-turn.
    dot(direction, chord).max(0.0) * dot(direction, normal)
}

/// Checked, rounded intermediate positions only. The caller always emits the
/// exact original endpoint, and keeps its straight move if this returns None.
pub(crate) fn between(
    prior: [Point; 2],
    next: [Point; 2],
    budget: usize,
    check: impl Fn(Point) -> Option<Point>,
) -> Option<Vec<Point>> {
    if budget == 0 {
        return None;
    }
    let [before, start] = prior;
    let [end, after] = next;
    let length = start.distance(end);
    if length < MIN_CHORD_MM {
        return None;
    }
    let chord = Point::new((end.x - start.x) / length, (end.y - start.y) / length);
    let normal = Point::new(-chord.y, chord.x);
    let bend = (forward_hint(before, start, chord, normal)
        - forward_hint(end, after, chord, normal))
        * length
        * 0.18;
    let bend = bend.clamp(-MAX_CONTROL_OFFSET_MM, MAX_CONTROL_OFFSET_MM);
    if bend.abs() < MIN_CONTROL_OFFSET_MM {
        return None;
    }
    // The projection advances monotonically along the chord. The continuous
    // curve lies inside the checked control triangle and bows by at most 0.75 mm.
    let control = Point::new(
        (start.x + end.x) * 0.5 + normal.x * bend,
        (start.y + end.y) * 0.5 + normal.y * bend,
    );
    check(control)?;
    let count = (bend.abs() / (2.0 * MAX_APPROX_ERROR_MM)).sqrt().ceil() as usize;
    if !(2..=MAX_SEGMENTS).contains(&count)
        || count - 1 > budget
        || length / (count as f64) < MIN_SEGMENT_MM
    {
        return None;
    }
    let mut points = Vec::with_capacity(count - 1);
    let mut previous = start;
    for i in 1..count {
        let t = i as f64 / count as f64;
        let a = (1.0 - t).powi(2);
        let b = 2.0 * t * (1.0 - t);
        let c = t * t;
        let sample = check(Point::new(
            a * start.x + b * control.x + c * end.x,
            a * start.y + b * control.y + c * end.y,
        ))?;
        if previous.distance(sample) < MIN_SEGMENT_MM {
            return None;
        }
        points.push(sample);
        previous = sample;
    }
    (previous.distance(end) >= MIN_SEGMENT_MM).then_some(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approximation_and_budget_are_bounded() {
        let prior = [Point::new(0., 0.), Point::new(10., 10.)];
        let next = [Point::new(30., 10.), Point::new(40., 0.)];
        assert!(between(prior, next, 1, Some).is_none());
        assert!(between(prior, next, 8, |p| (p.y <= 11.0).then_some(p)).is_none());
        let points = between(prior, next, 8, Some).unwrap();
        let count = points.len() + 1;
        let mut all = vec![prior[1]];
        all.extend(points);
        all.push(next[0]);
        for (i, pair) in all.windows(2).enumerate() {
            let t = (i as f64 + 0.5) / count as f64;
            let exact_y = 10.0 + 2.0 * (1.0 - t) * t * MAX_CONTROL_OFFSET_MM;
            let approximate_y = (pair[0].y + pair[1].y) * 0.5;
            assert!((exact_y - approximate_y).abs() <= MAX_APPROX_ERROR_MM);
            assert!(pair[0].distance(pair[1]) >= MIN_SEGMENT_MM);
        }
    }
}
