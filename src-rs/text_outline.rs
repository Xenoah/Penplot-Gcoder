//! Offline OpenType font discovery and flattened outline generation.
use crate::{Artwork, MAX_POINTS, MAX_STROKES, Point, Stroke};
use rustybuzz::ttf_parser::{self, Face, GlyphId, OutlineBuilder, Tag, name_id};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    ops::Range,
    path::{Path, PathBuf},
};
use unicode_bidi::{BidiClass, BidiInfo, bidi_class};
use unicode_script::{Script, UnicodeScript};

pub const MAX_FONT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TEXT_CHARS: usize = 2_000;
pub const MAX_TEXT_LINES: usize = 100;
const MAX_FACES_PER_FONT: u32 = 256;
const MAX_CATALOGUE_FILES: usize = 4_096;
const MAX_CANVAS_MM: f64 = 1.0e9;
const MAX_FONT_NAME_BYTES: usize = 256;

/// A local file and its specific face. Paths are not portable project identifiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFace {
    pub path: PathBuf,
    pub face_index: u32,
    pub family: String,
    pub style: String,
    pub postscript_name: Option<String>,
}

impl FontFace {
    pub fn label(&self) -> String {
        format!("{} — {}", self.family, self.style)
    }
}

/// A reference safe to save with the resolved outline. Contains no font bytes or paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FontIdentity {
    pub face_index: u32,
    pub family: String,
    pub style: String,
    pub postscript_name: Option<String>,
    pub sha256: String,
}

fn read_font(path: &Path) -> Result<Vec<u8>, String> {
    let path_meta = fs::metadata(path).map_err(|e| format!("Could not inspect font: {e}"))?;
    if !path_meta.is_file() || path_meta.len() > MAX_FONT_BYTES as u64 {
        return Err("Expected a regular font file no larger than 64 MiB.".into());
    }
    let file = File::open(path).map_err(|e| format!("Could not open font: {e}"))?;
    let meta = file
        .metadata()
        .map_err(|e| format!("Could not inspect font: {e}"))?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES as u64 {
        return Err("Expected a regular font file no larger than 64 MiB.".into());
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FONT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Could not read font: {e}"))?;
    if bytes.is_empty() || bytes.len() > MAX_FONT_BYTES {
        return Err("Font is empty or exceeds 64 MiB.".into());
    }
    Ok(bytes)
}

fn name(face: &Face<'_>, ids: &[u16]) -> Option<String> {
    for &id in ids {
        let mut local = None;
        for record in face
            .names()
            .into_iter()
            .take(1_024)
            .filter(|n| n.name_id == id)
        {
            if record.name.len() > MAX_FONT_NAME_BYTES * 2 {
                continue;
            }
            let Some(value) = record.to_string() else {
                continue;
            };
            let value = value.trim();
            if value.is_empty()
                || value.len() > MAX_FONT_NAME_BYTES
                || value.chars().any(char::is_control)
            {
                continue;
            }
            // Prefer a stable English family when present, but allow Japanese and other local names.
            if record.language() == ttf_parser::Language::English_UnitedStates {
                return Some(value.to_owned());
            }
            if local.is_none() {
                local = Some(value.to_owned());
            }
        }
        if local.is_some() {
            return local;
        }
    }
    None
}

fn label_for(path: &Path, i: u32, face: &Face<'_>) -> FontFace {
    let fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| s.len() <= MAX_FONT_NAME_BYTES && !s.chars().any(char::is_control))
        .unwrap_or("Unnamed font");
    FontFace {
        path: path.to_path_buf(),
        face_index: i,
        family: name(face, &[name_id::TYPOGRAPHIC_FAMILY, name_id::FAMILY])
            .unwrap_or_else(|| fallback.to_owned()),
        style: name(face, &[name_id::TYPOGRAPHIC_SUBFAMILY, name_id::SUBFAMILY]).unwrap_or_else(
            || {
                if face.is_bold() {
                    "Bold"
                } else if face.is_italic() {
                    "Italic"
                } else {
                    "Regular"
                }
                .to_owned()
            },
        ),
        postscript_name: name(face, &[name_id::POST_SCRIPT_NAME]),
    }
}

