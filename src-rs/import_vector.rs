//! Vector readers. Points are expressed in millimetres, with Y pointing up.
use crate::{Artwork, Point, Stroke};
use std::{
    collections::BTreeSet,
    f64::consts::{PI, TAU},
};
use usvg::tiny_skia_path::PathSegment;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_POINTS: usize = 500_000;
const MAX_STROKES: usize = 50_000;
const MAX_COORD: f64 = 1.0e9;
const MM_PER_PX: f64 = 25.4 / 96.0;

fn check_input(bytes: &[u8], tolerance: f64) -> Result<(), String> {
    if bytes.len() > MAX_BYTES {
        return Err("Vector input exceeds 8 MiB".into());
    }
    if !tolerance.is_finite() || !(0.001..=100.0).contains(&tolerance) {
        return Err("Curve tolerance must be between 0.001 and 100 mm".into());
    }
    Ok(())
}

fn near(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
}
fn midpoint(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}
fn validate_point(p: Point) -> Result<Point, String> {
    if !(p.x.abs() <= MAX_COORD && p.y.abs() <= MAX_COORD) {
        return Err("Vector coordinate is non-finite or exceeds 10⁹ mm".into());
    }
    Ok(p)
}

#[derive(Default)]
struct Output {
    art: Artwork,
    warnings: BTreeSet<String>,
    points: usize,
}
impl Output {
    fn warn(&mut self, s: impl Into<String>) {
        if self.warnings.len() < 64 {
            self.warnings.insert(s.into());
        }
    }
    fn add(
        &mut self,
        mut points: Vec<Point>,
        layer: &str,
        group: u32,
        closed: bool,
        filled: bool,
    ) -> Result<(), String> {
        for &pt in &points {
            validate_point(pt)?;
        }
        points.dedup_by(|a, b| near(*a, *b));
        if points.len() < 2 {
            return Ok(());
        }
        if closed && !near(points[0], points[points.len() - 1]) {
            points.push(points[0]);
        }
        self.points += points.len();
        if self.points > MAX_POINTS || self.art.strokes.len() >= MAX_STROKES {
            return Err("Vector geometry exceeds the plot complexity limit".into());
        }
        self.art.strokes.push(Stroke {
            points,
            layer: layer.into(),
            group,
            closed,
            filled,
        });
        Ok(())
    }
    fn finish(mut self, format: &str) -> Result<Artwork, String> {
        if self.art.strokes.is_empty() {
            let msg = self.warnings.into_iter().collect::<Vec<_>>().join("; ");
            return Err(if msg.is_empty() {
                format!("No plottable paths found in {format}")
            } else {
                format!("No plottable paths found in {format}: {msg}")
            });
        }
        self.art.warnings.extend(self.warnings);
        Ok(self.art)
    }
}

