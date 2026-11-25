use penplot::{
    Fill, ImportOptions, JobOptions, Point, Profile, Stroke, fit, generate, import_bytes, plan,
};
use std::process::Command;
fn rect(x0: f64, y0: f64, x1: f64, y1: f64, group: u32) -> Stroke {
    Stroke {
        points: vec![
            Point::new(x0, y0),
            Point::new(x1, y0),
            Point::new(x1, y1),
            Point::new(x0, y1),
        ],
        layer: "ink".into(),
        group,
        closed: true,
        filled: true,
    }
}
#[test]
fn rotated_fit_uses_the_intersection_with_carriage_limits() {
    let s = [rect(-20., -10., 0., 0., 0)];
    let mut p = Profile::default();
    p.offset_x = 15.;
    p.offset_y = -7.;
    let mut o = JobOptions {
        rotation_deg: 90.,
        ..JobOptions::default()
    };
    fit(&s, &p, &mut o, 5.).unwrap();
    let r = plan(&s, &o).unwrap();
    generate(&r, &p).unwrap();
    assert!((o.scale - 4.15).abs() < 1e-8);
    assert!(o.x >= 5. && o.y >= 12. - 1e-9);
    assert!(fit(&s, &p, &mut o, 49.).is_err());
}
#[test]
fn fit_accepts_vertical_or_horizontal_but_rejects_points() {
    let mut s = rect(2., 3., 2., 9., 0);
    s.closed = false;
    s.points.truncate(3);
    let mut o = JobOptions::default();
    fit(&[s.clone()], &Profile::default(), &mut o, 2.).unwrap();
    assert_eq!(o.x, 50.);
    s.points = vec![Point::default(); 2];
    assert!(fit(&[s], &Profile::default(), &mut o, 2.).is_err());
}
#[test]
fn hatching_respects_holes_and_independent_objects() {
    let a = rect(0., 0., 10., 10., 0);
    let h = rect(3., 3., 7., 7., 0);
    let o = JobOptions {
        fill: Fill::Lines,
        hatch_angle: 0.,
        hatch_spacing: 1.,
        ..Default::default()
    };
    let r = plan(&[a.clone(), h.clone()], &o).unwrap();
    let row: Vec<_> = r[2..]
        .iter()
        .filter(|s| (s.points[0].y - 4.5).abs() < 1e-8)
        .collect();
    assert_eq!(row.len(), 2);
    assert!(
        row.iter()
            .all(|s| s.points.iter().all(|p| p.x <= 3.) || s.points.iter().all(|p| p.x >= 7.))
    );
    let mut combined = vec![a];
    penplot::append_artwork(&mut combined, &[h]).unwrap();
    assert_ne!(combined[0].group, combined[1].group);
    assert!(
        plan(&combined, &o).unwrap()[2..]
            .iter()
            .any(|s| s.points[0].y == 4.5 && s.points[0].distance(s.points[1]) == 10.)
    );
}
#[test]
fn optimize_keeps_geometry_and_never_draws_a_connector() {
    let line = |x: f64| Stroke {
        points: vec![Point::new(x, 0.), Point::new(x + 1., 0.)],
        layer: "test".into(),
        group: 1,
        closed: false,
        filled: false,
    };
    let src = vec![line(0.), line(75.), line(8.), line(4.)];
    let p = Profile::default();
    let orig = generate(&plan(&src, &Default::default()).unwrap(), &p).unwrap();
    let opt = generate(
        &plan(
            &src,
            &JobOptions {
                optimize: true,
                ..Default::default()
            },
        )
        .unwrap(),
        &p,
    )
    .unwrap();
    assert_eq!(orig.draw_mm, 4.);
    assert_eq!(opt.draw_mm, 4.);
    assert!(opt.travel_mm < orig.travel_mm);
    assert_eq!(
        opt.gcode
            .lines()
            .filter(|s| s.starts_with("G1 Z2.000"))
            .count(),
        4
    );
}
#[test]
fn refuse_excessive_hatch_cost_and_bad_placement() {
    let s = [rect(0., 0., 20., 1e7, 0)];
    assert!(
        plan(
            &s,
            &JobOptions {
                fill: Fill::Cross,
                hatch_spacing: 0.02,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        plan(
            &s,
            &JobOptions {
                scale: f64::NAN,
                ..Default::default()
            }
        )
        .is_err()
    );
}
#[test]
fn sample_vectors_parse_and_generate_without_python() {
    let svg = import_bytes(
        "CARD.SVG",
        include_bytes!("../examples/alignment-card.svg"),
        &ImportOptions::default(),
    )
    .unwrap();
    assert!(svg.warnings.is_empty());
    assert!(svg.strokes.len() > 12);
    let data = import_bytes(
        "mark.dxf",
        include_bytes!("../examples/calibration.dxf"),
        &Default::default(),
    )
    .unwrap();
    assert_eq!(data.strokes.len(), 2);
    let o = JobOptions {
        fill: Fill::Cross,
        rotation_deg: 30.,
        optimize: true,
        x: 3.,
        y: 3.,
        ..Default::default()
    };
    for a in [svg, data] {
        let routes = plan(&a.strokes, &o).unwrap();
        assert!(generate(&routes, &Profile::default()).is_ok());
    }
}
#[test]
fn cli_refuses_unsafe_exports_and_does_not_clobber() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_penplot"))
            .current_dir(dir.path())
            .args(args)
            .output()
            .unwrap()
    };
    let input = "図形.svg";
    std::fs::write(
        dir.path().join(input),
        include_bytes!("../examples/alignment-card.svg"),
    )
    .unwrap();
    assert!(
        cmd(&["profile-init", "machine.json", "--k9"])
            .status
            .success()
    );
    assert!(
        !cmd(&[
            "convert",
            "--profile",
            "machine.json",
            "-o",
            "A.GCODE",
            input
        ])
        .status
        .success()
    );
    assert!(!dir.path().join("A.GCODE").exists());
    let args = [
        "convert",
        "--profile",
        "machine.json",
        "--calibrated",
        "--air-run",
        "--fit",
        "5",
        "-o",
        "A.GCODE",
        input,
    ];
    let out = cmd(&args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let data = std::fs::read_to_string(dir.path().join("A.GCODE")).unwrap();
    assert!(data.starts_with("; AIR RUN:"));
    let args2 = [
        "convert",
        "--profile",
        "machine.json",
        "--calibrated",
        "--air-run",
        "--fit",
        "5",
        "-o",
        "B.GCODE",
        input,
    ];
    assert!(cmd(&args2).status.success());
    assert_eq!(
        data,
        std::fs::read_to_string(dir.path().join("B.GCODE")).unwrap()
    );
    assert!(!cmd(&args).status.success());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("A.GCODE")).unwrap(),
        data
    );
    assert!(!cmd(&["check", "--x", "1000", input]).status.success());
    assert!(
        !cmd(&["check", "--fit", "2", "--y", "1", input])
            .status
            .success()
    );
}