/// Read every parseable face of a TrueType/OpenType file or collection.
pub fn list_font_faces(path: &Path) -> Result<Vec<FontFace>, String> {
    let bytes = read_font(path)?;
    let count = ttf_parser::fonts_in_collection(&bytes).unwrap_or(1);
    if !(1..=MAX_FACES_PER_FONT).contains(&count) {
        return Err("Font collection is empty or exceeds 256 faces.".into());
    }
    let faces: Vec<_> = (0..count)
        .filter_map(|i| {
            Face::parse(&bytes, i)
                .ok()
                .map(|face| label_for(path, i, &face))
        })
        .collect();
    if faces.is_empty() {
        return Err(
            "Font has no readable TrueType/OpenType face. Select a TTF, OTF, TTC or OTC file."
                .into(),
        );
    }
    Ok(faces)
}

fn supported_path(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|kind| ext.eq_ignore_ascii_case(kind))
        })
}

fn collect_paths(path: &Path, depth: usize, seen: &mut usize, files: &mut Vec<PathBuf>) {
    if depth > 12 || *seen >= MAX_CATALOGUE_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten().take(MAX_CATALOGUE_FILES) {
        if *seen >= MAX_CATALOGUE_FILES {
            break;
        }
        *seen += 1;
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            collect_paths(&path, depth + 1, seen, files);
        } else if (kind.is_file() || kind.is_symlink()) && supported_path(&path) {
            files.push(path);
        }
    }
}

#[cfg(target_os = "windows")]
fn font_roots() -> Vec<PathBuf> {
    let mut dirs = vec![
        std::env::var_os("WINDIR")
            .or_else(|| std::env::var_os("SystemRoot"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join("Fonts"),
    ];
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        dirs.push(local.join(r"Microsoft\Windows\Fonts"));
    }
    dirs
}

// Direct paths from the machine/user font registry complement the standard directories.
// Never install fonts or enumerate a network share automatically.
#[cfg(target_os = "windows")]
fn extra_font_paths() -> Vec<PathBuf> {
    use std::{
        ffi::OsString,
        os::windows::ffi::OsStringExt,
        path::{Component, Prefix},
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS},
        Storage::FileSystem::GetDriveTypeW,
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey,
            RegEnumValueW, RegOpenKeyExW,
        },
    };

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            // SAFETY: RegOpenKeyExW returned a uniquely owned handle.
            unsafe { RegCloseKey(self.0) };
        }
    }

    let subkey: Vec<_> = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts"
        .encode_utf16()
        .chain([0])
        .collect();
    let mut result = Vec::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let mut key = null_mut();
        // SAFETY: subkey is NUL-terminated, key is a writable output and hives are predefined handles.
        if unsafe { RegOpenKeyExW(hive, subkey.as_ptr(), 0, KEY_READ, &mut key) } != ERROR_SUCCESS {
            continue;
        }
        let key = Key(key);
        let mut name = vec![0u16; 16_384];
        let mut data = vec![0u16; 32_768];
        for index in 0..MAX_CATALOGUE_FILES as u32 {
            let mut name_units = name.len() as u32;
            let mut data_bytes = (data.len() * 2) as u32;
            let mut kind = 0;
            // SAFETY: both buffers are writable at their declared lengths, the lengths and
            // type outputs live for this call, reserved is null, and key is still open.
            let status = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut name_units,
                    null(),
                    &mut kind,
                    data.as_mut_ptr().cast::<u8>(),
                    &mut data_bytes,
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS || kind != REG_SZ || !data_bytes.is_multiple_of(2) {
                continue;
            }
            let Some(raw) = data.get(..(data_bytes as usize / 2)) else {
                continue;
            };
            let raw = raw.split(|&ch| ch == 0).next().unwrap_or_default();
            let path = PathBuf::from(OsString::from_wide(raw));
            if !path.is_absolute() || !supported_path(&path) {
                continue;
            }
            let Some(Component::Prefix(prefix)) = path.components().next() else {
                continue;
            };
            let drive = match prefix.kind() {
                Prefix::Disk(v) | Prefix::VerbatimDisk(v) => v,
                _ => continue,
            };
            let root = [u16::from(drive), u16::from(b':'), u16::from(b'\\'), 0];
            // SAFETY: root is a NUL-terminated UTF-16 drive root; 3 is DRIVE_FIXED.
            if unsafe { GetDriveTypeW(root.as_ptr()) } == 3 {
                result.push(path);
            }
        }
    }
    result
}

