//! Versioned portable document format; saved text outlines load without a font.
use crate::{
    ImportOptions, JobOptions, MAX_FILE_BYTES, MAX_POINTS, MAX_STROKES, Profile, RasterSettings,
    Stroke, is_raster_filename,
};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};

const SCHEMA: u32 = 3;
const MAX_LAYERS: usize = 128;
const MAX_NAME: usize = 512;
const MAX_COORD: f64 = 1e8;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectLayer {
    pub name: String,
    pub visible: bool,
    pub strokes: Vec<Stroke>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "raster_settings",
        alias = "image_settings",
        alias = "raster_layer",
        alias = "raster_options"
    )]
    pub raster: Option<RasterSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<crate::TextLayer>,
}

impl ProjectLayer {
    pub fn raster_settings(&self, options: &JobOptions) -> Option<RasterSettings> {
        self.raster.clone().or_else(|| {
            (is_raster_filename(&self.name)
                && !self.strokes.is_empty()
                && self.strokes.iter().all(|stroke| {
                    stroke.layer == "Raster contours" && stroke.closed && stroke.filled
                }))
            .then(|| RasterSettings::from_job(options))
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub layers: Vec<ProjectLayer>,
    pub options: JobOptions,
    pub profile: Profile,
    pub import_options: ImportOptions,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadProject {
    version: u32,
    layers: Vec<ProjectLayer>,
    options: JobOptions,
    profile: Profile,
    import_options: ImportOptions,
}

#[derive(Serialize)]
struct SaveProject<'a> {
    version: u32,
    layers: &'a [ProjectLayer],
    options: &'a JobOptions,
    profile: &'a Profile,
    import_options: &'a ImportOptions,
}

/// Bounds encoding too, instead of accumulating arbitrarily many output bytes
/// before discovering that the document could not be opened by read_limited.
struct LimitedBuffer(Vec<u8>);

impl Write for LimitedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_FILE_BYTES - self.0.len() {
            return Err(io::Error::other("Project exceeds 16 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Project {
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err("Project exceeds 16 MiB".into());
        }
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        let wire: LoadProject =
            serde_json::from_slice(bytes).map_err(|err| format!("Invalid project JSON: {err}"))?;
        if !(1..=SCHEMA).contains(&wire.version) {
            return Err(format!("Unsupported project version {}", wire.version));
        }
        let mut project = Self {
            layers: wire.layers,
            options: wire.options,
            profile: wire.profile,
            import_options: wire.import_options,
        };
        project.profile.restore_k9_editor();
        project.validate()?;
        Ok(project)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut buffer = LimitedBuffer(Vec::new());
        serde_json::to_writer(
            &mut buffer,
            &SaveProject {
                version: if self.layers.iter().any(|layer| layer.text.is_some()) {
                    SCHEMA
                } else {
                    2
                },
                layers: &self.layers,
                options: &self.options,
                profile: &self.profile,
                import_options: &self.import_options,
            },
        )
        .map_err(|err| format!("Cannot encode project: {err}"))?;
        buffer.write_all(b"\n").map_err(|err| err.to_string())?;
        Ok(buffer.0)
    }

    fn validate(&self) -> Result<(), String> {
        self.profile.validate()?;
        let options = &self.options;
        if !(0.0..=MAX_COORD).contains(&options.scale)
            || options.scale == 0.0
            || ![options.x, options.y].into_iter().all(valid_coord)
            || !options.rotation_deg.is_finite()
        {
            return Err("Invalid project scale, rotation or placement".into());
        }
        if !(0.0..=10_000.).contains(&options.min_image_stroke_mm) {
            return Err("Invalid minimum image stroke".into());
        }
        if !options.hatch_angle.is_finite() || !(0.02..=1000.0).contains(&options.hatch_spacing) {
            return Err("Invalid project hatch settings".into());
        }
        if !(1.0..=9600.0).contains(&self.import_options.dpi)
            || !(0.001..=10.0).contains(&self.import_options.tolerance)
        {
            return Err("Invalid project import DPI or tolerance".into());
        }
        if self.layers.len() > MAX_LAYERS {
            return Err("The project exceeds 128 layers".into());
        }
        let mut paths = 0;
        let mut vertices = 0;
        for (i, layer) in self.layers.iter().enumerate() {
            if !valid_name(&layer.name) {
                return Err(format!("Invalid project layer name at layer {}", i + 1));
            }
            if let Some(settings) = &layer.raster {
                settings.validate()?;
            }
            if let Some(text) = &layer.text {
                text.validate()?;
                if layer.raster.is_some() || layer.strokes.is_empty() {
                    return Err(
                        "A text layer requires saved outlines and cannot also be an image.".into(),
                    );
                }
            }
            paths += layer.strokes.len();
            if paths > MAX_STROKES {
                return Err("The project exceeds 100,000 paths".into());
            }
            for (n, stroke) in layer.strokes.iter().enumerate() {
                if stroke.points.len() < 2 {
                    return Err(format!(
                        "Layer {}, path {}: at least two points required",
                        i + 1,
                        n + 1
                    ));
                }
                if !valid_name(&stroke.layer) {
                    return Err(format!(
                        "Layer {}, path {}: invalid drawing group name",
                        i + 1,
                        n + 1
                    ));
                }
                vertices += stroke.points.len();
                if vertices > MAX_POINTS {
                    return Err("The project exceeds 500,000 vertices".into());
                }
                if !stroke
                    .points
                    .iter()
                    .all(|p| valid_coord(p.x) && valid_coord(p.y))
                {
                    return Err(format!(
                        "Layer {}, path {}: invalid coordinates",
                        i + 1,
                        n + 1
                    ));
                }
            }
        }
        Ok(())
    }
}

fn valid_coord(value: f64) -> bool {
    (-MAX_COORD..=MAX_COORD).contains(&value)
}

fn valid_name(value: &str) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= MAX_NAME
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Fill, Point};

    fn blank() -> Project {
        Project {
            layers: vec![],
            options: JobOptions::default(),
            profile: Profile::k9(),
            import_options: ImportOptions::default(),
        }
    }

    #[test]
    fn blank_and_off_bed_projects_roundtrip() {
        let mut project = blank();
        assert_eq!(
            project,
            Project::from_json(&project.to_json().unwrap()).unwrap()
        );
        project.options.x = 150.0;
        project.options.rotation_deg = 15.0;
        project.options.fill = Fill::Cross;
        project.import_options.invert = true;
        project.layers.push(ProjectLayer {
            raster: None,
            text: None,
            name: "図形".to_owned(),
            visible: false,
            strokes: vec![Stroke {
                points: vec![Point::new(-10., -3.), Point::new(101., 102.)],
                layer: "#afe000".to_owned(),
                group: 42,
                closed: false,
                filled: true,
            }],
        });
        let bytes = project.to_json().unwrap();
        assert_eq!(project, Project::from_json(&bytes).unwrap());
        assert_eq!(
            bytes,
            Project::from_json(&bytes).unwrap().to_json().unwrap()
        );
    }

    #[test]
    fn legacy_projects_default_to_straight_and_opt_in_persists() {
        let mut project = blank();
        let old = project.to_json().unwrap();
        assert!(!std::str::from_utf8(&old).unwrap().contains("smooth_pen_up"));
        assert!(!Project::from_json(&old).unwrap().options.smooth_pen_up);
        project.options.smooth_pen_up = true;
        assert!(
            Project::from_json(&project.to_json().unwrap())
                .unwrap()
                .options
                .smooth_pen_up
        );
    }

    #[test]
    fn rejects_unknown_versions_duplicates_and_any_calibration_field() {
        let bytes = blank().to_json().unwrap();
        let raw = std::str::from_utf8(&bytes).unwrap();
        for malformed in [
            raw.replacen("\"version\":2", "\"version\":7", 1),
            raw.replacen("\"version\":2", "\"version\":2,\"version\":2", 1),
            raw.replacen(
                "\"version\":2",
                "\"version\":2,\"calibration_acknowledged\":true",
                1,
            ),
            raw.replacen("\"bed_x\":100.0", "\"bed_x\":0.0", 1),
        ] {
            assert!(
                Project::from_json(malformed.as_bytes()).is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn malformed_layers_nonfinite_and_bad_controls_fail_before_writing() {
        let mut project = blank();
        project.import_options.dpi = f64::NAN;
        assert!(project.to_json().unwrap_err().contains("DPI"));
        project.import_options = ImportOptions::default();
        project.options.scale = -1.0;
        assert!(project.to_json().unwrap_err().contains("scale"));
        project.options = JobOptions::default();
        project.layers.push(ProjectLayer {
            raster: None,
            text: None,
            name: "ok".to_owned(),
            visible: true,
            strokes: vec![Stroke {
                points: vec![Point::new(0., 0.)],
                layer: "line".to_owned(),
                group: 0,
                closed: false,
                filled: false,
            }],
        });
        assert!(project.to_json().unwrap_err().contains("two points"));
        project.layers[0].strokes[0]
            .points
            .push(Point::new(f64::INFINITY, 1.0));
        assert!(project.to_json().unwrap_err().contains("coordinates"));
        project.layers[0].name = "too\nlong".to_owned();
        assert!(project.to_json().unwrap_err().contains("layer name"));
    }

    #[test]
    fn file_and_layer_limits_are_enforced() {
        assert!(
            Project::from_json(&vec![b' '; MAX_FILE_BYTES + 1])
                .unwrap_err()
                .contains("16 MiB")
        );
        let mut project = blank();
        project.layers = vec![
            ProjectLayer {
                raster: None,
                text: None,
                name: "ok".to_owned(),
                visible: true,
                strokes: vec![],
            };
            MAX_LAYERS + 1
        ];
        assert!(project.to_json().unwrap_err().contains("128 layers"));
    }
}
