//! Public pipeline and real CLI regression: only synthetic, redistributable pixels.
use image::{ImageEncoder, Rgba, RgbaImage};
use penplot::*;
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

fn fixture() -> RgbaImage {
    let mut image = RgbaImage::new(80, 60);
    for y in 10..50 {
        for x in 10..70 {
            image.put_pixel(
                x,
                y,
                Rgba(if x < 40 {
                    [220, 72, 72, 255]
                } else {
                    [55, 125, 225, 255]
                }),
            );
        }
    }
    for x in 20..60 {
        image.put_pixel(x, 25, Rgba([0, 0, 0, 255]));
    }
    for y in 26..43 {
        image.put_pixel(40, y, Rgba([0, 0, 0, 255]));
    }
    for y in 35..43 {
        for x in 20..28 {
            image.put_pixel(x, y, Rgba([0, 0, 0, 0]));
        }
    }
    image.put_pixel(4, 4, Rgba([0, 0, 0, 255]));
    image
}
fn png(image: &RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
    bytes
}
fn settings() -> RasterSettings {
    RasterSettings {
        pattern: None,
        pen_width_mm: 0.,
        fill: Fill::Lines,
        outline: false,
        hatch_spacing: 0.5,
        hatch_angle: 0.,
    }
}
fn project() -> Project {
    let mut import = ImportOptions {
        dpi: 25.4,
        ..Default::default()
    };
    import.processing.mode = RasterMode::Color;
    import.processing.palette_size = 3;
    let art = import_bytes("synthetic.png", &png(&fixture()), &import).unwrap();
    let mut by_color = BTreeMap::<String, Vec<Stroke>>::new();
    for s in art.strokes {
        by_color.entry(s.layer.clone()).or_default().push(s);
    }
    assert_eq!(by_color.len(), 3);
    Project {
        layers: by_color
            .into_iter()
            .map(|(name, strokes)| ProjectLayer {
                name,
                visible: true,
                strokes,
                raster: Some(settings()),
                text: None,
            })
            .collect(),
        options: JobOptions {
            x: 5.,
            y: 5.,
            ..Default::default()
        },
        profile: Profile::default(),
        import_options: import,
    }
}
fn routes(p: &Project) -> Vec<Stroke> {
    let mut strokes = Vec::new();
    let mut raster = RasterLayers::new();
    for (i, l) in p.layers.iter().enumerate().filter(|(_, l)| l.visible) {
        append_layer(&mut strokes, &l.strokes, i).unwrap();
        raster.insert(i, l.raster.clone().unwrap());
    }
    plan_with_raster(&strokes, &p.options, &raster)
        .unwrap()
        .strokes
}
fn drawn(gcode: &str, up: f64) -> BTreeSet<((i64, i64), (i64, i64))> {
    let mut previous = None;
    let mut lifted = true;
    let mut edges = BTreeSet::new();
    for line in gcode.lines() {
        if let Some(z) = line.strip_prefix("G1 Z") {
            lifted = z.split_whitespace().next().unwrap().parse::<f64>().unwrap() >= up;
        }
        if let Some(xy) = line.strip_prefix("G1 X") {
            let mut parts = xy.split_whitespace();
            let x = parts.next().unwrap().parse::<f64>().unwrap();
            let y = parts
                .next()
                .unwrap()
                .strip_prefix('Y')
                .unwrap()
                .parse::<f64>()
                .unwrap();
            let next = ((x * 1000.).round() as i64, (y * 1000.).round() as i64);
            if let Some(last) = previous
                && !lifted
            {
                assert!(edges.insert((last, next)), "duplicate drawing segment");
            }
            previous = Some(next);
        }
    }
    edges
}
#[test]
fn color_hatches_stay_inside_their_pixels_skip_holes_and_have_no_shared_outline() {
    let p = project();
    let routes = routes(&p);
    let image = fixture();
    assert!(!routes.is_empty());
    assert!(routes.iter().all(|s| !s.closed && !s.filled));
    for s in &routes {
        let color = ink_rgb(&s.layer).unwrap();
        for segment in s.points.windows(2) {
            for t in [0.001, 0.1, 0.3, 0.5, 0.7, 0.9, 0.999] {
                // Image ink bounds start at source (4,10), placed at paper (5,5).
                let x = segment[0].x + (segment[1].x - segment[0].x) * t - 5. + 4.;
                let y = segment[0].y + (segment[1].y - segment[0].y) * t - 5. + 10.;
                let px = image.get_pixel(x.floor() as u32, (60. - y).floor() as u32);
                assert_eq!(px[3], 255, "hatch enters transparency at {x},{y}");
                assert_eq!([px[0], px[1], px[2]], color, "hatch crosses color boundary");
            }
        }
    }
    let output = generate(&routes, &p.profile).unwrap();
    assert!(!drawn(&output.gcode, p.profile.clearance_z()).is_empty());
}
#[test]
fn color_pass_actual_commands_match_whole_job_after_save_reload_and_reorder() {
    let p = project();
    let loaded = Project::from_json(&p.to_json().unwrap()).unwrap();
    assert_eq!(loaded, p);
    let whole = routes(&loaded);
    let expected = drawn(
        &generate(&whole, &p.profile).unwrap().gcode,
        p.profile.clearance_z(),
    );
    let jobs = color_jobs(&whole, &p.profile, false, true).unwrap();
    let union = jobs
        .iter()
        .flat_map(|j| drawn(&j.output.gcode, p.profile.clearance_z()))
        .collect::<BTreeSet<_>>();
    assert_eq!(expected, union);
    for j in &jobs {
        let selected: Vec<_> = whole
            .iter()
            .filter(|s| ink_rgb(&s.layer) == Some(j.color))
            .cloned()
            .collect();
        assert_eq!(
            drawn(
                &generate(&selected, &p.profile).unwrap().gcode,
                p.profile.clearance_z()
            ),
            drawn(&j.output.gcode, p.profile.clearance_z())
        );
        for line in j.output.gcode.lines().filter(|l| !l.starts_with(';')) {
            assert!(
                !line.starts_with("M0")
                    && !line.starts_with("M6")
                    && !line.starts_with('T')
                    && !line.starts_with("G28")
            );
        }
    }
    let mut reversed = loaded;
    reversed.layers.reverse();
    let reverse = color_jobs(&routes(&reversed), &p.profile, false, true).unwrap();
    for (a, b) in jobs.iter().zip(reverse) {
        assert_eq!(a.color, b.color);
        assert_eq!(a.output.gcode, b.output.gcode);
    }
}
#[test]
fn cli_color_folder_matches_saved_gui_pipeline_and_never_overwrites() {
    let temp = tempfile::tempdir().unwrap();
    let p = project();
    let file = temp.path().join("job.penplot");
    std::fs::write(&file, p.to_json().unwrap()).unwrap();
    let out = temp.path().join("pens");
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_penplot"))
            .args(["convert", "--calibrated", "--colors-dir"])
            .arg(&out)
            .arg(&file)
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let expected = color_jobs(&routes(&p), &p.profile, false, false).unwrap();
    for j in expected {
        assert_eq!(
            std::fs::read_to_string(out.join(j.filename)).unwrap(),
            j.output.gcode
        );
    }
    assert!(out.join("PENS.txt").is_file());
    assert!(!run().status.success());
    let png_path = temp.path().join("colors.png");
    std::fs::write(&png_path, png(&fixture())).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_penplot"))
        .args([
            "check",
            "--raster",
            "color",
            "--palette",
            "3",
            "--dpi",
            "25.4",
            "--spacing",
            "0.5",
            "--angle",
            "0",
        ])
        .arg(&png_path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result = Command::new(env!("CARGO_BIN_EXE_penplot"))
        .args(["check", "--raster", "clean", "--dpi", "25.4"])
        .arg(&png_path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn transparent_and_white_background_controls_and_legacy_modes_remain_distinct() {
    let mut image = RgbaImage::from_pixel(12, 8, Rgba([255, 255, 255, 255]));
    for x in 2..10 {
        image.put_pixel(x, 3, Rgba([0, 0, 0, 255]));
    }
    image.put_pixel(0, 0, Rgba([255, 0, 0, 0]));
    let bytes = png(&image);
    let mut o = ImportOptions {
        dpi: 25.4,
        ..Default::default()
    };
    let outline = import_bytes("x.png", &bytes, &o).unwrap();
    assert!(outline.strokes.iter().all(|s| s.closed));
    o.trace = TraceMode::Centerline;
    let center = import_bytes("x.png", &bytes, &o).unwrap();
    assert_eq!(center.strokes.len(), 1);
    assert!(!center.strokes[0].closed);
    o.processing.mode = RasterMode::Color;
    assert!(
        import_bytes("x.png", &bytes, &o)
            .unwrap()
            .strokes
            .iter()
            .all(|s| s.layer == "#000000")
    );
    o.processing.exclude_background = false;
    let all = import_bytes("x.png", &bytes, &o).unwrap();
    assert!(all.strokes.iter().any(|s| s.layer == "#FFFFFF"));
    assert!(!all.strokes.iter().any(|s| s.layer == "#FF0000"));
}