pub fn svg(bytes: &[u8], tolerance: f64) -> Result<Artwork, String> {
    check_input(bytes, tolerance)?;
    let input = std::str::from_utf8(bytes)
        .map_err(|_| "SVG must be UTF-8")?
        .trim_start_matches('\u{feff}');
    let doc = usvg::roxmltree::Document::parse_with_options(
        input,
        usvg::roxmltree::ParsingOptions {
            nodes_limit: 50_000,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Invalid SVG XML: {e}"))?;
    // Limit recursion before handing references to the normalizer. Nested use is
    // refused because a compact definition graph can expand exponentially.
    let mut output = Output::default();
    let mut use_count = 0;
    let mut referenced_ids = BTreeSet::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("use")) {
        referenced_ids.extend(
            node.attributes()
                .filter(|a| a.name() == "href")
                .filter_map(|a| a.value().strip_prefix('#')),
        );
    }
    for node in doc.descendants().filter(|n| n.is_element()) {
        if node.ancestors().take(66).count() > 64 {
            return Err("SVG nesting exceeds the import limit".into());
        }
        match node.tag_name().name() {
            "use" => {
                use_count += 1;
                if use_count > 1000
                    || use_count * input.len() > 16 * MAX_BYTES
                    || node.ancestors().any(|a| {
                        a.attribute("id")
                            .is_some_and(|id| referenced_ids.contains(id))
                    })
                {
                    return Err("SVG contains too many or nested reusable elements; expand clones before importing".into());
                }
                for attr in node.attributes().filter(|a| a.name() == "href") {
                    if !attr.value().starts_with('#') {
                        output.warn("SVG external references were ignored");
                    }
                }
            }
            "text" | "tspan" | "textPath" => {
                output.warn("SVG text was ignored; convert text to paths")
            }
            "image" | "foreignObject" => output
                .warn("Embedded images/foreign objects were ignored; trace or import separately"),
            "script" | "animate" | "animateTransform" | "set" => {
                output.warn("SVG scripts and animation were ignored")
            }
            _ => {}
        }
    }
    if doc.descendants().any(|n| n.has_tag_name("marker")) {
        return Err(
            "SVG contains markers; convert arrowheads and other markers to paths before importing"
                .into(),
        );
    }
    let options = usvg::Options {
        dpi: 96.0,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_xmltree(&doc, &options).map_err(|e| format!("Invalid SVG: {e}"))?;
    svg_children(
        tree.root(),
        f64::from(tree.size().height()),
        tolerance,
        &mut 0,
        &mut output,
    )?;
    output.finish("SVG")
}

fn svg_children(
    group: &usvg::Group,
    height_px: f64,
    tol: f64,
    id: &mut u32,
    output: &mut Output,
) -> Result<(), String> {
    if group.opacity().get() == 0.0 {
        return Ok(());
    }
    if group.clip_path().is_some() || group.mask().is_some() || !group.filters().is_empty() {
        output.warn("SVG content with clip paths, masks or filters was omitted; flatten visual effects first");
        return Ok(());
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(child) => svg_children(child, height_px, tol, id, output)?,
            usvg::Node::Path(path) if path.is_visible() => {
                *id += 1;
                svg_path(path, height_px, tol, *id, output)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn svg_path(
    path: &usvg::Path,
    height_px: f64,
    tol: f64,
    id: u32,
    output: &mut Output,
) -> Result<(), String> {
    let fill = path.fill().filter(|f| f.opacity().get() > 0.0);
    let stroke = path.stroke().filter(|s| s.opacity().get() > 0.0);
    let Some(paint) = stroke
        .map(|s| s.paint())
        .or_else(|| fill.map(|f| f.paint()))
    else {
        return Ok(());
    };
    let layer = match paint {
        usvg::Paint::Color(c) => format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue),
        _ => {
            output.warn("SVG gradient/pattern paint is plotted as a single pen line");
            String::from("#000000")
        }
    };
    if stroke.is_some_and(|s| s.dasharray().is_some()) {
        output.warn("Dashed SVG strokes are imported as solid centerlines; convert dashes to separate paths for gaps");
    }
    let transform = path.abs_transform();
    let convert = |p: usvg::tiny_skia_path::Point| {
        let x = f64::from(p.x);
        let y = f64::from(p.y);
        validate_point(Point::new(
            (f64::from(transform.sx) * x + f64::from(transform.kx) * y + f64::from(transform.tx))
                * MM_PER_PX,
            (height_px
                - f64::from(transform.ky) * x
                - f64::from(transform.sy) * y
                - f64::from(transform.ty))
                * MM_PER_PX,
        ))
    };
    let mut current = Vec::new();
    let mut paths = Vec::new();
    let mut count = 0usize;
    let mut last = Point::new(0.0, 0.0);
    let mut start = last;
    for seg in path.data().segments() {
        let old_len = current.len();
        match seg {
            PathSegment::MoveTo(p) => {
                if !current.is_empty() {
                    paths.push((std::mem::take(&mut current), false));
                }
                last = convert(p)?;
                start = last;
                current.push(last);
            }
            PathSegment::LineTo(p) => {
                if current.is_empty() {
                    current.push(last);
                }
                last = convert(p)?;
                current.push(last);
            }
            PathSegment::QuadTo(c, p) => {
                if current.is_empty() {
                    current.push(last);
                }
                let c = convert(c)?;
                let end = convert(p)?;
                let p1 = Point::new((last.x + 2.0 * c.x) / 3.0, (last.y + 2.0 * c.y) / 3.0);
                let p2 = Point::new((end.x + 2.0 * c.x) / 3.0, (end.y + 2.0 * c.y) / 3.0);
                subdivide([last, p1, p2, end], tol, 0, &mut current)?;
                last = end;
            }
            PathSegment::CubicTo(a, b, p) => {
                if current.is_empty() {
                    current.push(last);
                }
                let end = convert(p)?;
                subdivide([last, convert(a)?, convert(b)?, end], tol, 0, &mut current)?;
                last = end;
            }
            PathSegment::Close => {
                if !current.is_empty() {
                    paths.push((std::mem::take(&mut current), true));
                }
                last = start;
            }
        }
        count += if matches!(seg, PathSegment::MoveTo(_)) {
            1
        } else {
            current.len().saturating_sub(old_len)
        };
        if count > MAX_POINTS || paths.len() > MAX_STROKES {
            return Err("SVG path exceeds the plot complexity limit".into());
        }
    }
    if !current.is_empty() {
        paths.push((current, false));
    }
    let compound_nonzero =
        fill.is_some_and(|f| f.rule() == usvg::FillRule::NonZero) && paths.len() > 1;
    if compound_nonzero {
        output.warn("Hatch skipped for compound SVG nonzero fill; use fill-rule=evenodd or split into separate shapes");
    }
    for (points, explicit_close) in paths {
        let same_endpoint = points.len() >= 3 && near(points[0], points[points.len() - 1]);
        let implicit_fill_outline = fill.is_some() && stroke.is_none() && points.len() >= 3;
        let closed = explicit_close || same_endpoint || implicit_fill_outline;
        if fill.is_some() && !closed && points.len() >= 3 {
            output.warn("Hatch skipped for open SVG fill boundaries with strokes; close the SVG path to hatch it");
        }
        output.add(
            points,
            &layer,
            id,
            closed,
            closed && fill.is_some() && !compound_nonzero,
        )?;
    }
    Ok(())
}

fn line_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length_sq = dx * dx + dy * dy;
    let frac = if length_sq == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / length_sq).clamp(0.0, 1.0)
    };
    (p.x - a.x - frac * dx).hypot(p.y - a.y - frac * dy)
}

fn subdivide(
    [a, b, c, d]: [Point; 4],
    tol: f64,
    depth: usize,
    pts: &mut Vec<Point>,
) -> Result<(), String> {
    if pts.len() >= MAX_POINTS {
        return Err("SVG curve exceeds the plot complexity limit".into());
    }
    if line_distance(b, a, d).max(line_distance(c, a, d)) <= tol {
        pts.push(d);
        return Ok(());
    }
    if depth >= 28 {
        return Err("SVG curve exceeds the subdivision depth limit".into());
    }
    let ab = midpoint(a, b);
    let bc = midpoint(b, c);
    let cd = midpoint(c, d);
    let abc = midpoint(ab, bc);
    let bcd = midpoint(bc, cd);
    let mid = midpoint(abc, bcd);
    subdivide([a, ab, abc, mid], tol, depth + 1, pts)?;
    subdivide([mid, bcd, cd, d], tol, depth + 1, pts)
}

type Pair<'a> = (i16, &'a str);
type Pairs<'a> = &'a [Pair<'a>];

fn lookup<'a>(pairs: Pairs<'a>, code: i16) -> Option<&'a str> {
    pairs
        .iter()
        .find_map(|&(c, value)| (c == code).then_some(value))
}
fn number(text: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
        .ok_or_else(|| "DXF contains an invalid number".into())
}
fn num(pairs: Pairs<'_>, code: i16, default: f64) -> Result<f64, String> {
    lookup(pairs, code)
        .map(number)
        .transpose()
        .map(|v| v.unwrap_or(default))
}
fn required(pairs: Pairs<'_>, code: i16) -> Result<f64, String> {
    number(
        lookup(pairs, code)
            .ok_or_else(|| format!("DXF entity is missing required group code {code}"))?,
    )
}
fn integer(pairs: Pairs<'_>, code: i16) -> Result<i32, String> {
    lookup(pairs, code)
        .unwrap_or("0")
        .parse()
        .map_err(|_| format!("Invalid DXF integer (group code {code})"))
}
fn xy(pairs: Pairs<'_>, xcode: i16, factor: f64) -> Result<Point, String> {
    validate_point(Point::new(
        required(pairs, xcode)? * factor,
        required(pairs, xcode + 10)? * factor,
    ))
}
fn units_factor(val: &str) -> Option<f64> {
    Some(match val.parse::<u8>().ok()? {
        0 | 4 => 1.0,
        1 => 25.4,
        2 => 304.8,
        3 => 1_609_344.0,
        5 => 10.0,
        6 => 1000.0,
        7 => 1.0e6,
        8 => 2.54e-5,
        9 => 0.0254,
        10 => 914.4,
        11 => 1e-7,
        12 => 1e-6,
        13 => 0.001,
        14 => 100.0,
        15 => 10_000.0,
        16 => 100_000.0,
        17 => 1.0e12,
        18 => 1.495_978_707e14,
        19 => 9.460_730_472_580_8e18,
        20 => 3.085_677_581_491_367e19,
        21 => 1_200_000.0 / 3937.0,
        22 => 100_000.0 / 3937.0,
        23 => 3_600_000.0 / 3937.0,
        24 => 6_336_000_000.0 / 3937.0,
        _ => return None,
    })
}

