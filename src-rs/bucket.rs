//! Closed faces for paint-bucket fills, and physical monochrome pen patterns.
use crate::{MAX_POINTS, MAX_STROKES, Point, Stroke};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pattern {
    Dots,
    #[default]
    Diagonal,
    Cross,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PatternSettings {
    pub pattern: Pattern,
    #[serde(default, skip_serializing_if = "is_false")]
    pub clip_to_pen: bool,
    /// Relative coverage, 1..100. 100 maps to 75% geometric coverage.
    pub density: f64,
    pub pen_width_mm: f64,
    pub dot_diameter_mm: f64,
    pub angle_deg: f64,
}
fn is_false(value: &bool) -> bool {
    !value
}
impl Default for PatternSettings {
    fn default() -> Self {
        Self {
            pattern: Pattern::Diagonal,
            clip_to_pen: false,
            density: 35.,
            pen_width_mm: 0.3,
            dot_diameter_mm: 1.,
            angle_deg: 45.,
        }
    }
}
impl PatternSettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1.0..=100.).contains(&self.density)
            || !(0.05..=3.).contains(&self.pen_width_mm)
            || !(0.1..=10.).contains(&self.dot_diameter_mm)
            || self.dot_diameter_mm < self.pen_width_mm
            || !self.angle_deg.is_finite()
        {
            return Err("Pattern: density 1..100%, pen width 0.05..3 mm, dot diameter at least the pen width (up to 10 mm), finite angle required.".into());
        }
        Ok(())
    }
    pub fn spacing(&self) -> f64 {
        let coverage = self.density * 0.0075;
        match self.pattern {
            Pattern::Diagonal => self.pen_width_mm / coverage,
            Pattern::Cross => self.pen_width_mm / (1. - (1. - coverage).sqrt()),
            Pattern::Dots => self.dot_diameter_mm * (std::f64::consts::PI / (4. * coverage)).sqrt(),
        }
    }
}
fn cross(a: Point, b: Point) -> f64 {
    a.x * b.y - a.y * b.x
}
fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}
fn lerp(a: Point, b: Point, t: f64) -> Point {
    if t <= 0. {
        return a;
    }
    if t >= 1. {
        return b;
    }
    Point::new(
        (a.x + (b.x - a.x) * t).clamp(a.x.min(b.x), a.x.max(b.x)),
        (a.y + (b.y - a.y) * t).clamp(a.y.min(b.y), a.y.max(b.y)),
    )
}
fn distance(p: Point, a: Point, b: Point) -> f64 {
    let d = sub(b, a);
    let len2 = d.x * d.x + d.y * d.y;
    if len2 == 0. {
        return p.distance(a);
    }
    p.distance(lerp(
        a,
        b,
        (((p.x - a.x) * d.x + (p.y - a.y) * d.y) / len2).clamp(0., 1.),
    ))
}
fn edges(s: &Stroke) -> impl Iterator<Item = (Point, Point)> + '_ {
    s.points.windows(2).map(|p| (p[0], p[1])).chain(
        (s.closed && s.points.len() > 2 && s.points.first() != s.points.last())
            .then(|| (*s.points.last().unwrap(), s.points[0])),
    )
}
fn inside(p: Point, ring: &Stroke) -> bool {
    let mut yes = false;
    for (a, b) in edges(ring) {
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y) {
            yes = !yes;
        }
    }
    yes
}
pub fn region_contains(rings: &[Stroke], p: Point) -> bool {
    rings.iter().filter(|s| inside(p, s)).count() % 2 == 1
}
fn area(r: &Stroke) -> f64 {
    edges(r).map(|(a, b)| cross(a, b)).sum::<f64>() * 0.5
}