#[test]
fn cli_k9_start_is_prefilled_but_only_emitted_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_penplot"))
            .current_dir(dir.path())
            .args(args)
            .output()
            .unwrap()
    };
    let write = |name, bytes: &[u8]| std::fs::write(dir.path().join(name), bytes).unwrap();
    let read = |name| std::fs::read_to_string(dir.path().join(name)).unwrap();
    write("art.svg", include_bytes!("../examples/alignment-card.svg"));
    assert!(cmd(&["profile-init", "k9.json", "--k9"]).status.success());
    let mut profile: serde_json::Value = serde_json::from_str(&read("k9.json")).unwrap();
    let exact = "M203 X100 Y100\nM201 X200 Y200\nM204 P100 T200";
    assert_eq!(profile["k9_start_gcode"]["code"], exact);
    assert_eq!(profile["k9_start_gcode"]["enabled"], false);
    let convert = |output, air| {
        let mut args = vec![
            "convert",
            "--profile",
            "k9.json",
            "--calibrated",
            "-o",
            output,
            "art.svg",
        ];
        if air {
            args.insert(1, "--air-run");
        }
        cmd(&args)
    };
    assert!(convert("off.gcode", true).status.success());
    let off = read("off.gcode");
    assert!(!off.contains("M203"));
    assert!(off.contains("F2400.000"));

    profile["k9_start_gcode"]["enabled"] = true.into();
    write("k9.json", &serde_json::to_vec(&profile).unwrap());
    for (name, air) in [("on.gcode", false), ("air-on.gcode", true)] {
        let result = convert(name, air);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let on = read(name);
        assert!(on.contains(&format!(
            "G21\nG90\n; K9 experimental planner settings (not saved by this file)\n{exact}\nG1 Z"
        )));
        assert_eq!(on.matches("M203").count(), 1);
        assert!(on.contains("F2400.000"));
        assert!(!on.contains("M500"));
        assert_eq!(on.contains("G1 Z2.000"), !air);
    }
    assert_eq!(
        read("air-on.gcode")
            .lines()
            .filter(|l| l.starts_with("G1"))
            .collect::<Vec<_>>(),
        off.lines()
            .filter(|l| l.starts_with("G1"))
            .collect::<Vec<_>>()
    );

    for disallowed in [
        "M500",
        "G91",
        "G1 Z-4",
        "M203 Z5",
        "M203 X101",
        "M201 X200 E200",
        "M204 T500",
        "M204 R100",
        "M203 X100 X99",
        "M203 X100 M500",
        "M203 X100\rM500",
        "M201 X100*15",
    ] {
        profile["k9_start_gcode"]["code"] = disallowed.into();
        write("k9.json", &serde_json::to_vec(&profile).unwrap());
        let result = convert("bad.gcode", true);
        assert!(!result.status.success(), "accepted {disallowed}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("K9 start G-code line"));
        assert!(!dir.path().join("bad.gcode").exists());
    }

    profile["k9_start_gcode"]["code"] = "M204 T150 P100 ; M500 is not executable".into();
    write("k9.json", &serde_json::to_vec(&profile).unwrap());
    assert!(convert("edit.gcode", true).status.success());
    assert!(read("edit.gcode").contains("\nM204 T150 P100\n"));
    assert!(!read("edit.gcode").contains("M500"));
}