pub fn dxf(bytes: &[u8], tolerance: f64) -> Result<Artwork, String> {
    check_input(bytes, tolerance)?;
    if bytes.starts_with(b"AutoCAD Binary DXF") {
        return Err("Binary DXF is unsupported; save as ASCII DXF".into());
    }
    let source = std::str::from_utf8(bytes)
        .map_err(|_| "DXF must be UTF-8/ASCII; save as UTF-8 ASCII DXF")?
        .trim_start_matches('\u{feff}');
    let mut lines = source.lines();
    let mut pairs = Vec::new();
    while let Some(code) = lines.next() {
        if code.trim().is_empty() && lines.clone().all(|line| line.trim().is_empty()) {
            break;
        }
        let code = code
            .trim()
            .parse::<i16>()
            .map_err(|_| format!("Invalid DXF group code at line {}", pairs.len() * 2 + 1))?;
        let value = lines
            .next()
            .ok_or("Truncated DXF group code/value pair")?
            .trim();
        if value.len() > 4096 || pairs.len() >= 500_000 {
            return Err("DXF input exceeds the group size or count limit".into());
        }
        pairs.push((code, value));
    }
    let mut entities = None;
    let mut hidden_layers = BTreeSet::new();
    let mut units = None;
    let mut in_header = false;
    let mut saw_eof = false;
    let mut i = 0;
    while i < pairs.len() {
        match pairs[i] {
            (0, "SECTION") => {
                let section = match pairs.get(i + 1) {
                    Some(&(2, name)) => name,
                    _ => return Err("Malformed DXF SECTION".into()),
                };
                let start = i + 2;
                let end = pairs[start..]
                    .iter()
                    .position(|p| *p == (0, "ENDSEC"))
                    .map(|offset| start + offset)
                    .ok_or("Unterminated DXF SECTION")?;
                if section == "ENTITIES" && entities.replace(&pairs[start..end]).is_some() {
                    return Err("Repeated DXF ENTITIES section".into());
                }
                if section == "TABLES" {
                    let table = &pairs[start..end];
                    let mut records = table
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| p.0 == 0)
                        .peekable();
                    while let Some((pos, &(_, kind))) = records.next() {
                        if kind != "LAYER" {
                            continue;
                        }
                        let fields =
                            &table[pos + 1..records.peek().map_or(table.len(), |(i, _)| *i)];
                        if (integer(fields, 62)? < 0 || integer(fields, 70)? & 1 != 0)
                            && let Some(name) = lookup(fields, 2)
                        {
                            hidden_layers.insert(name.to_lowercase());
                        }
                    }
                }
                if section == "HEADER" {
                    if in_header {
                        return Err("Repeated DXF HEADER section".into());
                    }
                    in_header = true;
                    if let Some(n) = pairs[start..end]
                        .iter()
                        .position(|p| *p == (9, "$INSUNITS"))
                    {
                        let value = pairs
                            .get(start + n + 1)
                            .filter(|p| p.0 == 70)
                            .ok_or("Malformed DXF $INSUNITS")?
                            .1;
                        units = Some(value);
                    }
                }
                i = end + 1;
            }
            (0, "EOF") => {
                saw_eof = true;
                break;
            }
            (999, _) => i += 1,
            _ => return Err("Unexpected data outside DXF sections".into()),
        }
    }
    if !saw_eof {
        return Err("DXF is missing EOF marker".into());
    }
    let ents = entities.ok_or("DXF has no ENTITIES section")?;
    let units = units.unwrap_or("0");
    let scale = units_factor(units).ok_or("Unsupported DXF $INSUNITS value")?;
    let mut output = Output::default();
    if units == "0" {
        output.warn("DXF has no defined units; assumed 1 drawing unit = 1 mm");
    }
    let mut iter = ents
        .iter()
        .enumerate()
        .filter(|(_, p)| p.0 == 0)
        .map(|(i, p)| (i, p.1))
        .peekable();
    if ents.first().is_some_and(|p| p.0 != 0 && p.0 != 999) {
        return Err("Unexpected DXF value before first entity".into());
    }
    let mut group = 0;
    while let Some((pos, kind)) = iter.next() {
        let fields = &ents[pos + 1..iter.peek().map_or(ents.len(), |x| x.0)];
        let model = integer(fields, 67)? == 0
            && lookup(fields, 410).is_none_or(|s| s.eq_ignore_ascii_case("model"));
        let planar = num(fields, 210, 0.0)? == 0.0
            && num(fields, 220, 0.0)? == 0.0
            && num(fields, 230, 1.0)? == 1.0;
        let visible = integer(fields, 60)? == 0;
        let layer = lookup(fields, 8).unwrap_or("0");
        let shown = model && visible && !hidden_layers.contains(&layer.to_lowercase());
        let active = shown && planar;
        if shown && !planar {
            output.warn("DXF entities with a tilted or inverted extrusion axis were omitted");
        }
        group += 1;
        if kind == "POLYLINE" {
            let flags = integer(fields, 70)?;
            let mut vertices = Vec::new();
            let mut terminated = false;
            while let Some(&(vpos, vkind)) = iter.peek() {
                if vkind == "SEQEND" {
                    iter.next();
                    terminated = true;
                    break;
                }
                if vkind != "VERTEX" {
                    break;
                }
                iter.next();
                if active && flags & !129 == 0 {
                    let vertex = &ents[vpos + 1..iter.peek().map_or(ents.len(), |x| x.0)];
                    vertices.push((xy(vertex, 10, scale)?, num(vertex, 42, 0.0)?));
                }
            }
            if !terminated {
                return Err("DXF POLYLINE is missing SEQEND".into());
            }
            if active {
                if flags & !129 == 0 {
                    let closed = flags & 1 != 0;
                    let points = sample_poly(&vertices, closed, tolerance)?;
                    output.add(points, layer, group, closed, false)?;
                } else {
                    output.warn("DXF fitted, 3D, or mesh POLYLINE was omitted");
                }
            }
            continue;
        }
        if !active {
            continue;
        }
        let (points, closed) = match kind {
            "LINE" => (vec![xy(fields, 10, scale)?, xy(fields, 11, scale)?], false),
            "LWPOLYLINE" => {
                let mut vertices: Vec<(Point, f64, bool)> = Vec::new();
                for &(code, value) in fields {
                    match code {
                        10 => vertices.push((Point::new(number(value)? * scale, 0.0), 0.0, false)),
                        20 | 42 => {
                            let vertex = vertices
                                .last_mut()
                                .ok_or("DXF LWPOLYLINE vertex is missing X")?;
                            if code == 20 {
                                vertex.0.y = number(value)? * scale;
                                vertex.2 = true;
                            } else {
                                vertex.1 = number(value)?;
                            }
                        }
                        _ => {}
                    }
                }
                if vertices.iter().any(|v| !v.2) {
                    return Err("DXF LWPOLYLINE vertex is missing Y".into());
                }
                if lookup(fields, 90).is_some()
                    && usize::try_from(integer(fields, 90)?).ok() != Some(vertices.len())
                {
                    return Err("DXF LWPOLYLINE vertex count disagrees with its data".into());
                }
                let vertices = vertices
                    .into_iter()
                    .map(|(p, bulge, _)| (p, bulge))
                    .collect::<Vec<_>>();
                let closed = integer(fields, 70)? & 1 != 0;
                (sample_poly(&vertices, closed, tolerance)?, closed)
            }
            "ARC" | "CIRCLE" => {
                let center = xy(fields, 10, scale)?;
                let radius = required(fields, 40)? * scale;
                let closed = kind == "CIRCLE";
                let (start, sweep) = if closed {
                    (0.0, TAU)
                } else {
                    let start = required(fields, 50)?.rem_euclid(360.0).to_radians();
                    let end = required(fields, 51)?.rem_euclid(360.0).to_radians();
                    (start, (end - start).rem_euclid(TAU))
                };
                let mut points: Vec<Point> = Vec::new();
                sample_arc(center, radius, start, sweep, tolerance, true, &mut points)?;
                if closed {
                    let first = points[0];
                    *points.last_mut().expect("complete circle") = first;
                }
                (points, closed)
            }
            "ELLIPSE" => {
                let center = xy(fields, 10, scale)?;
                let axis = xy(fields, 11, scale)?;
                let ratio = required(fields, 40)?;
                if ratio <= 0.0 || ratio > 1.0 || num(fields, 31, 0.0)? != 0.0 {
                    return Err("Tilted or invalid DXF ELLIPSE".into());
                }
                let raw_start = num(fields, 41, 0.0)?;
                let raw_end = num(fields, 42, TAU)?;
                let start = raw_start.rem_euclid(TAU);
                let closed = ((raw_end - raw_start).abs() - TAU).abs() < 1e-9;
                let sweep = if closed {
                    TAU
                } else {
                    (raw_end.rem_euclid(TAU) - start).rem_euclid(TAU)
                };
                let radius = axis.x.hypot(axis.y);
                let minor = Point::new(-axis.y * ratio, axis.x * ratio);
                let unit_x = Point::new(axis.x / radius, minor.x / radius);
                let unit_y = Point::new(axis.y / radius, minor.y / radius);
                let mut points: Vec<Point> = Vec::new();
                sample_arc(
                    Point::new(0.0, 0.0),
                    radius,
                    start,
                    sweep,
                    tolerance,
                    true,
                    &mut points,
                )?;
                for p in &mut points {
                    *p = validate_point(Point::new(
                        center.x + p.x * unit_x.x + p.y * unit_x.y,
                        center.y + p.x * unit_y.x + p.y * unit_y.y,
                    ))?;
                }
                if closed {
                    let first = points[0];
                    *points.last_mut().expect("complete ellipse") = first;
                }
                (points, closed)
            }
            "SPLINE" => {
                let Some((points, closed)) = dxf_spline(fields, scale, tolerance, &mut output)?
                else {
                    continue;
                };
                (points, closed)
            }
            "VERTEX" | "SEQEND" => return Err("DXF has an orphan POLYLINE record".into()),
            other => {
                let short = other.chars().take(40).collect::<String>();
                output.warn(format!("Unsupported DXF entity omitted: {short}"));
                continue;
            }
        };
        output.add(points, layer, group, closed, false)?;
    }
    output.finish("DXF")
}