/// Polygonize visible linework. Endpoint joins, intersections and shared edges
/// are split before face traversal; open boundaries never become filled areas.
/// Join keys use 1e-7 mm precision to reconcile floating point crossings;
/// stored vertices retain their source coordinates so fills never shift artwork.
pub fn bucket_region(strokes: &[Stroke], point: Point) -> Result<Vec<Stroke>, String> {
    if !point.x.is_finite() || !point.y.is_finite() {
        return Err("Invalid bucket position".into());
    }
    crate::check_size(strokes)?;
    if strokes
        .iter()
        .flat_map(|s| &s.points)
        .any(|p| !p.x.is_finite() || !p.y.is_finite() || p.x.abs() > 1e8 || p.y.abs() > 1e8)
    {
        return Err("Invalid bucket boundary coordinates".into());
    }
    let mut segments: Vec<_> = strokes
        .iter()
        .flat_map(edges)
        .filter(|(a, b)| a.distance(*b) > 1e-7)
        .collect();
    if segments.len() > 12_000 {
        return Err("Bucket boundary is too complex; simplify the linework first.".into());
    }
    if segments.iter().any(|(a, b)| distance(point, *a, *b) < 1e-6) {
        return Err("Click inside a region, away from its boundary.".into());
    }
    segments.sort_by(|(a, b), (c, d)| a.x.min(b.x).total_cmp(&c.x.min(d.x)));
    let mut cuts = vec![vec![0., 1.]; segments.len()];
    let mut budget = 8_000_000usize;
    for i in 0..segments.len() {
        let (a, b) = segments[i];
        let r = sub(b, a);
        for j in i + 1..segments.len() {
            let (c, d) = segments[j];
            if c.x.min(d.x) > a.x.max(b.x) + 1e-7 {
                break;
            }
            budget = budget
                .checked_sub(1)
                .ok_or("Bucket intersection limit reached; simplify the linework.")?;
            if c.y.min(d.y) > a.y.max(b.y) + 1e-7 || c.y.max(d.y) < a.y.min(b.y) - 1e-7 {
                continue;
            }
            let s = sub(d, c);
            let den = cross(r, s);
            let q = sub(c, a);
            if den.abs() > 1e-12 {
                let (t, u) = (cross(q, s) / den, cross(q, r) / den);
                if (-1e-9..=1. + 1e-9).contains(&t) && (-1e-9..=1. + 1e-9).contains(&u) {
                    cuts[i].push(t.clamp(0., 1.));
                    cuts[j].push(u.clamp(0., 1.));
                }
            } else if cross(q, r).abs() < 1e-8 {
                for p in [c, d] {
                    let t = ((p.x - a.x) * r.x + (p.y - a.y) * r.y) / (r.x * r.x + r.y * r.y);
                    if (0.0..=1.).contains(&t) {
                        cuts[i].push(t);
                    }
                }
                for p in [a, b] {
                    let u = ((p.x - c.x) * s.x + (p.y - c.y) * s.y) / (s.x * s.x + s.y * s.y);
                    if (0.0..=1.).contains(&u) {
                        cuts[j].push(u);
                    }
                }
            }
            if cuts[i].len() > 20_000 || cuts[j].len() > 20_000 {
                return Err("Bucket has too many intersections".into());
            }
        }
    }
    let mut ids = BTreeMap::new();
    let mut vertices = Vec::new();
    let mut links = BTreeSet::new();
    for ((a, b), mut ts) in segments.into_iter().zip(cuts) {
        ts.sort_by(f64::total_cmp);
        ts.dedup_by(|a, b| (*a - *b).abs() < 1e-10);
        for pair in ts.windows(2) {
            let mut ends = [0; 2];
            for (k, t) in pair.iter().enumerate() {
                let p = lerp(a, b, *t);
                let key = ((p.x * 1e7).round() as i64, (p.y * 1e7).round() as i64);
                ends[k] = *ids.entry(key).or_insert_with(|| {
                    let id = vertices.len();
                    vertices.push(p);
                    id
                });
            }
            if ends[0] != ends[1] {
                links.insert((ends[0].min(ends[1]), ends[0].max(ends[1])));
            }
            if links.len() > 40_000 {
                return Err("Bucket has too many split edges".into());
            }
        }
    }
    let mut neighbors = vec![Vec::new(); vertices.len()];
    for &(a, b) in &links {
        neighbors[a].push(b);
        neighbors[b].push(a);
    }
    for (i, list) in neighbors.iter_mut().enumerate() {
        list.sort_by(|a, b| {
            let a = sub(vertices[*a], vertices[i]);
            let b = sub(vertices[*b], vertices[i]);
            a.y.atan2(a.x).total_cmp(&b.y.atan2(b.x))
        });
    }
    let mut visited = BTreeSet::new();
    let mut faces = Vec::new();
    for &(a, b) in &links {
        for start in [(a, b), (b, a)] {
            if visited.contains(&start) {
                continue;
            }
            let mut edge = start;
            let mut points = Vec::new();
            loop {
                if !visited.insert(edge) {
                    break;
                }
                points.push(vertices[edge.0]);
                let list = &neighbors[edge.1];
                let index = list.iter().position(|v| *v == edge.0).unwrap();
                edge = (edge.1, list[(index + list.len() - 1) % list.len()]);
                if edge == start {
                    break;
                }
            }
            if edge == start && points.len() >= 3 {
                points.push(points[0]);
                let ring = Stroke {
                    points,
                    layer: "#000000".into(),
                    group: 0,
                    closed: true,
                    filled: true,
                };
                if area(&ring) > 1e-8 {
                    faces.push(ring);
                }
            }
        }
    }
    faces.sort_by(|a, b| area(a).total_cmp(&area(b)));
    let index = faces
        .iter()
        .position(|r| inside(point, r))
        .ok_or("No closed region here. Close the outline, then click inside it.")?;
    let outer = &faces[index];
    let mut result = vec![outer.clone()];
    // Disconnected nested faces become holes; include immediate children only.
    for i in 0..index {
        let p = faces[i].points[0];
        if inside(p, outer) && !faces[i + 1..index].iter().any(|r| inside(p, r)) {
            result.push(faces[i].clone());
        }
    }
    crate::check_size(&result)?;
    Ok(result)
}