#[cfg(not(target_os = "windows"))]
fn font_roots() -> Vec<PathBuf> {
    let mut dirs = [
        "/usr/share/fonts",
        "/usr/local/share/fonts",
        "/System/Library/Fonts",
        "/Library/Fonts",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect::<Vec<_>>();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.extend([home.join(".fonts"), home.join("Library/Fonts")]);
        dirs.push(
            std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share"))
                .join("fonts"),
        );
    }
    dirs
}

/// Scan standard system and per-user locations. Run off the UI thread.
/// Nonstandard or unsupported files can still be inspected using `list_font_faces`.
pub fn list_installed_fonts() -> Vec<FontFace> {
    let mut files = Vec::new();
    let mut seen = 0;
    for root in font_roots() {
        collect_paths(&root, 0, &mut seen, &mut files);
    }
    #[cfg(target_os = "windows")]
    files.extend(extra_font_paths());
    // Font directories occasionally alias the same file.
    let mut canonical = BTreeSet::new();
    files.sort();
    let mut faces = Vec::new();
    for path in files {
        if canonical.insert(path.canonicalize().unwrap_or_else(|_| path.clone()))
            && let Ok(file_faces) = list_font_faces(&path)
        {
            faces.extend(file_faces);
        }
    }
    faces.sort_by_cached_key(|face| {
        (
            face.family.to_lowercase(),
            face.style.to_lowercase(),
            face.path.clone(),
            face.face_index,
        )
    });
    faces
}

fn identity_from_bytes(bytes: &[u8], font: &FontFace) -> Result<FontIdentity, String> {
    let face = Face::parse(bytes, font.face_index)
        .map_err(|e| format!("Could not read selected font face: {e:?}"))?;
    let named = label_for(&font.path, font.face_index, &face);
    Ok(FontIdentity {
        face_index: font.face_index,
        family: named.family,
        style: named.style,
        postscript_name: named.postscript_name,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    })
}

/// Read a portable, exact file + collection-face identity.
pub fn font_identity(font: &FontFace) -> Result<FontIdentity, String> {
    identity_from_bytes(&read_font(&font.path)?, font)
}

fn mid(a: Point, b: Point) -> Point {
    Point::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let s = if len2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    p.distance(Point::new(a.x + dx * s, a.y + dy * s))
}

struct Builder<'a> {
    strokes: &'a mut Vec<Stroke>,
    points: &'a mut usize,
    ring: Vec<Point>,
    group: u32,
    scale: f64,
    x: f64,
    y: f64,
    tolerance: f64,
    failed: bool,
}