const MAX_SPLINE_DEGREE: usize = 8;
const SPLINE_SLOTS: usize = MAX_SPLINE_DEGREE + 1;
const JOIN_EPSILON: f64 = 1.0e-6;

#[derive(Clone, Copy, Default)]
struct Homogeneous {
    x: f64,
    y: f64,
    w: f64,
}
impl Homogeneous {
    fn mix(self, b: Self, t: f64) -> Self {
        let s = 1.0 - t;
        Self {
            x: self.x * s + b.x * t,
            y: self.y * s + b.y * t,
            w: self.w * s + b.w * t,
        }
    }
    fn project(self) -> Result<Point, String> {
        validate_point(Point::new(self.x / self.w, self.y / self.w))
    }
}

// Group codes: Autodesk DXF reference, SPLINE.
fn dxf_spline(
    fields: Pairs<'_>,
    scale: f64,
    tol: f64,
    output: &mut Output,
) -> Result<Option<(Vec<Point>, bool)>, String> {
    let flags = integer(fields, 70)?;
    let degree = integer(fields, 71)?;
    let n_knots = integer(fields, 72)?;
    let n_controls = integer(fields, 73)?;
    if !(1..=MAX_SPLINE_DEGREE as i32).contains(&degree) || flags & !31 != 0 {
        output.warn("DXF SPLINE with unsupported flags or degree outside 1–8 was omitted");
        return Ok(None);
    }
    let d = degree as usize;
    let mut raw = Vec::<(f64, Option<f64>, Option<f64>)>::new();
    let mut knots = Vec::new();
    let mut weights = Vec::new();
    for &(code, value) in fields {
        match code {
            10 => raw.push((number(value)? * scale, None, None)),
            20 | 30 => {
                let c = raw.last_mut().ok_or("DXF SPLINE coordinate is missing X")?;
                let slot = if code == 20 { &mut c.1 } else { &mut c.2 };
                if slot.replace(number(value)? * scale).is_some() {
                    return Err("DXF SPLINE has duplicate coordinates for a control point".into());
                }
            }
            40 => knots.push(number(value)?),
            41 => weights.push(number(value)?),
            _ => {}
        }
    }
    if raw.is_empty() && integer(fields, 74)? > 0 && n_controls == 0 {
        output.warn("Fit-point-only DXF SPLINE was omitted; export control points and knots");
        return Ok(None);
    }
    let n = raw.len();
    if usize::try_from(n_controls).ok() != Some(n)
        || usize::try_from(n_knots).ok() != Some(knots.len())
        || n <= d
        || knots.len() != n + d + 1
        || n > 50_000
    {
        return Err("DXF SPLINE has inconsistent control point or knot counts".into());
    }
    if knots.windows(2).any(|pair| pair[0] > pair[1])
        || !(knots[knots.len() - 1] - knots[0]).is_finite()
        || knots[d] >= knots[n]
    {
        return Err("DXF SPLINE knots are unordered or its parameter domain is empty".into());
    }
    let mut equal_run = 1;
    for (i, pair) in knots.windows(2).enumerate() {
        if pair[0] == pair[1] {
            equal_run += 1;
            if equal_run > d + 1 || (equal_run > d && knots[d] < knots[i] && knots[i] < knots[n]) {
                return Err("DXF SPLINE has excessive knot multiplicity or a discontinuity".into());
            }
        } else {
            equal_run = 1;
        }
    }
    if weights.is_empty() {
        weights.resize(n, 1.0);
    }
    if weights.len() != n || weights.iter().any(|&w| w <= 0.0) {
        return Err("DXF SPLINE must provide one strictly positive weight per control point, or omit the weights".into());
    }
    let w_max = weights.iter().copied().fold(0.0, f64::max);
    if weights.iter().any(|&w| w / w_max < 1.0e-12) {
        return Err("DXF SPLINE weight ratio exceeds numerical limits".into());
    }
    let mut controls = Vec::with_capacity(n);
    let z0 = raw[0].2.unwrap_or(0.0);
    if !z0.is_finite()
        || raw
            .iter()
            .any(|p| (p.2.unwrap_or(0.0) - z0).abs() > JOIN_EPSILON)
    {
        output.warn("DXF SPLINE not parallel to the XY plane was omitted");
        return Ok(None);
    }
    for ((x, y, _), w) in raw.into_iter().zip(weights) {
        let p = validate_point(Point::new(
            x,
            y.ok_or("DXF SPLINE control point is missing Y")?,
        ))?;
        let w = w / w_max;
        controls.push(Homogeneous {
            x: p.x * w,
            y: p.y * w,
            w,
        });
    }
    let mut points: Vec<Point> = Vec::new();
    for k in d..n {
        let a = knots[k];
        let b = knots[k + 1];
        if a == b {
            continue;
        }
        let mut bez = [Homogeneous::default(); SPLINE_SLOTS];
        // Polar de Boor: for each Bezier control i, evaluate the B-spline
        // blossom at (a repeated d-i times, b repeated i times). Keeping
        // weights homogeneous makes this valid for rational B-splines.
        for (i, dest) in bez[..=d].iter_mut().enumerate() {
            let mut work = [Homogeneous::default(); SPLINE_SLOTS];
            work[..=d].copy_from_slice(&controls[k - d..=k]);
            for r in 1..=d {
                let t = if r <= d - i { a } else { b };
                for j in (r..=d).rev() {
                    let u = knots[k - d + j];
                    let v = knots[k - r + j + 1];
                    let alpha = (t - u) / (v - u);
                    if !(0.0..=1.0).contains(&alpha) {
                        return Err("DXF SPLINE cannot be evaluated with these knots".into());
                    }
                    work[j] = work[j - 1].mix(work[j], alpha);
                }
            }
            *dest = work[d];
        }
        let first = bez[0].project()?;
        if let Some(&prev) = points.last() {
            if (first.x - prev.x).hypot(first.y - prev.y) > JOIN_EPSILON {
                return Err("DXF SPLINE spans do not join".into());
            }
        } else {
            points.push(first);
        }
        flatten_spline_bezier(&bez[..=d], tol, 0, &mut points)?;
    }
    // Don't invent a closing segment for a malformed 'closed' entity.
    let closed = flags & 3 != 0;
    if closed {
        let first = points[0];
        let last = points.last_mut().expect("nonempty knot domain");
        if (first.x - last.x).hypot(first.y - last.y) > JOIN_EPSILON {
            return Err(
                "DXF SPLINE is flagged closed or periodic, but its endpoints do not meet".into(),
            );
        }
        *last = first;
    }
    Ok(Some((points, closed)))
}

