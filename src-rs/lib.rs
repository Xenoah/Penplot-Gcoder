//! Offline document import, placement, toolpath planning and checked G-code emission.
//! All internal coordinates are millimetres with positive Y pointing up.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

mod centerline;
mod color_export;
mod geometry;
mod import_raster;
mod raster_processing;
pub use color_export::{ColorJob, color_jobs, ink_rgb, save_color_jobs};
pub use raster_processing::{RasterMode, RasterProcessing};
mod import_vector;
mod machine;
mod project;
mod text_layer;
pub mod text_outline;
mod travel_curve;
pub use geometry::{
    Fill, JobOptions, PlanResult, RasterLayers, RasterSettings, fit, plan, plan_with_raster,
};
pub use machine::{
    CurvedTravel, K9StartGcode, MachinePreset, Origin, Output, Profile, generate, generate_air_run,
    generate_with_travel,
};
pub use project::{Project, ProjectLayer};
pub use text_layer::TextLayer;

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_POINTS: usize = 500_000;
pub const MAX_STROKES: usize = 100_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    pub fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    pub points: Vec<Point>,
    pub layer: String,
    /// Rings sharing a group and layer belong to one compound hatch region.
    pub group: u32,
    pub closed: bool,
    pub filled: bool,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Artwork {
    pub strokes: Vec<Stroke>,
    pub warnings: Vec<String>,
}
/// Mutually exclusive ways to derive toolpaths from raster ink or shaped text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceMode {
    #[default]
    Outline,
    Centerline,
}

impl TraceMode {
    pub fn is_outline(&self) -> bool {
        *self == Self::Outline
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportOptions {
    #[serde(default, skip_serializing_if = "RasterProcessing::is_default")]
    pub processing: RasterProcessing,
    pub threshold: u8,
    pub invert: bool,
    pub dpi: f64,
    pub tolerance: f64,
    #[serde(default, skip_serializing_if = "TraceMode::is_outline")]
    pub trace: TraceMode,
}
impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            processing: RasterProcessing::default(),
            threshold: 128,
            invert: false,
            dpi: 96.0,
            tolerance: 0.1,
            trace: TraceMode::Outline,
        }
    }
}
pub fn read_limited(path: &Path) -> Result<Vec<u8>, String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = f.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES as u64 {
        return Err("Expected a regular file of at most 16 MiB".into());
    }
    let mut buf = Vec::with_capacity(meta.len() as usize);
    f.take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() > MAX_FILE_BYTES {
        return Err("File exceeds 16 MiB".into());
    }
    Ok(buf)
}

pub fn is_raster_filename(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|ext| {
            ["png", "jpg", "jpeg"]
                .iter()
                .any(|r| r.eq_ignore_ascii_case(ext))
        })
}

pub fn import_bytes(filename: &str, bytes: &[u8], opts: &ImportOptions) -> Result<Artwork, String> {
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return Err("Empty input or file exceeds 16 MiB".into());
    }
    if !opts.tolerance.is_finite() || !(0.001..=10.).contains(&opts.tolerance) {
        return Err("Curve tolerance must be 0.001 to 10 mm".into());
    }
    if !opts.dpi.is_finite() || !(1.0..=9600.0).contains(&opts.dpi) {
        return Err("DPI must be 1 to 9600".into());
    }
    opts.processing.validate()?;
    let art = match Path::new(filename)
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "svg" => import_vector::svg(bytes, opts.tolerance),
        "dxf" => import_vector::dxf(bytes, opts.tolerance),
        "png" | "jpg" | "jpeg" => import_raster::raster(bytes, opts),
        _ => Err("Unsupported format. Choose SVG, ASCII DXF, PNG or JPEG.".into()),
    }?;
    if art.strokes.is_empty() {
        return Err(format!(
            "No plottable geometry. {}",
            art.warnings.join("; ")
        ));
    }
    check_size(&art.strokes)?;
    Ok(art)
}

pub(crate) fn check_size(strokes: &[Stroke]) -> Result<(), String> {
    if strokes.len() > MAX_STROKES
        || strokes
            .iter()
            .try_fold(0usize, |n, s| n.checked_add(s.points.len()))
            .unwrap_or(usize::MAX)
            > MAX_POINTS
    {
        return Err("Document exceeds 100,000 paths or 500,000 vertices".into());
    }
    Ok(())
}

/// Append one imported document without allowing its group IDs to collide with earlier files.
pub fn append_artwork(dest: &mut Vec<Stroke>, more: &[Stroke]) -> Result<(), String> {
    if more.is_empty() {
        return Ok(());
    }
    check_size(dest)?;
    check_size(more)?;
    if dest.len() + more.len() > MAX_STROKES
        || dest
            .iter()
            .chain(more)
            .map(|s| s.points.len())
            .sum::<usize>()
            > MAX_POINTS
    {
        return Err("Combined document exceeds geometry limit".into());
    }
    let base = dest
        .iter()
        .map(|s| u64::from(s.group) + 1)
        .max()
        .unwrap_or(0);
    let mut remap = BTreeMap::new();
    for s in more {
        let next =
            u32::try_from(base + remap.len() as u64).map_err(|_| "Too many distinct groups")?;
        remap.entry(s.group).or_insert(next);
    }
    dest.extend(more.iter().cloned().map(|mut s| {
        s.group = remap[&s.group];
        s
    }));
    Ok(())
}

/// Write a complete file beside its target before publishing it. No partially written
/// G-code becomes visible under the final SD filename when validation or I/O fails.
pub fn save_file(path: &Path, bytes: &[u8], overwrite: bool) -> Result<(), String> {
    use std::io::Write;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("{}: {e}", parent.display()))?;
    tmp.write_all(bytes)
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    if overwrite {
        tmp.persist(path)
    } else {
        tmp.persist_noclobber(path)
    }
    .map_err(|e| format!("{}: {}", path.display(), e.error))?;
    Ok(())
}

/// Combine visible files in the chosen order. The display namespace also keeps two
/// independent files using the same ink color apart when optimizing travel.
pub fn append_layer(dest: &mut Vec<Stroke>, more: &[Stroke], index: usize) -> Result<(), String> {
    let start = dest.len();
    append_artwork(dest, more)?;
    for s in &mut dest[start..] {
        s.layer = format!("{index:03}:{}", s.layer);
    }
    Ok(())
}