#[test]
fn cli_k9_start_and_quadratic_travel_coexist_without_changing_draw_commands() {
    let dir = tempfile::tempdir().unwrap();
    // Two strokes with nonparallel tangents, separated by a 20 mm pen-up link.
    std::fs::write(
        dir.path().join("two.svg"),
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="50mm" viewBox="0 0 50 50"><path d="M5 45 L10 40 M30 40 L35 45" fill="none" stroke="black"/></svg>"#,
    )
    .unwrap();
    let convert = |prefix: bool, smooth: bool, air: bool, out: &str| {
        let mut profile = Profile::k9();
        profile.k9_start_gcode.as_mut().unwrap().enabled = prefix;
        std::fs::write(
            dir.path().join("k9.json"),
            serde_json::to_vec(&profile).unwrap(),
        )
        .unwrap();
        let mut args = vec![
            "convert",
            "--profile",
            "k9.json",
            "--calibrated",
            "--x",
            "5",
            "--y",
            "5",
            "-o",
            out,
            "two.svg",
        ];
        if smooth {
            args.insert(1, "--smooth-pen-up");
        } else {
            args.insert(1, "--straight-pen-up");
        }
        if air {
            args.insert(1, "--air-run");
        }
        let result = Command::new(env!("CARGO_BIN_EXE_penplot"))
            .current_dir(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        std::fs::read_to_string(dir.path().join(out)).unwrap()
    };
    let straight = convert(true, false, false, "STRAIGHT.GCODE");
    let curve = convert(true, true, false, "CURVE.GCODE");
    let curve_without_start = convert(false, true, false, "BASE.GCODE");
    let air = convert(true, true, true, "AIR.GCODE");
    let exact = "M203 X100 Y100\nM201 X200 Y200\nM204 P100 T200\n";
    for output in [&straight, &curve, &air] {
        assert_eq!(output.matches(exact).count(), 1);
        assert!(output.contains(&format!("{exact}G1 Z")));
    }
    assert_eq!(
        curve.replace(
            &format!("; K9 experimental planner settings (not saved by this file)\n{exact}"),
            ""
        ),
        curve_without_start
    );
    let lines = |g: &str, pred: fn(&str) -> bool| {
        g.lines()
            .filter(|l| pred(l))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let draw = |l: &str| l.starts_with("G1 X") && l.ends_with("F1200.000");
    let travel = |l: &str| l.starts_with("G1 X") && l.ends_with("F2400.000");
    let z = |l: &str| l.starts_with("G1 Z");
    assert_eq!(lines(&curve, draw), lines(&straight, draw));
    assert_eq!(lines(&curve, z), lines(&straight, z));
    assert!(lines(&curve, travel).len() > lines(&straight, travel).len());
    assert_eq!(lines(&curve, travel), lines(&air, travel));
    assert!(lines(&air, z).iter().all(|l| l.starts_with("G1 Z4.000")));
}

#[test]
fn cli_project_roundtrip_matches_visible_gui_layer_route() {
    use penplot::{Project, ProjectLayer};
    let dir = tempfile::tempdir().unwrap();
    let sh = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_penplot"))
            .current_dir(dir.path())
            .args(args)
            .output()
            .unwrap()
    };
    let mut project = Project {
        layers: vec![
            ProjectLayer {
                raster: None,
                text: None,
                name: "visible".into(),
                visible: true,
                strokes: vec![rect(0., 0., 5., 5., 1)],
            },
            ProjectLayer {
                raster: None,
                text: None,
                name: "hidden".into(),
                visible: false,
                strokes: vec![rect(-200., 0., 0., 50., 1)],
            },
        ],
        options: JobOptions {
            x: 3.,
            y: 4.,
            ..Default::default()
        },
        profile: Profile::default(),
        import_options: ImportOptions::default(),
    };
    std::fs::write(dir.path().join("job.penplot"), project.to_json().unwrap()).unwrap();
    assert!(sh(&["check", "job.penplot"]).status.success());
    assert!(
        !sh(&["check", "--scale", "1", "job.penplot"])
            .status
            .success()
    );
    assert!(
        !sh(&["convert", "-o", "bad.gcode", "job.penplot"])
            .status
            .success()
    );
    assert!(
        sh(&["convert", "--calibrated", "-o", "ok.gcode", "job.penplot"])
            .status
            .success()
    );
    let mut visible = Vec::new();
    penplot::append_layer(&mut visible, &project.layers[0].strokes, 0).unwrap();
    let expected = generate(&plan(&visible, &project.options).unwrap(), &project.profile).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ok.gcode")).unwrap(),
        expected.gcode
    );
    project.layers[0].visible = false;
    std::fs::write(dir.path().join("blank.penplot"), project.to_json().unwrap()).unwrap();
    assert!(!sh(&["check", "blank.penplot"]).status.success());
}