impl Builder<'_> {
    fn project(&self, x: f32, y: f32) -> Point {
        Point::new(
            f64::from(x) * self.scale + self.x,
            f64::from(y) * self.scale + self.y,
        )
    }
    fn push(&mut self, p: Point) {
        if self.failed {
            return;
        }
        if self.ring.last().is_some_and(|last| last.distance(p) < 1e-9) {
            return;
        }
        if *self.points >= MAX_POINTS || !(p.x.abs() <= MAX_CANVAS_MM && p.y.abs() <= MAX_CANVAS_MM)
        {
            self.failed = true;
            return;
        }
        *self.points += 1;
        self.ring.push(p);
    }
    fn curve(&mut self, [a, b, c, d]: [Point; 4], depth: u8) {
        if self.failed {
            return;
        }
        if segment_distance(b, a, d).max(segment_distance(c, a, d)) <= self.tolerance {
            self.push(d);
            return;
        }
        if depth >= 24 {
            self.failed = true;
            return;
        }
        let ab = mid(a, b);
        let bc = mid(b, c);
        let cd = mid(c, d);
        let abc = mid(ab, bc);
        let bcd = mid(bc, cd);
        let m = mid(abc, bcd);
        self.curve([a, ab, abc, m], depth + 1);
        self.curve([m, bcd, cd, d], depth + 1);
    }
}

impl OutlineBuilder for Builder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        if !self.ring.is_empty() {
            self.close();
        }
        self.push(self.project(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.push(self.project(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let Some(a) = self.ring.last().copied() else {
            self.failed = true;
            return;
        };
        let q = self.project(x1, y1);
        let d = self.project(x, y);
        let p1 = Point::new(a.x + (q.x - a.x) * 2.0 / 3.0, a.y + (q.y - a.y) * 2.0 / 3.0);
        let p2 = Point::new(d.x + (q.x - d.x) * 2.0 / 3.0, d.y + (q.y - d.y) * 2.0 / 3.0);
        self.curve([a, p1, p2, d], 0);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let Some(a) = self.ring.last().copied() else {
            self.failed = true;
            return;
        };
        self.curve(
            [
                a,
                self.project(x1, y1),
                self.project(x2, y2),
                self.project(x, y),
            ],
            0,
        );
    }
    fn close(&mut self) {
        if self.ring.len() < 3 || self.failed {
            self.ring.clear();
            return;
        }
        self.push(self.ring[0]);
        if self.failed || self.strokes.len() >= MAX_STROKES {
            self.failed = true;
            return;
        }
        self.strokes.push(Stroke {
            points: std::mem::take(&mut self.ring),
            layer: "Text outlines".into(),
            group: self.group,
            closed: true,
            // Only the actual boundary is plotted. TrueType permits non-zero overlaps,
            // so even-odd hatching could cut false holes in some fonts.
            filled: false,
        });
    }
}

fn script_runs(text: &str) -> Vec<(Range<usize>, Script)> {
    let chars: Vec<_> = text.char_indices().map(|(i, c)| (i, c.script())).collect();
    let strong = |s: Script| !matches!(s, Script::Common | Script::Inherited | Script::Unknown);
    let mut out = Vec::new();
    let mut last = chars
        .iter()
        .find(|(_, s)| strong(*s))
        .map_or(Script::Common, |(_, s)| *s);
    let mut start = 0;
    for (i, s) in chars {
        if strong(s) && s != last {
            out.push((start..i, last));
            last = s;
            start = i;
        }
    }
    if start < text.len() {
        out.push((start..text.len(), last));
    }
    out
}

fn allows_no_outline(s: &str) -> bool {
    s.is_empty()
        || s.chars().all(|c| {
            c.is_whitespace()
                || matches!(
                    bidi_class(c),
                    BidiClass::BN
                        | BidiClass::LRE
                        | BidiClass::RLE
                        | BidiClass::LRO
                        | BidiClass::RLO
                        | BidiClass::PDF
                        | BidiClass::LRI
                        | BidiClass::RLI
                        | BidiClass::FSI
                        | BidiClass::PDI
                )
        })
}

fn cluster_text<'a>(text: &'a str, clusters: &[usize], cluster: u32) -> &'a str {
    let start = cluster as usize;
    if let Ok(i) = clusters.binary_search(&start) {
        let end = clusters.get(i + 1).copied().unwrap_or(text.len());
        text.get(start..end).unwrap_or("")
    } else {
        ""
    }
}

