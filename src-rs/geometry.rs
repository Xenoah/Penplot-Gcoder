//! Planning works on artwork, not on machine coordinates. Export validates the latter independently.
use crate::{MAX_POINTS, MAX_STROKES, Point, Profile, Stroke, check_size};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    #[default]
    #[serde(alias = "outline", alias = "outline_only")]
    None,
    #[serde(alias = "hatch", alias = "hatch_fill")]
    Lines,
    #[serde(alias = "cross_hatch")]
    Cross,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobOptions {
    /// 1.0 = original metric size. Position anchors the rotated bounding box's lower left.
    pub scale: f64,
    pub rotation_deg: f64,
    pub x: f64,
    pub y: f64,
    pub optimize: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub smooth_pen_up: bool,
    pub fill: Fill,
    pub hatch_spacing: f64,
    pub hatch_angle: f64,
    #[serde(default, skip_serializing_if = "is_zero", alias = "min_image_stroke")]
    pub min_image_stroke_mm: f64,
}
impl Default for JobOptions {
    fn default() -> Self {
        Self {
            scale: 1.,
            rotation_deg: 0.,
            x: 0.,
            y: 0.,
            optimize: false,
            smooth_pen_up: false,
            fill: Fill::None,
            hatch_spacing: 1.,
            hatch_angle: 45.,
            min_image_stroke_mm: 0.,
        }
    }
}
fn is_false(value: &bool) -> bool {
    !value
}
fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

/// Raster settings saved independently of the source image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RasterSettings {
    #[serde(default = "default_outline", skip_serializing_if = "is_true")]
    pub outline: bool,
    #[serde(alias = "mode", alias = "fill_mode")]
    pub fill: Fill,
    #[serde(alias = "spacing", alias = "hatch_spacing_mm")]
    pub hatch_spacing: f64,
    #[serde(alias = "angle", alias = "hatch_angle_deg")]
    pub hatch_angle: f64,
}

fn default_outline() -> bool {
    true
}
fn is_true(value: &bool) -> bool {
    *value
}

impl Default for RasterSettings {
    fn default() -> Self {
        Self {
            fill: Fill::None,
            outline: true,
            hatch_spacing: 1.,
            hatch_angle: 45.,
        }
    }
}

