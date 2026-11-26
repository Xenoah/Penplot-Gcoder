//! Thin CLI: it never opens a serial port and shares the exact desktop pipeline.
use penplot::{Fill, ImportOptions, JobOptions, MachinePreset, Profile, TraceMode};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
const HELP: &str = r#"penplot | offline pen-plotter compiler

  penplot check [OPTIONS] ART.svg [SECOND.dxf ...]
  penplot convert --profile MEASURED.json --calibrated -o DRAW.GCODE [OPTIONS] ART
  penplot convert --calibrated [--air-run] -o DRAW.GCODE SAVED.penplot
  penplot profile-init OUTPUT.json [--k9 | --p1s | --aries]
  penplot profile-migrate OLD.json OUTPUT.json

  --profile FILE       measured native JSON, or migrate a legacy printer preset
  --calibrated         confirm HOME, clearance, contact height, offset and limits
  --air-run            keep Z at clearance even on drawing moves
  --fit MM             center and fit with this margin; excludes --scale/--x/--y
  --scale RATIO        1 = original metric size
  --rotate DEG         rotate counter-clockwise before placement
  --x MM, --y MM       lower-left of rotated artwork bounds on paper
  --hatch lines|cross  outlined and hatched filled, closed shapes
  --spacing MM         hatch spacing (default 1); --angle DEG (default 45)
  --min-image-stroke MM omit raster strokes shorter than MM after placement (0 = off)
  --optimize           reduce lifts' travel within contiguous layer runs
  --smooth-pen-up      use quadratic smoothing of eligible pen-up links
  --straight-pen-up    disable smoothing (default except new K9 profiles)
  --dpi N              PNG/JPEG physical scale (default 96)
  --threshold 0..255   raster foreground brightness cutoff (default 128)
  --invert             select bright instead of dark pixels
  --trace outline|centerline   trace raster contours (default) or single strokes
  --raster threshold|clean|color   classic trace, clean line art, or color hatch
  --clean-radius PX    local background radius 2..128 (default 16)
  --ink-contrast N     local ink contrast 1..80 (default 12; lower keeps faint ink)
  --speckle PIXELS     remove connected dots <= 0..100 pixels (clean only; default 3)
  --palette N          maximum 2..8 estimated pen colors (default 4)
  --include-background retain white color regions (transparent pixels stay empty)
  --white-cutoff N     exclude pixels with all RGB channels >= 128..255 (default 245)
  --hatch-only         omit raster outlines when hatching (color always defaults to this)
  --colors-dir FOLDER  convert to a NEW folder of separate pen passes + PENS.txt
  --tolerance MM       vector curve flatness (default 0.1)
  --                   treat remaining arguments as input paths