fn missing_description(s: &str) -> String {
    let chars = s
        .chars()
        .take(8)
        .map(|c| format!("U+{:04X}", u32::from(c)))
        .collect::<Vec<_>>()
        .join(" ");
    if chars.is_empty() {
        "unknown character".into()
    } else {
        chars
    }
}

fn outline_from_bytes(
    bytes: &[u8],
    face_index: u32,
    text: &str,
    em_mm: f64,
    tolerance: f64,
) -> Result<Artwork, String> {
    if !em_mm.is_finite() || !(0.1..=500.0).contains(&em_mm) {
        return Err("Text size must be 0.1 to 500 mm.".into());
    }
    if !tolerance.is_finite() || !(0.001..=1.0).contains(&tolerance) {
        return Err("Text outline tolerance must be 0.001 to 1 mm.".into());
    }
    if text.len() > MAX_TEXT_CHARS * 4 || text.chars().count() > MAX_TEXT_CHARS {
        return Err("Text exceeds 2,000 Unicode characters.".into());
    }
    let normalized = text
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}'], "\n")
        .replace('\t', "    ");
    if normalized.trim().is_empty() {
        return Err("Enter text to generate outlines.".into());
    }
    if normalized.chars().any(|c| c.is_control() && c != '\n') {
        return Err("Text contains unsupported control characters.".into());
    }
    if normalized.lines().count() > MAX_TEXT_LINES {
        return Err("Text exceeds 100 lines.".into());
    }
    let face = Face::parse(bytes, face_index)
        .map_err(|e| format!("Could not read selected font face: {e:?}"))?;
    if face.tables().glyf.is_none() && face.tables().cff.is_none() && face.tables().cff2.is_none() {
        return Err("This face has no usable vector outlines. Bitmap-only or color-only fonts cannot be plotted.".into());
    }
    let shaper = rustybuzz::Face::from_slice(bytes, face_index)
        .ok_or("Could not shape selected font face.")?;
    let font_mm = em_mm / f64::from(face.units_per_em());
    let line_height = (f64::from(face.ascender()) - f64::from(face.descender())
        + f64::from(face.line_gap()))
        * font_mm;
    let line_height = if line_height > 0.0 {
        line_height
    } else {
        1.2 * em_mm
    };

    let mut artwork = Artwork::default();
    let mut points = 0;
    let mut group = 0u32;
    for (row, line) in normalized.split('\n').enumerate() {
        let bidi = BidiInfo::new(line, None);
        let mut pen_x = 0.0;
        let pen_y = -((row as f64) * line_height);
        for para in &bidi.paragraphs {
            let (levels, runs) = bidi.visual_runs(para, para.range.clone());
            for run in runs {
                let rtl = levels[run.start].is_rtl();
                let source = &line[run];
                let mut fragments = script_runs(source);
                if rtl {
                    fragments.reverse();
                }
                for (span, script) in fragments {
                    let fragment = &source[span];
                    let mut buffer = rustybuzz::UnicodeBuffer::new();
                    buffer.push_str(fragment);
                    buffer.set_direction(if rtl {
                        rustybuzz::Direction::RightToLeft
                    } else {
                        rustybuzz::Direction::LeftToRight
                    });
                    if let Ok(tag) = <&[u8; 4]>::try_from(script.short_name().as_bytes())
                        && let Some(script) =
                            rustybuzz::Script::from_iso15924_tag(Tag::from_bytes(tag))
                    {
                        buffer.set_script(script);
                    }
                    buffer.set_flags(rustybuzz::BufferFlags::REMOVE_DEFAULT_IGNORABLES);
                    buffer.guess_segment_properties();
                    let shaped = rustybuzz::shape(&shaper, &[], buffer);
                    let mut clusters: Vec<_> = shaped
                        .glyph_infos()
                        .iter()
                        .map(|g| g.cluster as usize)
                        .collect();
                    clusters.sort_unstable();
                    clusters.dedup();
                    for (glyph, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                        let cluster = cluster_text(fragment, &clusters, glyph.cluster);
                        let gid = u16::try_from(glyph.glyph_id)
                            .map(GlyphId)
                            .map_err(|_| "Invalid shaped glyph identifier.")?;
                        if gid.0 == 0 {
                            return Err(format!(
                                "Selected font has no glyph for {}. Choose a different font.",
                                missing_description(cluster)
                            ));
                        }
                        let mut builder = Builder {
                            strokes: &mut artwork.strokes,
                            points: &mut points,
                            ring: Vec::new(),
                            group,
                            scale: font_mm,
                            x: pen_x + f64::from(pos.x_offset) * font_mm,
                            y: pen_y + f64::from(pos.y_offset) * font_mm,
                            tolerance,
                            failed: false,
                        };
                        let bbox = face.outline_glyph(gid, &mut builder);
                        if !builder.ring.is_empty() {
                            builder.close();
                        }
                        if builder.failed {
                            return Err(
                                "Text outlines exceed the geometry complexity limit.".into()
                            );
                        }
                        if bbox.is_none() && !allows_no_outline(cluster) {
                            return Err(format!(
                                "No plottable vector outline for {}. Choose a different font; bitmap-only and color-only glyphs are not plotted.",
                                missing_description(cluster)
                            ));
                        }
                        group = group.checked_add(1).ok_or("Too many shaped glyphs.")?;
                        if group > MAX_STROKES as u32 {
                            return Err("Text shaping exceeds the glyph count limit.".into());
                        }
                        pen_x += f64::from(pos.x_advance) * font_mm;
                    }
                }
            }
        }
    }
    let Some(min_x) = artwork
        .strokes
        .iter()
        .flat_map(|s| s.points.iter().map(|p| p.x))
        .reduce(f64::min)
    else {
        return Err("Text has no plottable outlines.".into());
    };
    let min_y = artwork
        .strokes
        .iter()
        .flat_map(|s| s.points.iter().map(|p| p.y))
        .fold(f64::INFINITY, f64::min);
    for p in artwork.strokes.iter_mut().flat_map(|s| &mut s.points) {
        p.x -= min_x;
        p.y -= min_y;
    }
    crate::check_size(&artwork.strokes)?;
    Ok(artwork)
}