impl RasterSettings {
    pub fn from_job(o: &JobOptions) -> Self {
        Self {
            outline: true,
            fill: o.fill,
            hatch_spacing: o.hatch_spacing,
            hatch_angle: o.hatch_angle,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.hatch_angle.is_finite() && (0.02..=1000.).contains(&self.hatch_spacing) {
            Ok(())
        } else {
            Err("Hatch spacing must be 0.02 to 1000 mm; angle must be finite".into())
        }
    }
}

pub type RasterLayers = BTreeMap<usize, RasterSettings>;

#[derive(Debug)]
pub struct PlanResult {
    pub strokes: Vec<Stroke>,
    pub removed_count: usize,
    pub removed_mm: f64,
}

#[derive(Clone, Copy)]
struct Bounds {
    lo: Point,
    hi: Point,
}
impl Bounds {
    fn empty() -> Self {
        Self {
            lo: Point::new(f64::INFINITY, f64::INFINITY),
            hi: Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
        }
    }
    fn push(&mut self, p: Point) {
        self.lo.x = self.lo.x.min(p.x);
        self.lo.y = self.lo.y.min(p.y);
        self.hi.x = self.hi.x.max(p.x);
        self.hi.y = self.hi.y.max(p.y);
    }
}
fn rotate(p: Point, (s, c): (f64, f64)) -> Point {
    Point::new(c * p.x - s * p.y, s * p.x + c * p.y)
}
fn rotated_bounds(strokes: &[Stroke], angle: f64) -> Result<Bounds, String> {
    check_size(strokes)?;
    if strokes.is_empty() {
        return Err("No visible paths to plan".into());
    }
    if !angle.is_finite() {
        return Err("Rotation is not finite".into());
    }
    let trig = angle.rem_euclid(360.).to_radians().sin_cos();
    let mut b = Bounds::empty();
    for s in strokes {
        if s.points.len() < 2 {
            return Err("A path needs at least two points".into());
        }
        for &p in &s.points {
            if ![p.x, p.y].iter().all(|v| v.is_finite() && v.abs() <= 1e8) {
                return Err("Non-finite or excessive source coordinates".into());
            }
            b.push(rotate(p, trig));
        }
    }
    Ok(b)
}
/// Fit uniformly to the measured common pen/carriage area, without dividing by a zero axis.
pub fn fit(
    strokes: &[Stroke],
    profile: &Profile,
    opts: &mut JobOptions,
    margin: f64,
) -> Result<(), String> {
    profile.validate()?;
    if !margin.is_finite() || margin < 0. {
        return Err("Margin must be non-negative".into());
    }
    let b = rotated_bounds(strokes, opts.rotation_deg)?;
    let (minx, miny, maxx, maxy) = profile.effective_area();
    let (w, h) = (maxx - minx - 2. * margin, maxy - miny - 2. * margin);
    if w <= 0. || h <= 0. {
        return Err("Margin leaves no usable drawing area".into());
    }
    let (sw, sh) = (b.hi.x - b.lo.x, b.hi.y - b.lo.y);
    let sc = (if sw > 1e-10 { w / sw } else { f64::INFINITY }).min(if sh > 1e-10 {
        h / sh
    } else {
        f64::INFINITY
    });
    if !sc.is_finite() || sc <= 0. || sc > 1e8 {
        return Err("Cannot fit zero-sized or microscopic artwork".into());
    }
    opts.scale = sc;
    opts.x = (minx + maxx - sw * sc) / 2.;
    opts.y = (miny + maxy - sh * sc) / 2.;
    Ok(())
}

pub fn plan(strokes: &[Stroke], o: &JobOptions) -> Result<Vec<Stroke>, String> {
    plan_with_raster(strokes, o, &RasterLayers::new()).map(|result| result.strokes)
}

pub fn plan_with_raster(
    strokes: &[Stroke],
    o: &JobOptions,
    layers: &RasterLayers,
) -> Result<PlanResult, String> {
    if !(0.0..=10_000.).contains(&o.min_image_stroke_mm) {
        return Err("Minimum image stroke must be 0 to 10000 mm".into());
    }
    for settings in layers.values() {
        settings.validate()?;
    }
    let raster = |layer: &str| -> Option<&RasterSettings> {
        let (prefix, _) = layer.split_once(':')?;
        layers.get(&prefix.parse::<usize>().ok()?)
    };
    let job = RasterSettings::from_job(o);
    let b = rotated_bounds(strokes, o.rotation_deg)?;
    if ![o.scale, o.x, o.y]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= 1e8)
        || o.scale <= 0.
    {
        return Err("Invalid scale or placement".into());
    }
    if o.fill != Fill::None
        && (!o.hatch_angle.is_finite() || !(0.02..=1000.).contains(&o.hatch_spacing))
    {
        return Err("Hatch spacing must be 0.02 to 1000 mm; angle must be finite".into());
    }
    let trig = o.rotation_deg.rem_euclid(360.).to_radians().sin_cos();
    let mut out = Vec::new();
    for src in strokes {
        let mut s = src.clone();
        for p in &mut s.points {
            let r = rotate(*p, trig);
            *p = Point::new(
                (r.x - b.lo.x) * o.scale + o.x,
                (r.y - b.lo.y) * o.scale + o.y,
            );
            if ![p.x, p.y].iter().all(|v| v.abs() <= 1e8) {
                return Err("Transformed coordinates exceed planner limits".into());
            }
        }
        s.points.dedup();
        if s.points.len() < 2 {
            continue;
        }
        if s.closed && s.points.first() != s.points.last() {
            s.points.push(s.points[0]);
        }
        out.push(s);
    }
    if out.is_empty() {
        return Err("All visible paths have zero length".into());
    }
    check_size(&out)?;
    if o.fill != Fill::None || layers.values().any(|r| r.fill != Fill::None) {
        let mut groups: BTreeMap<(&str, u32), Vec<&Stroke>> = BTreeMap::new();
        for s in &out {
            if s.filled
                && s.closed
                && s.points.len() >= 4
                && raster(&s.layer).unwrap_or(&job).fill != Fill::None
            {
                groups.entry((&s.layer, s.group)).or_default().push(s);
            }
        }
        let mut fill = Vec::new();
        let mut budget = 8_000_000;
        let allowed = MAX_STROKES
            .saturating_sub(out.len())
            .min((MAX_POINTS - out.iter().map(|s| s.points.len()).sum::<usize>()) / 2);
        for ((layer, group), rings) in groups {
            let settings = raster(layer).unwrap_or(&job);
            hatch(
                &rings,
                layer,
                group,
                settings.hatch_spacing,
                settings.hatch_angle,
                &mut fill,
                &mut budget,
                allowed,
            )?;
            if settings.fill == Fill::Cross {
                hatch(
                    &rings,
                    layer,
                    group,
                    settings.hatch_spacing,
                    settings.hatch_angle + 90.,
                    &mut fill,
                    &mut budget,
                    allowed,
                )?;
            }
        }
        out.retain(|s| {
            let settings = raster(&s.layer).unwrap_or(&job);
            settings.outline || settings.fill == Fill::None || !s.filled || !s.closed
        });
        out.extend(fill);
    }
    let mut removed_count = 0;
    let mut removed_mm = 0.0;
    if o.min_image_stroke_mm > 0.0 && !layers.is_empty() {
        out.retain(|stroke| {
            if raster(&stroke.layer).is_none() {
                return true;
            }
            let length: f64 = stroke.points.windows(2).map(|p| p[0].distance(p[1])).sum();
            let slack = 1e-10 * o.min_image_stroke_mm;
            if length < o.min_image_stroke_mm - slack {
                removed_count += 1;
                removed_mm += length;
                false
            } else {
                true
            }
        });
    }
    if o.optimize {
        optimize_runs(&mut out);
    }
    Ok(PlanResult {
        strokes: out,
        removed_count,
        removed_mm,
    })
}
/// Even/odd clipping across the rings of ONE compound shape, never across unrelated objects.
/// Scanline intersections use half-open edges (a polygon vertex must not be counted twice).
#[allow(clippy::too_many_arguments)]
fn hatch(
    rings: &[&Stroke],
    layer: &str,
    group: u32,
    spacing: f64,
    angle: f64,
    out: &mut Vec<Stroke>,
    budget: &mut usize,
    allowed: usize,
) -> Result<(), String> {
    let rad = angle.rem_euclid(360.).to_radians();
    let forward = (-rad).sin_cos();
    let back = rad.sin_cos();
    let mut edges = Vec::new();
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for ring in rings {
        for p in ring.points.windows(2) {
            let a = rotate(p[0], forward);
            let b = rotate(p[1], forward);
            lo = lo.min(a.y).min(b.y);
            hi = hi.max(a.y).max(b.y);
            if (a.y - b.y).abs() > 1e-12 {
                edges.push((a, b));
            }
        }
    }
    let rows = ((hi - lo) / spacing).ceil() as usize;
    *budget = budget
        .checked_sub(rows.saturating_mul(edges.len().max(1)))
        .ok_or("Hatch too dense for this artwork; increase spacing or simplify")?;
    let mut xs = Vec::new();
    for row in 0..rows {
        let y = lo + (row as f64 + 0.5) * spacing;
        xs.clear();
        for &(a, b) in &edges {
            if (a.y <= y && y < b.y) || (b.y <= y && y < a.y) {
                xs.push(a.x + (b.x - a.x) * (y - a.y) / (b.y - a.y));
            }
        }
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks_exact(2) {
            if pair[1] - pair[0] <= 1e-9 {
                continue;
            }
            if out.len() >= allowed {
                return Err("Hatch exceeds path/point limit; increase spacing".into());
            }
            let (a, b) = if row % 2 == 0 {
                (pair[0], pair[1])
            } else {
                (pair[1], pair[0])
            };
            out.push(Stroke {
                points: vec![
                    rotate(Point::new(a, y), back),
                    rotate(Point::new(b, y), back),
                ],
                layer: layer.to_owned(),
                group,
                closed: false,
                filled: false,
            });
        }
    }
    Ok(())
}
fn travel_len(s: &[Stroke], from: Point) -> f64 {
    let mut cur = from;
    let mut total = 0.;
    for p in s {
        total += cur.distance(p.points[0]);
        cur = *p.points.last().unwrap();
    }
    total
}
fn optimize_runs(out: &mut [Stroke]) {
    // Preserve layer runs, never add a pen-down join. Restrict search on large drawings.
    let mut cur = Point::default();
    let mut start = 0;
    while start < out.len() {
        let end = start
            + out[start..]
                .iter()
                .take_while(|s| s.layer == out[start].layer)
                .count();
        let run = &mut out[start..end];
        let previous = travel_len(run, cur);
        let mut order: Vec<usize> = (0..run.len()).collect();
        let mut rev = vec![false; run.len()];
        let mut p = cur;
        for i in 0..run.len() {
            let n = if run.len() <= 1024 {
                run.len()
            } else {
                (i + 64).min(run.len())
            };
            let (mut best, mut bd, mut flip) = (i, f64::INFINITY, false);
            for (k, &idx) in order.iter().enumerate().take(n).skip(i) {
                let s = &run[idx];
                let d = p.distance(s.points[0]);
                if d < bd {
                    best = k;
                    bd = d;
                    flip = false;
                }
                if !s.closed {
                    let d = p.distance(*s.points.last().unwrap());
                    if d < bd {
                        best = k;
                        bd = d;
                        flip = true;
                    }
                }
            }
            order.swap(i, best);
            let idx = order[i];
            rev[i] = flip;
            p = if flip {
                run[idx].points[0]
            } else {
                *run[idx].points.last().unwrap()
            };
        }
        let mut p = cur;
        let mut new_len = 0.;
        for (k, &idx) in order.iter().enumerate() {
            let s = &run[idx];
            let (a, b) = (s.points[0], *s.points.last().unwrap());
            new_len += p.distance(if rev[k] { b } else { a });
            p = if rev[k] { a } else { b };
        }
        if new_len < previous {
            let orig: Vec<_> = run
                .iter_mut()
                .map(|s| {
                    Some(Stroke {
                        points: std::mem::take(&mut s.points),
                        layer: std::mem::take(&mut s.layer),
                        group: s.group,
                        closed: s.closed,
                        filled: s.filled,
                    })
                })
                .collect();
            let mut orig = orig;
            for (i, &idx) in order.iter().enumerate() {
                let mut s = orig[idx].take().unwrap();
                if rev[i] {
                    s.points.reverse();
                }
                run[i] = s;
            }
        }
        cur = *run.last().unwrap().points.last().unwrap();
        start = end;
    }
}