/// Solid polka dots are short continuous spirals, with ordinary G1 XY and Z lifts.
/// A dot is emitted only when its whole pen footprint fits inside the region.
pub(crate) fn dots(
    rings: &[&Stroke],
    layer: &str,
    group: u32,
    settings: &PatternSettings,
    out: &mut Vec<Stroke>,
    budget: &mut usize,
    allowed: usize,
) -> Result<(), String> {
    let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    let boundary: Vec<_> = rings.iter().flat_map(|s| edges(s)).collect();
    for &(a, b) in &boundary {
        for p in [a, b] {
            lo.x = lo.x.min(p.x);
            lo.y = lo.y.min(p.y);
            hi.x = hi.x.max(p.x);
            hi.y = hi.y.max(p.y);
        }
    }
    let spacing = settings.spacing();
    let radius = settings.dot_diameter_mm * 0.5;
    let nx = ((hi.x - lo.x) / spacing).ceil() as usize;
    let ny = ((hi.y - lo.y) / spacing).ceil() as usize;
    *budget = budget
        .checked_sub(nx.saturating_mul(ny).saturating_mul(boundary.len().max(1)))
        .ok_or("Dots too dense; reduce density or simplify the region")?;
    let ink_radius = (radius - settings.pen_width_mm * 0.5).max(0.001);
    let turns = (ink_radius / (settings.pen_width_mm * 0.75)).max(1.);
    let count = (turns * 24.).ceil() as usize;
    let mut point_count: usize = out.iter().map(|s| s.points.len()).sum();
    for row in 0..ny {
        for col in 0..nx {
            let center = Point::new(
                lo.x + (col as f64 + 0.5) * spacing,
                lo.y + (row as f64 + 0.5) * spacing,
            );
            if rings.iter().filter(|s| inside(center, s)).count() % 2 == 0
                || boundary.iter().any(|(a, b)| {
                    distance(center, *a, *b) < ink_radius + settings.pen_width_mm * 0.5 + 1e-7
                })
            {
                continue;
            }
            if out.len() >= allowed.min(MAX_STROKES) || point_count + count + 1 > MAX_POINTS {
                return Err("Dots exceed geometry limits; reduce density".into());
            }
            let points = (0..=count)
                .map(|i| {
                    let t = i as f64 / count as f64;
                    let angle = t * turns * std::f64::consts::TAU;
                    Point::new(
                        center.x + t * ink_radius * angle.cos(),
                        center.y + t * ink_radius * angle.sin(),
                    )
                })
                .collect();
            out.push(Stroke {
                points,
                layer: layer.into(),
                group,
                closed: false,
                filled: false,
            });
            point_count += count + 1;
        }
    }
    Ok(())
}