/// Convert selected font and horizontal text to millimetre contours. Size is the font em.
/// Font variations use the file's default coordinates; no silent substitution is performed.
pub fn outline_text(
    font: &FontFace,
    text: &str,
    em_mm: f64,
    tolerance_mm: f64,
) -> Result<Artwork, String> {
    outline_from_bytes(
        &read_font(&font.path)?,
        font.face_index,
        text,
        em_mm,
        tolerance_mm,
    )
}

/// Outline and identify the exact same font bytes (single read, safe across file changes).
pub fn outline_text_with_identity(
    font: &FontFace,
    text: &str,
    em_mm: f64,
    tolerance_mm: f64,
) -> Result<(Artwork, FontIdentity), String> {
    let bytes = read_font(&font.path)?;
    let id = identity_from_bytes(&bytes, font)?;
    outline_from_bytes(&bytes, font.face_index, text, em_mm, tolerance_mm).map(|art| (art, id))
}

/// Trace the visible ink of each shaped outline glyph, including its white
/// counters. These are inferred plotting paths, not the original pen order.
fn centerline_text(mut art: Artwork, em_mm: f64) -> Result<Artwork, String> {
    let step = em_mm / 144.0;
    let mut paths = Vec::new();
    let mut dropped = 0;
    let mut vertices = 0;
    let mut pixels = 12_000_000usize;
    let mut scans = 30_000_000usize;
    for rings in art.strokes.chunk_by(|a, b| a.group == b.group) {
        let (x0, y0, x1, y1) = rings.iter().flat_map(|r| &r.points).fold(
            (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
            |(x0, y0, x1, y1), p| (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)),
        );
        let origin = Point::new(x0 - step, y0 - step);
        let width = ((x1 - x0) / step).ceil() as usize + 2;
        let height = ((y1 - y0) / step).ceil() as usize + 2;
        if width > 2048 || height > 2048 {
            return Err("A glyph is too large for centerline tracing. Use Outline.".into());
        }
        pixels = pixels.checked_sub(width * height).ok_or(
            "Text is too long for centerline tracing. Split it into shorter text objects.",
        )?;
        let edges: Vec<_> = rings
            .iter()
            .flat_map(|s| s.points.windows(2).map(|w| (w[0], w[1])))
            .filter(|(a, b)| a.y != b.y)
            .collect();
        scans = scans
            .checked_sub(edges.len() * height)
            .ok_or("Text is too detailed for centerline tracing. Split it or use Outline.")?;
        let mut mask = vec![0u8; width * height];
        let mut crossings = Vec::with_capacity(edges.len());
        for (row, cells) in mask.chunks_exact_mut(width).enumerate() {
            let y = origin.y + (height as f64 - row as f64 - 0.5) * step;
            crossings.clear();
            for &(a, b) in &edges {
                if (a.y <= y && y < b.y) || (b.y <= y && y < a.y) {
                    let x = a.x + (b.x - a.x) * (y - a.y) / (b.y - a.y);
                    crossings.push((x, if b.y > a.y { 1 } else { -1 }));
                }
            }
            crossings.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            let mut cross = crossings.iter().peekable();
            let mut winding = 0;
            for (column, pixel) in cells.iter_mut().enumerate() {
                let x = origin.x + (column as f64 + 0.5) * step;
                while cross.peek().is_some_and(|(edge_x, _)| *edge_x <= x) {
                    winding += cross.next().expect("inspected crossing").1;
                }
                *pixel = u8::from(winding != 0);
            }
        }
        let mut result =
            crate::centerline::trace(&mask, width, height, step, origin, "Text centerlines")?;
        if result.strokes.is_empty() {
            dropped += 1;
        }
        for s in &mut result.strokes {
            s.group = rings[0].group;
            vertices += s.points.len();
        }
        paths.extend(result.strokes);
        if paths.len() > MAX_STROKES || vertices > MAX_POINTS {
            return Err("Text centerlines exceed the geometry limit.".into());
        }
    }
    if paths.is_empty() {
        return Err(
            "Text has no drawable centerlines. Use Outline for tiny or isolated marks.".into(),
        );
    }
    if dropped != 0 {
        art.warnings.push(format!(
            "{dropped} glyph(s) had no drawable centerline. Use Outline if the marks are required."
        ));
    }
    art.strokes = paths;
    crate::check_size(&art.strokes)?;
    Ok(art)
}