#[test]
fn raster_filter_keeps_placement_and_unrelated_short_vectors() {
    let mut image = rect(0., 0., 0.1, 0.1, 1);
    image.layer = "Raster contours".into();
    let mut long = rect(10., 10., 12., 12., 2);
    long.layer = "Raster contours".into();
    let vector = rect(4., 4., 4.1, 4.1, 1);
    let mut strokes = vec![];
    penplot::append_layer(&mut strokes, &[image, long], 0).unwrap();
    penplot::append_layer(&mut strokes, &[vector], 1).unwrap();
    let raster = penplot::RasterLayers::from([(0, Default::default())]);
    let mut opts = JobOptions {
        x: 20.,
        y: 30.,
        scale: 2.,
        ..Default::default()
    };
    let old = penplot::plan_with_raster(&strokes, &opts, &raster).unwrap();
    opts.min_image_stroke_mm = 1.;
    let reduced = penplot::plan_with_raster(&strokes, &opts, &raster).unwrap();
    assert_eq!(reduced.removed_count, 1);
    assert_eq!(reduced.strokes, old.strokes[1..]);
    opts.min_image_stroke_mm = 0.8;
    assert_eq!(
        penplot::plan_with_raster(&strokes, &opts, &raster)
            .unwrap()
            .removed_count,
        0
    );
    opts.scale = 4.;
    opts.min_image_stroke_mm = 1.;
    assert_eq!(
        penplot::plan_with_raster(&strokes, &opts, &raster)
            .unwrap()
            .removed_count,
        0
    );
}

