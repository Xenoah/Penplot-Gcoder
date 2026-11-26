//! Split already-placed routes; never normalize or fit an individual pen pass.
use crate::{Output, Profile, Stroke, generate_with_travel};
use std::{collections::BTreeMap, path::Path};

pub fn ink_rgb(layer: &str) -> Option<[u8; 3]> {
    let style = layer.rsplit(':').next()?;
    let hex = style.strip_prefix('#').filter(|h| h.len() == 6)?;
    let code = u32::from_str_radix(hex, 16).ok()?;
    Some([(code >> 16) as u8, (code >> 8) as u8, code as u8])
}
pub struct ColorJob {
    pub color: [u8; 3],
    pub filename: String,
    pub output: Output,
}
pub fn color_jobs(
    routes: &[Stroke],
    profile: &Profile,
    air: bool,
    smooth: bool,
) -> Result<Vec<ColorJob>, String> {
    if routes.is_empty() {
        return Err("No drawable color paths.".into());
    }
    let mut colors = BTreeMap::<[u8; 3], Vec<Stroke>>::new();
    for s in routes {
        colors
            .entry(ink_rgb(&s.layer).unwrap_or([0, 0, 0]))
            .or_default()
            .push(s.clone());
    }
    if colors.len() > 32 {
        return Err("Color export is limited to 32 pens. Reduce the palette.".into());
    }
    colors
        .into_iter()
        .enumerate()
        .map(|(i, (color, strokes))| {
            let output = generate_with_travel(&strokes, profile, air, smooth)?;
            Ok(ColorJob {
                color,
                filename: format!("P{:02}.GCODE", i + 1),
                output,
            })
        })
        .collect()
}
/// Reserve a new folder and remove our incomplete output on I/O error.
/// All passes are generated and validated before any file is written.
pub fn save_color_jobs(path: &Path, jobs: &[ColorJob]) -> Result<(), String> {
    if jobs.is_empty() {
        return Err("No color passes to save.".into());
    }
    std::fs::create_dir(path)
        .map_err(|e| format!("{}: {e}. Choose a NEW folder name.", path.display()))?;
    let result = (|| {
        let mut guide = String::from(
            "Penplot-Gcoder | color passes\n\nAll files use the SAME paper coordinates and machine profile.\nKeep the paper fixed and preserve the same XY origin for every pass.\nTest an air run with the pen removed first.\nRun ONE file to completion, then replace the pen while stopped and raised.\nUse the same tip offset and measured contact/clearance Z for every pen.\nDo not reset XY, move the paper, or change Fit/Scale between passes.\nNo automatic tool-change, pause or homing command is inserted.\nUncolored geometry uses black. RGB values describe the suggested pens.\n\nOrder / file / pen RGB\n",
        );
        guide.push_str(
            if jobs.iter().all(|j| j.output.gcode.starts_with("; AIR RUN")) {
                "AIR RUN ONLY: pens remain raised.\n"
            } else {
                "PLOT: drawing moves lower the pen.\n"
            },
        );
        for (i, job) in jobs.iter().enumerate() {
            if job.filename != format!("P{:02}.GCODE", i + 1) {
                return Err("Invalid color pass filename".to_owned());
            }
            crate::save_file(
                &path.join(&job.filename),
                job.output.gcode.as_bytes(),
                false,
            )?;
            guide.push_str(&format!(
                "{} / {} / #{:02X}{:02X}{:02X}\n",
                i + 1,
                job.filename,
                job.color[0],
                job.color[1],
                job.color[2]
            ));
        }
        crate::save_file(&path.join("PENS.txt"), guide.as_bytes(), false)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(path);
    }
    result
}