fn flatten_spline_bezier(
    controls: &[Homogeneous],
    tol: f64,
    depth: usize,
    points: &mut Vec<Point>,
) -> Result<(), String> {
    if points.len() >= MAX_POINTS {
        return Err("DXF SPLINE exceeds the plot complexity limit".into());
    }
    let n = controls.len();
    let a = controls[0].project()?;
    let b = controls[n - 1].project()?;
    let mut flat = true;
    // Positive weights confine the rational Bezier to the projected
    // control polygon's convex hull. Check distances to the chord segment
    // (not its infinite line), also catching collinear backtracking.
    for c in &controls[1..n - 1] {
        if line_distance(c.project()?, a, b) > tol {
            flat = false;
            break;
        }
    }
    if flat {
        points.push(b);
        return Ok(());
    }
    if depth >= 28 {
        return Err("DXF SPLINE exceeds the subdivision depth limit".into());
    }
    let mut work = [Homogeneous::default(); SPLINE_SLOTS];
    let mut left = work;
    let mut right = work;
    work[..n].copy_from_slice(controls);
    left[0] = work[0];
    right[n - 1] = work[n - 1];
    for r in 1..n {
        for j in 0..n - r {
            work[j] = work[j].mix(work[j + 1], 0.5);
        }
        left[r] = work[0];
        right[n - r - 1] = work[n - r - 1];
    }
    flatten_spline_bezier(&left[..n], tol, depth + 1, points)?;
    flatten_spline_bezier(&right[..n], tol, depth + 1, points)
}

