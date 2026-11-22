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
                name: "visible".into(),
                visible: true,
                strokes: vec![rect(0., 0., 5., 5., 1)],
            },
            ProjectLayer {
                raster: None,
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