/// Shape text and produce only the requested trace style, reading the font once.
pub fn trace_text_with_identity(
    font: &FontFace,
    text: &str,
    em_mm: f64,
    tolerance_mm: f64,
    trace: crate::TraceMode,
) -> Result<(Artwork, FontIdentity), String> {
    let (art, id) = outline_text_with_identity(font, text, em_mm, tolerance_mm)?;
    match trace {
        crate::TraceMode::Outline => Ok((art, id)),
        crate::TraceMode::Centerline => centerline_text(art, em_mm).map(|art| (art, id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TTF: &[u8] = include_bytes!("../tests/fixtures/font/synthetic.ttf");
    static TTC: &[u8] = include_bytes!("../tests/fixtures/font/synthetic.ttc");

    fn outline(text: &str, size: f64) -> Artwork {
        outline_from_bytes(TTF, 0, text, size, 0.02).unwrap()
    }

    #[test]
    fn open_type_ligature_and_japanese_mixed_multiline() {
        let art = outline("fi 日本語\nO", 10.0);
        assert_eq!(art.strokes.len(), 6);
        // One contour for the fi ligature, not the three of separate f + i.
        assert_eq!(art.strokes.iter().filter(|s| s.group == 0).count(), 1);
        let second_line = &art.strokes[4..];
        assert_eq!(second_line[0].group, second_line[1].group);
        assert!(
            second_line
                .iter()
                .flat_map(|s| &s.points)
                .all(|p| p.y <= 7.0)
        );
        assert!(
            art.strokes[..4]
                .iter()
                .flat_map(|s| &s.points)
                .all(|p| p.y >= 10.0)
        );
    }

    #[test]
    fn text_outline_preserves_holes_and_scales_mm() {
        let art = outline("O", 20.0);
        assert_eq!(art.strokes.len(), 2);
        assert!(
            art.strokes
                .iter()
                .all(|s| s.points.first() == s.points.last() && s.closed && !s.filled)
        );
        let bounds = |art: &Artwork| {
            art.strokes.iter().flat_map(|s| &s.points).fold(
                (
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ),
                |(x0, y0, x1, y1), p| (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)),
            )
        };
        assert_eq!(bounds(&art), (0.0, 0.0, 10.0, 14.0));
        assert_eq!(bounds(&outline("O", 10.0)), (0.0, 0.0, 5.0, 7.0));
    }

    #[test]
    fn text_centerline_keeps_the_counter_as_one_ring() {
        let art = centerline_text(outline("O", 10.), 10.).unwrap();
        assert_eq!(art.strokes.len(), 1);
        let stroke = &art.strokes[0];
        assert!(stroke.closed && !stroke.filled);
        assert!(stroke.points.len() >= 4);
        assert!(
            stroke
                .points
                .iter()
                .all(|p| p.x > 0. && p.x < 5. && p.y > 0. && p.y < 7.)
        );
        assert_eq!(outline("O", 10.).strokes.len(), 2);
    }

    #[test]
    fn centerline_text_keeps_shaped_unicode_multiline_glyphs_in_position() {
        let art = centerline_text(outline("日本語\nO", 10.), 10.).unwrap();
        let latin: Vec<_> = art.strokes.iter().filter(|s| s.group == 3).collect();
        assert_eq!(latin.len(), 1);
        assert!(latin[0].closed);
        assert!(latin[0].points.iter().all(|p| p.y < 7.));
        assert!((0..3).all(|group| art.strokes.iter().any(|s| s.group == group)));
        assert!(
            art.strokes
                .iter()
                .filter(|s| s.group != 3)
                .all(|s| s.points.iter().all(|p| p.y > 7.))
        );
    }

    #[test]
    fn collection_face_identity_distinguishes_index_and_has_no_project_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private-name.ttc");
        fs::write(&path, TTC).unwrap();
        let faces = list_font_faces(&path).unwrap();
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[1].family, "Penplot Synthetic Alt");
        let (art, identity) = outline_text_with_identity(&faces[1], "日本語", 10., 0.1).unwrap();
        assert_eq!(art.strokes.len(), 3);
        assert_eq!(identity.face_index, 1);
        assert_eq!(identity.sha256.len(), 64);
        assert_ne!(identity, font_identity(&faces[0]).unwrap());
        assert!(
            !serde_json::to_string(&identity)
                .unwrap()
                .contains("private-name")
        );
    }

    #[test]
    fn rejects_missing_glyph_and_invalid_size_instead_of_replacing_content() {
        for (input, size) in [("🔮", 10.0), ("O", f64::NAN), ("　 ", 10.0), ("O\0", 10.0)] {
            assert!(outline_from_bytes(TTF, 0, input, size, 0.1).is_err());
        }
        let error = outline_from_bytes(TTF, 0, "🔮", 10.0, 0.1).unwrap_err();
        assert!(error.contains("U+1F52E"), "{error}");
    }

    #[test]
    fn bad_font_file_is_reported() {
        let f = tempfile::NamedTempFile::new().unwrap();
        fs::write(f.path(), b"not a font").unwrap();
        assert!(list_font_faces(f.path()).is_err());
    }
}