fn sample_poly(vertices: &[(Point, f64)], closed: bool, tol: f64) -> Result<Vec<Point>, String> {
    let mut points: Vec<Point> = Vec::new();
    let Some(&(first, _)) = vertices.first() else {
        return Ok(points);
    };
    points.push(validate_point(first)?);
    let segments = if closed && vertices.len() > 1 {
        vertices.len()
    } else {
        vertices.len() - 1
    };
    for i in 0..segments {
        let (a, bulge) = vertices[i];
        let b = validate_point(vertices[(i + 1) % vertices.len()].0)?;
        if points.len() >= MAX_POINTS {
            return Err("DXF polyline exceeds geometry limit".into());
        }
        if bulge.abs() < 1e-12 || near(a, b) {
            points.push(b);
            continue;
        }
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let chord = dx.hypot(dy);
        let sweep = 4.0 * bulge.atan();
        if chord * bulge.abs() * 0.5 <= tol && bulge.abs() <= 1.0 {
            points.push(b);
            continue;
        }
        let shift = (1.0 / bulge - bulge) * 0.25;
        let center = Point::new(
            (a.x + b.x) * 0.5 - dy * shift,
            (a.y + b.y) * 0.5 + dx * shift,
        );
        let radius = (a.x - center.x).hypot(a.y - center.y);
        sample_arc(
            center,
            radius,
            (a.y - center.y).atan2(a.x - center.x),
            sweep,
            tol,
            false,
            &mut points,
        )?;
        *points.last_mut().expect("first vertex is present") = b;
    }
    Ok(points)
}