#[test]
fn raster_cutoff_does_not_restore_hatch_inside_a_removed_hole() {
    let outer = rect(0., 0., 4., 4., 3);
    let inner = rect(1.95, 2.075, 2.05, 2.175, 3);
    let mut sources = vec![];
    penplot::append_layer(&mut sources, &[outer, inner], 0).unwrap();
    let raster = penplot::RasterLayers::from([(
        0,
        penplot::RasterSettings {
            fill: Fill::Lines,
            hatch_spacing: 0.25,
            hatch_angle: 0.0,
        },
    )]);
    let opts = JobOptions {
        min_image_stroke_mm: 0.5,
        ..Default::default()
    };
    let filtered = penplot::plan_with_raster(&sources, &opts, &raster).unwrap();
    assert_eq!(filtered.removed_count, 1);
    let hatch_at_hole: Vec<_> = filtered
        .strokes
        .iter()
        .filter(|s| !s.closed && s.points[0].y == 2.125)
        .collect();
    assert_eq!(hatch_at_hole.len(), 2);
    assert!(hatch_at_hole.iter().all(|s| {
        !s.points
            .windows(2)
            .any(|p| p[0].x.min(p[1].x) < 2.0 && p[0].x.max(p[1].x) > 2.0)
    }));
}

#[test]
fn portable_text_project_loads_and_cli_outputs_without_a_font() {
    use penplot::{Project, ProjectLayer, TextLayer, text_outline};
    let temp = tempfile::tempdir().unwrap();
    let font_path = temp.path().join("only-here.ttf");
    std::fs::write(&font_path, include_bytes!("fixtures/font/synthetic.ttf")).unwrap();
    let face = text_outline::list_font_faces(&font_path).unwrap().remove(0);
    let (art, font) =
        text_outline::outline_text_with_identity(&face, "fi 日本語\nO", 10., 0.025).unwrap();
    let mut project = Project {
        layers: vec![ProjectLayer {
            name: "Text: fi 日本語".into(),
            visible: true,
            strokes: art.strokes,
            raster: None,
            text: Some(TextLayer {
                content: "fi 日本語\nO".into(),
                font,
                em_mm: 10.,
                tolerance_mm: 0.025,
                trace: penplot::TraceMode::Outline,
                transform: [1., 0., 0., 1., 0., 0.],
            }),
        }],
        options: JobOptions {
            x: 10.,
            y: 10.,
            ..Default::default()
        },
        profile: Profile::default(),
        import_options: ImportOptions::default(),
    };
    let json = project.to_json().unwrap();
    assert!(!std::str::from_utf8(&json).unwrap().contains("only-here"));
    std::fs::remove_file(font_path).unwrap();
    let saved = temp.path().join("text.penplot");
    std::fs::write(&saved, &json).unwrap();
    let loaded = Project::from_json(&json).unwrap();
    assert_eq!(loaded, project);
    let result = Command::new(env!("CARGO_BIN_EXE_penplot"))
        .current_dir(temp.path())
        .args([
            "convert",
            "--calibrated",
            "-o",
            "text.gcode",
            "text.penplot",
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut strokes = vec![];
    penplot::append_layer(&mut strokes, &loaded.layers[0].strokes, 0).unwrap();
    let expected = generate(&plan(&strokes, &loaded.options).unwrap(), &loaded.profile).unwrap();
    assert_eq!(
        std::fs::read_to_string(temp.path().join("text.gcode")).unwrap(),
        expected.gcode
    );
    project.layers[0].text = None;
    let mut legacy = serde_json::to_value(
        serde_json::from_slice::<serde_json::Value>(&project.to_json().unwrap()).unwrap(),
    )
    .unwrap();
    legacy["version"] = 1.into();
    legacy["layers"][0].as_object_mut().unwrap().remove("text");
    assert!(
        Project::from_json(&serde_json::to_vec(&legacy).unwrap())
            .unwrap()
            .layers[0]
            .text
            .is_none()
    );
}

#[test]
fn malformed_text_metadata_is_rejected_without_loading_fonts() {
    use penplot::{Project, ProjectLayer, TextLayer, text_outline};
    let face = text_outline::list_font_faces(
        &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/font/synthetic.ttf"),
    )
    .unwrap()
    .remove(0);
    let (art, font) = text_outline::outline_text_with_identity(&face, "O", 10., 0.025).unwrap();
    let original = Project {
        layers: vec![ProjectLayer {
            name: "Text: O".into(),
            visible: true,
            strokes: art.strokes,
            raster: None,
            text: Some(TextLayer {
                content: "O".into(),
                font,
                em_mm: 10.,
                tolerance_mm: 0.025,
                trace: penplot::TraceMode::Outline,
                transform: [1., 0., 0., 1., 0., 0.],
            }),
        }],
        options: JobOptions::default(),
        profile: Profile::default(),
        import_options: ImportOptions::default(),
    };
    for mutation in 0..8 {
        let mut project = original.clone();
        let text = project.layers[0].text.as_mut().unwrap();
        match mutation {
            0 => text.content = "O".repeat(2001),
            1 => text.content = "O\n".repeat(101),
            2 => text.font.sha256 = "bad".into(),
            3 => text.transform[0] = 0.,
            4 => text.em_mm = f64::NAN,
            5 => text.transform[2] = 1.,
            6 => text.content = "\u{0007}".into(),
            _ => project.layers[0].strokes.clear(),
        }
        assert!(project.to_json().is_err(), "mutation {mutation}");
    }
}

#[test]
fn k9_cli_exports_wait_then_vertical_lift_before_curved_travel_and_keeps_air_up() {
    let dir = tempfile::tempdir().unwrap();
    let profile_path = dir.path().join("k9.json");
    let svg_path = dir.path().join("links.svg");
    let cli = env!("CARGO_BIN_EXE_penplot");
    assert!(
        Command::new(cli)
            .arg("profile-init")
            .arg(&profile_path)
            .arg("--k9")
            .status()
            .unwrap()
            .success()
    );
    let raw = std::fs::read_to_string(&profile_path).unwrap();
    let (mut profile, _) = Profile::from_json(&raw).unwrap();
    assert_eq!((profile.draw_speed, profile.travel_speed), (20.0, 40.0));
    assert_eq!((profile.k9_lift_mm, profile.liftup_delay), (Some(2.0), 30));
    assert!(!profile.k9_start_gcode.as_ref().unwrap().enabled);
    profile.k9_lift_mm = Some(2.5);
    profile.pen_down_z = 1.25;
    profile.offset_z = 0.2;
    profile.liftup_delay = 47;
    std::fs::write(&profile_path, serde_json::to_vec(&profile).unwrap()).unwrap();
    std::fs::write(
        &svg_path,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="50mm" height="30mm" viewBox="0 0 50 30"><path d="M5 25 L10 20 M30 20 L35 25" fill="none" stroke="black"/></svg>"#,
    )
    .unwrap();
    let export = |name: &str, optional: &[&str]| {
        let path = dir.path().join(name);
        let run = Command::new(cli)
            .args([
                "convert",
                "--calibrated",
                "--x",
                "5",
                "--y",
                "5",
                "--profile",
            ])
            .arg(&profile_path)
            .arg("-o")
            .arg(&path)
            .args(optional)
            .arg(&svg_path)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        std::fs::read_to_string(path).unwrap()
    };
    let curve = export("curve.gcode", &[]);
    let straight = export("straight.gcode", &["--straight-pen-up"]);
    let air = export("air.gcode", &["--air-run"]);
    let lift = "G4 P47\nG1 Z3.950 F120.000\n";
    assert!(curve.contains("G21\nG90\n"));
    assert!(
        !curve
            .lines()
            .any(|line| line.starts_with("M20") || line == "G91")
    );
    assert!(curve.contains(&format!("G1 X10.000 Y10.000 F1200.000\n{lift}")));
    assert!(curve.ends_with(lift));
    let between = curve.split(lift).nth(1).unwrap();
    let to_next = between.split("G1 Z1.450 F120.000\n").next().unwrap();
    assert!(
        to_next.lines().count() > 1
            && to_next
                .lines()
                .all(|l| l.starts_with("G1 X") && l.ends_with("F2400.000"))
    );
    assert!(to_next.ends_with("G1 X30.000 Y10.000 F2400.000\n"));
    assert!(straight.contains(&format!("{lift}G1 X30.000 Y10.000 F2400.000\nG1 Z1.450")));
    assert!(
        air.lines()
            .filter(|l| l.starts_with("G1 Z"))
            .all(|l| l == "G1 Z3.950 F120.000")
    );
    profile.max_z = 3.9;
    assert!(profile.validate().is_err());
}