Output never overwrites an existing file. Both modes preflight the final commands.
'check' can use the demo profile. Export requires a project or a measured profile.
Projects use their saved visible layers, placement and profile; no setting overrides.
No machine access, homing, extrusion, heating or motor disable commands.
P1S is layout-only. Aries output is experimental and requires measured Z values.
In profile JSON, pen_down_z (Draw Z) and pen_up_z (Clearance Z) are separate
absolute heights in mm from the same active/homed Z origin; offset_z adds to both.
An optional K9 k9_lift_mm overrides Clearance Z with Draw Z + lift.
liftup_delay is the wait in ms before the vertical lift.
The K9 profile pre-fills k9_start_gcode.code; set k9_start_gcode.enabled to
true only to test the experimental M203 X/Y, M201 X/Y and M204 P/T preamble.
This does not change commanded feeds or save EEPROM, and affects air runs too.
"#;
type Args = std::vec::IntoIter<OsString>;
fn value(it: &mut Args, key: &str) -> Result<OsString, String> {
    it.next().ok_or_else(|| format!("Missing value for {key}"))
}
fn num(it: &mut Args, key: &str) -> Result<f64, String> {
    value(it, key)?
        .to_str()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|x| x.is_finite())
        .ok_or_else(|| format!("Invalid number for {key}"))
}
fn load_profile(path: &Path) -> Result<Profile, String> {
    let bytes = penplot::read_limited(path)?;
    let (p, warnings) =
        Profile::from_json(std::str::from_utf8(&bytes).map_err(|_| "Expected UTF-8 JSON")?)?;
    for w in warnings {
        eprintln!("Warning: {w}");
    }
    Ok(p)
}
fn save_profile(p: &Profile, out: &Path) -> Result<(), String> {
    let mut txt = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
    txt.push('\n');
    penplot::save_file(out, txt.as_bytes(), false)?;
    println!(
        "Wrote review-only profile {}. Measure it before export.",
        out.display()
    );
    Ok(())
}
fn run(mut it: Args) -> Result<(), String> {
    let action = it.next();
    let Some(action) = action.as_deref().and_then(|v| v.to_str()) else {
        print!("{HELP}");
        return Ok(());
    };
    match action {
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            return Ok(());
        }
        "-V" | "--version" => {
            println!("penplot {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        "profile-init" => {
            let out = PathBuf::from(value(&mut it, "output profile")?);
            let selection = it.next();
            let p = match selection.as_deref().map(|v| v.to_str()) {
                None => Profile::default(),
                Some(Some("--k9")) => Profile::k9(),
                Some(Some("--p1s")) => Profile::for_machine(MachinePreset::BambuP1s),
                Some(Some("--aries")) => Profile::for_machine(MachinePreset::VoxelabAries),
                _ => return Err("Expected --k9, --p1s or --aries after the output path".into()),
            };
            if it.next().is_some() {
                return Err("Unexpected profile-init arguments".into());
            }
            return save_profile(&p, &out);
        }
        "profile-migrate" => {
            let input = PathBuf::from(value(&mut it, "old profile")?);
            let out = PathBuf::from(value(&mut it, "new profile")?);
            if it.next().is_some() {
                return Err("Unexpected profile-migrate arguments".into());
            }
            return save_profile(&load_profile(&input)?, &out);
        }
        "convert" | "check" => {}
        _ => return Err(format!("Unknown command '{action}'. Try penplot --help.")),
    }
    let (mut profile, mut output) = (None, None);
    let (mut opts, mut import) = (JobOptions::default(), ImportOptions::default());
    let (mut calibrated, mut air, mut placement, mut margin) = (false, false, false, None);
    let mut files = Vec::new();
    let mut supplied_job_settings = false;
    let mut minimum = None;
    let mut smoothing = None;
    let mut colors_dir = None;
    let mut hatch_only = false;
    while let Some(arg) = it.next() {
        let flag = arg.to_str().unwrap_or("");
        if flag.starts_with('-')
            && !matches!(
                flag,
                "--help"
                    | "-h"
                    | "--"
                    | "--output"
                    | "-o"
                    | "--calibrated"
                    | "--air-run"
                    | "--colors-dir"
                    | "--min-image-stroke"
            )
        {
            supplied_job_settings = true;
        }
        match flag {
            "--help" | "-h" => {
                print!("{HELP}");
                return Ok(());
            }
            "--" => {
                files.extend(it.map(PathBuf::from));
                break;
            }
            "--profile" => profile = Some(PathBuf::from(value(&mut it, "--profile")?)),
            "--output" | "-o" => output = Some(PathBuf::from(value(&mut it, "output")?)),
            "--calibrated" => calibrated = true,
            "--air-run" => air = true,
            "--colors-dir" => colors_dir = Some(PathBuf::from(value(&mut it, "--colors-dir")?)),
            "--hatch-only" => hatch_only = true,
            "--raster" => {
                import.processing.mode = match value(&mut it, "--raster")?.to_str() {
                    Some("threshold") => penplot::RasterMode::Threshold,
                    Some("clean") => penplot::RasterMode::CleanLine,
                    Some("color") => penplot::RasterMode::Color,
                    _ => return Err("--raster must be threshold, clean or color".into()),
                }
            }
            "--palette" => {
                import.processing.palette_size = value(&mut it, "--palette")?
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or("--palette must be 2..8")?
            }
            "--include-background" => import.processing.exclude_background = false,
            "--white-cutoff" => {
                import.processing.background_cutoff = value(&mut it, "--white-cutoff")?
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or("--white-cutoff must be 128..255")?
            }
            "--clean-radius" => {
                import.processing.radius = value(&mut it, "--clean-radius")?
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or("--clean-radius must be 2..128")?
            }
            "--ink-contrast" => {
                import.processing.contrast = value(&mut it, "--ink-contrast")?
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or("--ink-contrast must be 1..80")?
            }
            "--speckle" => {
                import.processing.speckle = value(&mut it, "--speckle")?
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or("--speckle must be 0..100")?
            }
            "--fit" => margin = Some(num(&mut it, "--fit")?),
            "--scale" => {
                opts.scale = num(&mut it, "--scale")?;
                placement = true;
            }
            "--x" => {
                opts.x = num(&mut it, "--x")?;
                placement = true;
            }
            "--y" => {
                opts.y = num(&mut it, "--y")?;
                placement = true;
            }
            "--rotate" => opts.rotation_deg = num(&mut it, "--rotate")?,
            "--min-image-stroke" => minimum = Some(num(&mut it, "--min-image-stroke")?),
            "--optimize" => opts.optimize = true,
            "--smooth-pen-up" => smoothing = Some(true),
            "--straight-pen-up" => smoothing = Some(false),
            "--hatch" => {
                opts.fill = match value(&mut it, "--hatch")?.to_str() {
                    Some("lines") => Fill::Lines,
                    Some("cross") => Fill::Cross,
                    _ => return Err("--hatch must be lines or cross".into()),
                }
            }
            "--spacing" => opts.hatch_spacing = num(&mut it, "--spacing")?,
            "--angle" => opts.hatch_angle = num(&mut it, "--angle")?,
            "--dpi" => import.dpi = num(&mut it, "--dpi")?,
            "--threshold" => {
                import.threshold = value(&mut it, "--threshold")?
                    .to_str()
                    .and_then(|s| s.parse().ok())
                    .ok_or("--threshold must be an integer 0 to 255")?
            }
            "--invert" => import.invert = true,
            "--trace" => {
                import.trace = match value(&mut it, "--trace")?.to_str() {
                    Some("outline") => TraceMode::Outline,
                    Some("centerline") => TraceMode::Centerline,
                    _ => return Err("--trace must be outline or centerline".into()),
                }
            }
            "--tolerance" => import.tolerance = num(&mut it, "--tolerance")?,
            flag if flag.starts_with('-') => return Err(format!("Unknown option {flag}")),
            _ => files.push(PathBuf::from(arg)),
        }
    }
    if files.is_empty() || files.len() > 128 {
        return Err("Specify 1 to 128 input files".into());
    }
    if margin.is_some() && placement {
        return Err("--fit is incompatible with --scale/--x/--y".into());
    }
    let is_project = files.iter().any(|f| {
        f.extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("penplot"))
    });
    if is_project && (files.len() != 1 || supplied_job_settings) {
        return Err(
            "Use one .penplot project on its own; choose settings in Studio before saving it"
                .into(),
        );
    }
    if output.is_some() && colors_dir.is_some() {
        return Err("Use -o OR --colors-dir, not both.".into());
    }
    if action == "convert"
        && ((output.is_none() && colors_dir.is_none())
            || !calibrated
            || (!is_project && profile.is_none()))
    {
        return Err("Export requires -o FILE or --colors-dir NEW_FOLDER, --calibrated and an explicit --profile FILE or saved .penplot project".into());
    }
    if action == "check" && (output.is_some() || colors_dir.is_some()) {
        return Err("check does not write files; use convert".into());
    }
    let mut strokes = Vec::new();
    let mut raster = penplot::RasterLayers::new();
    let p = if is_project {
        let project = penplot::Project::from_json(&penplot::read_limited(&files[0])?)?;
        for (index, layer) in project.layers.iter().enumerate().filter(|(_, l)| l.visible) {
            penplot::append_layer(&mut strokes, &layer.strokes, index)?;
            if let Some(settings) = layer.raster_settings(&project.options) {
                raster.insert(index, settings);
            }
        }
        opts = project.options;
        project.profile
    } else {
        for (index, path) in files.into_iter().enumerate() {
            let art = penplot::import_bytes(
                &path.to_string_lossy(),
                &penplot::read_limited(&path)?,
                &import,
            )
            .map_err(|e| format!("{}: {e}", path.display()))?;
            for w in art.warnings {
                eprintln!("Warning [{}]: {w}", path.display());
            }
            penplot::append_layer(&mut strokes, &art.strokes, index)?;
            if penplot::is_raster_filename(&path.to_string_lossy()) {
                let mut settings = penplot::RasterSettings::from_job(&opts);
                if import.processing.mode == penplot::RasterMode::Color {
                    if settings.fill == Fill::None {
                        settings.fill = Fill::Lines;
                    }
                    settings.outline = false;
                } else if hatch_only {
                    settings.outline = false;
                }
                raster.insert(index, settings);
            }
        }
        profile
            .map(|path| load_profile(&path))
            .transpose()?
            .unwrap_or_default()
    };
    if !is_project {
        opts.smooth_pen_up = smoothing.unwrap_or(p.k9_lift_mm.is_some());
    }
    if let Some(m) = margin {
        penplot::fit(&strokes, &p, &mut opts, m)?;
    }
    if let Some(minimum) = minimum {
        opts.min_image_stroke_mm = minimum;
    }
    let plan = penplot::plan_with_raster(&strokes, &opts, &raster)?;
    if plan.removed_count > 0 {
        eprintln!(
            "Excluded {} image paths ({:.2} mm).",
            plan.removed_count, plan.removed_mm
        );
    }
    let routes = plan.strokes;
    if routes.is_empty() {
        return Err("No drawable paths remain after filtering".into());
    }
    if action == "check"
        && let Some(reason) = p.export_blocker()
    {
        p.check_layout(&routes)?;
        println!("{} paths | layout only: {reason}", routes.len());
        return Ok(());
    }
    let program = penplot::generate_with_travel(&routes, &p, air, opts.smooth_pen_up)?;
    println!(
        "{} paths | draw {:.2} mm | travel between paths {:.2} mm | ideal motion {:.1} s | {}",
        routes.len(),
        program.draw_mm,
        program.travel_mm,
        program.seconds,
        if air { "AIR RUN" } else { "PLOT" }
    );
    if let Some(path) = colors_dir {
        let jobs = penplot::color_jobs(&routes, &p, air, opts.smooth_pen_up)?;
        penplot::save_color_jobs(&path, &jobs)?;
        println!(
            "Wrote {} pen passes to {}. Follow PENS.txt; keep paper and XY origin fixed.",
            jobs.len(),
            path.display()
        );
    }
    if let Some(path) = output {
        penplot::save_file(&path, program.gcode.as_bytes(), false)?;
        println!("Wrote {}", path.display());
    }
    Ok(())
}
fn main() -> std::process::ExitCode {
    match run(std::env::args_os().skip(1).collect::<Vec<_>>().into_iter()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("penplot: {e}");
            std::process::ExitCode::from(2)
        }
    }
}