fn sample_arc(
    center: Point,
    radius: f64,
    start: f64,
    sweep: f64,
    tol: f64,
    include_first: bool,
    points: &mut Vec<Point>,
) -> Result<(), String> {
    if !radius.is_finite() || radius <= 0.0 || radius > 10.0 * MAX_COORD {
        return Err("DXF arc radius is out of range".into());
    }
    let angle = 4.0 * ((tol / (2.0 * radius)).min(1.0)).sqrt().asin();
    // Pick a mesh that satisfies the sagitta bound and includes cardinal arc
    // extrema, so bounds checking cannot miss an overshoot between samples.
    let mut cuts = vec![0.0, 1.0];
    for k in -8..=8 {
        let u = (f64::from(k) * PI / 2.0 - start) / sweep;
        if u > 0.0 && u < 1.0 {
            cuts.push(u);
        }
    }
    cuts.sort_by(f64::total_cmp);
    if include_first {
        points.push(validate_point(Point::new(
            center.x + radius * start.cos(),
            center.y + radius * start.sin(),
        ))?);
    }
    for cuts in cuts.windows(2) {
        let delta = cuts[1] - cuts[0];
        let n = ((delta * sweep.abs() / angle.min(PI / 4.0)).ceil() as usize).max(1);
        if n > MAX_POINTS.saturating_sub(points.len()) {
            return Err("DXF arc exceeds geometry limit".into());
        }
        for q in 1..=n {
            let t = start + sweep * (cuts[0] + delta * (q as f64) / (n as f64));
            points.push(validate_point(Point::new(
                center.x + radius * t.cos(),
                center.y + radius * t.sin(),
            ))?);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: Point, expected: (f64, f64)) {
        assert!((actual.x - expected.0).abs() < 0.0001, "x={}", actual.x);
        assert!((actual.y - expected.1).abs() < 0.0001, "y={}", actual.y);
    }
    fn dxf_file(entities: &str, units: usize) -> String {
        format!(
            "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n{units}\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n{entities}0\nENDSEC\n0\nEOF\n"
        )
    }

    #[test]
    fn svg_transforms_units_paint_and_separate_subpaths() {
        let data = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20mm" height="10mm" viewBox="0 0 20 10">
          <style>.blue {fill:none; stroke:navy}</style>
          <g transform="translate(2 1)">
            <path class="blue" d="M1,1 Q3,8 5,1 M8 2 11 4"/>
          </g>
        </svg>"#;
        let result = svg(data, 0.05).expect("valid SVG");
        assert_eq!(result.strokes.len(), 2);
        assert!(result.strokes[0].points.len() > 4);
        assert!(
            result
                .strokes
                .iter()
                .all(|s| s.layer == "#000080" && !s.closed && !s.filled)
        );
        close(result.strokes[0].points[0], (3.0, 8.0));
        close(result.strokes[1].points[0], (10.0, 7.0));
        close(result.strokes[1].points[1], (13.0, 5.0));
    }

    #[test]
    fn svg_visible_clone_evenodd_hole_and_clip_warning() {
        let data = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10mm" height="10mm" viewBox="0 0 10 10">
          <defs>
            <path id="ring" fill-rule="evenodd" fill="#ea1234" d="M1,1 H9 V9 H1 Z M2,2 H8 V8 H2 Z"/>
            <clipPath id="clip"><rect width="1" height="1"/></clipPath>
          </defs>
          <g id="rendered"><use href="#ring"/></g>
          <rect x="3" y="3" width="1" height="1" visibility="hidden"/>
          <g clip-path="url(#clip)"><rect x="0" y="0" width="4" height="4"/></g>
        </svg>"##;
        let result = svg(data, 0.05).expect("valid SVG");
        assert_eq!(result.strokes.len(), 2);
        assert_eq!(result.strokes[0].group, result.strokes[1].group);
        assert!(
            result
                .strokes
                .iter()
                .all(|s| s.closed && s.filled && s.layer == "#ea1234")
        );
        assert!(result.warnings.iter().any(|w| w.contains("clip")));
    }

    #[test]
    fn svg_refuses_dtd_recursive_clones_and_zero_tolerance() {
        assert!(svg(br#"<!DOCTYPE svg [ <!ENTITY label "x"> ]><svg xmlns="http://www.w3.org/2000/svg"/>"#, 0.1).is_err());
        assert!(svg(br##"<svg xmlns="http://www.w3.org/2000/svg"><g id="loop"><use href="#loop"/></g></svg>"##, 0.1).is_err());
        assert!(svg(&[], 0.0).is_err());
    }

    #[test]
    fn dxf_converts_inches_preserves_layer_and_omits_unsupported_entity() {
        let data = dxf_file(
            "0\nLINE\n8\noutline\n10\n1\n20\n2\n11\n3\n21\n4\n0\nINSERT\n8\noutline\n",
            1,
        );
        let result = dxf(data.as_bytes(), 0.1).expect("valid DXF");
        assert_eq!(result.strokes.len(), 1);
        assert_eq!(result.strokes[0].layer, "outline");
        close(result.strokes[0].points[1], (76.2, 101.6));
        assert!(result.warnings.iter().any(|w| w.contains("INSERT")));
    }

    #[test]
    fn dxf_bulges_and_circle_include_extrema() {
        let data = dxf_file(
            "0\nLWPOLYLINE\n90\n2\n70\n0\n10\n0\n20\n0\n42\n1\n10\n10\n20\n0\n0\nCIRCLE\n10\n20\n20\n12\n40\n3\n",
            4,
        );
        let result = dxf(data.as_bytes(), 0.1).expect("valid DXF");
        assert!(
            result.strokes[0]
                .points
                .iter()
                .any(|p| (p.y + 5.0).abs() < 1e-8)
        );
        let circle = &result.strokes[1];
        assert!(circle.closed);
        close(circle.points[0], (23.0, 12.0));
        close(*circle.points.last().expect("closed point"), (23.0, 12.0));
    }

    #[test]
    fn dxf_arc_wraps_without_turning_an_ellipse_arc_into_a_full_ellipse() {
        let data = dxf_file(
            "0\nARC\n10\n0\n20\n0\n40\n10\n50\n350\n51\n10\n0\nELLIPSE\n10\n2\n20\n0\n11\n0\n21\n4\n40\n0.5\n41\n0\n42\n1.5707963267948966\n",
            4,
        );
        let result = dxf(data.as_bytes(), 0.01).expect("valid DXF");
        let arc = &result.strokes[0];
        assert!(arc.points.iter().all(|p| p.x > 9.8));
        let ellipse = &result.strokes[1];
        assert!(!ellipse.closed);
        close(ellipse.points[0], (2.0, 4.0));
        close(*ellipse.points.last().expect("endpoint"), (0.0, 0.0));
    }

    #[test]
    fn dxf_rejects_malformed_and_nonfinite_geometry() {
        let malformed = dxf_file("0\nLWPOLYLINE\n90\n2\n10\n0\n20\n0\n10\n1\n", 4);
        assert!(dxf(malformed.as_bytes(), 0.1).is_err());
        let missing_end = dxf_file("0\nPOLYLINE\n0\nVERTEX\n10\n0\n20\n0\n", 4);
        assert!(dxf(missing_end.as_bytes(), 0.1).is_err());
        let nonfinite = dxf_file("0\nLINE\n10\nNaN\n20\n0\n11\n2\n21\n1\n", 4);
        assert!(dxf(nonfinite.as_bytes(), 0.1).is_err());
    }
    fn spline_entity(
        degree: usize,
        flags: usize,
        knots: &[f64],
        controls: &[(f64, f64, f64)],
        weights: &[f64],
    ) -> String {
        use std::fmt::Write as _;
        let mut data = format!(
            "0\nSPLINE\n8\ncurved\n70\n{flags}\n71\n{degree}\n72\n{}\n73\n{}\n74\n0\n",
            knots.len(),
            controls.len()
        );
        for knot in knots {
            write!(data, "40\n{knot}\n").unwrap();
        }
        for weight in weights {
            write!(data, "41\n{weight}\n").unwrap();
        }
        for &(x, y, z) in controls {
            write!(data, "10\n{x}\n20\n{y}\n30\n{z}\n").unwrap();
        }
        data
    }

    #[test]
    fn dxf_spline_rational_quarter_circle_respects_chord_error() {
        let data = spline_entity(
            2,
            12,
            &[0., 0., 0., 1., 1., 1.],
            &[(10., 0., 0.), (10., 10., 0.), (0., 10., 0.)],
            &[1., std::f64::consts::FRAC_1_SQRT_2, 1.],
        );
        let art = dxf(dxf_file(&data, 4).as_bytes(), 0.03).expect("rational spline");
        let stroke = &art.strokes[0];
        close(stroke.points[0], (10., 0.));
        close(*stroke.points.last().unwrap(), (0., 10.));
        for pts in stroke.points.windows(2) {
            assert!((pts[0].x.hypot(pts[0].y) - 10.0).abs() < 1.0e-8);
            let mid = midpoint(pts[0], pts[1]);
            assert!((mid.x.hypot(mid.y) - 10.0).abs() <= 0.03);
        }
    }

    #[test]
    fn dxf_spline_bezier_extraction_joins_nonuniform_polynomial_spans() {
        // Degree two, with knot 0.25. de Boor yields (2.5,7.5) at
        // the shared point, not the control point (0,10) or (10,0).
        let data = spline_entity(
            2,
            8,
            &[0., 0., 0., 0.25, 1., 1., 1.],
            &[(0., 0., 0.), (0., 10., 0.), (10., 0., 0.), (10., 10., 0.)],
            &[],
        );
        let art = dxf(dxf_file(&data, 4).as_bytes(), 0.1).expect("polynomial spline");
        let pts = &art.strokes[0].points;
        close(pts[0], (0., 0.));
        close(*pts.last().unwrap(), (10., 10.));
        assert_eq!(
            pts.iter()
                .filter(|p| (p.x - 2.5).abs() < 1e-9 && (p.y - 7.5).abs() < 1e-9)
                .count(),
            1
        );
        assert!(pts.iter().all(|p| !near(*p, Point::new(0., 10.))));
        assert!(pts.windows(2).all(|pair| !near(pair[0], pair[1])));
    }

    #[test]
    fn dxf_spline_rejects_discontinuity_bad_knots_bad_weights_and_false_closure() {
        let controls = [(0., 0., 0.), (1., 3., 0.), (4., 0., 0.), (8., 0., 0.)];
        let cases = [
            (1, 8, vec![0., 0., 0.5, 0.5, 1., 1.], vec![]),
            (2, 8, vec![0., 0., 0., 1.1, 1., 1., 1.], vec![]),
            (
                2,
                12,
                vec![0., 0., 0., 0.5, 1., 1., 1.],
                vec![1., 1., -2., 1.],
            ),
            (2, 9, vec![0., 0., 0., 0.5, 1., 1., 1.], vec![]),
        ];
        for (degree, flags, knots, weights) in cases {
            let data = spline_entity(degree, flags, &knots, &controls, &weights);
            assert!(
                dxf(dxf_file(&data, 4).as_bytes(), 0.1).is_err(),
                "accepted {data:?}"
            );
        }
    }

    #[test]
    fn dxf_spline_skips_fit_only_and_spatial_curves_with_warnings() {
        let tilted = spline_entity(
            2,
            0,
            &[0., 0., 0., 1., 1., 1.],
            &[(0., 0., 0.), (1., 3., 1.), (4., 0., 0.)],
            &[],
        );
        let data = format!(
            "0\nLINE\n10\n0\n20\n0\n11\n2\n21\n1\n0\nSPLINE\n70\n8\n71\n3\n72\n0\n73\n0\n74\n2\n11\n0\n21\n0\n11\n1\n21\n2\n{tilted}"
        );
        let art = dxf(dxf_file(&data, 4).as_bytes(), 0.1).expect("supported line remains");
        assert_eq!(art.strokes.len(), 1);
        assert!(art.warnings.iter().any(|w| w.contains("Fit-point-only")));
        assert!(art.warnings.iter().any(|w| w.contains("XY plane")));
    }
}
