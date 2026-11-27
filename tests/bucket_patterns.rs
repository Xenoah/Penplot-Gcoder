use penplot::{
    Fill, ImportOptions, JobOptions, Pattern, PatternSettings, Point, Profile, Project,
    ProjectLayer, RasterSettings, Stroke,
};
fn ring(x: f64, y: f64, w: f64, h: f64) -> Stroke {
    Stroke {
        points: vec![
            Point::new(x, y),
            Point::new(x + w, y),
            Point::new(x + w, y + h),
            Point::new(x, y + h),
            Point::new(x, y),
        ],
        layer: "outline".into(),
        group: 0,
        closed: true,
        filled: false,
    }
}
fn fill(pattern: Pattern, density: f64) -> RasterSettings {
    RasterSettings {
        pattern: Some(PatternSettings {
            pattern,
            density,
            ..Default::default()
        }),
        fill: Fill::Lines,
        outline: false,
        ..Default::default()
    }
}
fn planned(region: &[Stroke], setting: RasterSettings) -> Vec<Stroke> {
    let mut source = vec![];
    penplot::append_layer(&mut source, region, 0).unwrap();
    penplot::plan_with_raster(
        &source,
        &JobOptions {
            x: 5.,
            y: 5.,
            ..Default::default()
        },
        &[(0, setting)].into_iter().collect(),
    )
    .unwrap()
    .strokes
}
#[test]
fn bucket_selects_faces_holes_and_separately_drawn_connected_lines() {
    let outer = ring(0., 0., 40., 30.);
    let hole = ring(10., 10., 10., 10.);
    let region =
        penplot::bucket_region(&[outer.clone(), hole.clone()], Point::new(5., 5.)).unwrap();
    assert_eq!(region.len(), 2);
    assert!(penplot::region_contains(&region, Point::new(5., 5.)));
    assert!(!penplot::region_contains(&region, Point::new(15., 15.)));
    let inner = penplot::bucket_region(&[outer.clone(), hole], Point::new(15., 15.)).unwrap();
    assert_eq!(inner.len(), 1);
    let mut segments: Vec<_> = outer
        .points
        .windows(2)
        .map(|p| Stroke {
            points: p.to_vec(),
            closed: false,
            ..outer.clone()
        })
        .collect();
    let split = Stroke {
        points: vec![Point::new(20., 0.), Point::new(20., 30.)],
        closed: false,
        ..outer.clone()
    };
    segments.push(split);
    let left = penplot::bucket_region(&segments, Point::new(5., 5.)).unwrap();
    assert!(penplot::region_contains(&left, Point::new(5., 5.)));
    assert!(!penplot::region_contains(&left, Point::new(25., 5.)));
    let right = penplot::bucket_region(&segments, Point::new(25., 5.)).unwrap();
    assert!(!penplot::region_contains(&right, Point::new(5., 5.)));
    // Exact duplicate boundaries do not cancel or double the region.
    segments.extend(segments.clone());
    assert_eq!(
        penplot::bucket_region(&segments, Point::new(5., 5.)).unwrap(),
        left
    );
}
#[test]
fn bucket_never_bridges_a_gap_or_fills_outside() {
    let mut open = ring(0., 0., 30., 30.);
    open.closed = false;
    open.points.pop();
    assert!(penplot::bucket_region(&[open], Point::new(10., 10.)).is_err());
    let closed = ring(0., 0., 30., 30.);
    assert!(penplot::bucket_region(std::slice::from_ref(&closed), Point::new(-1., 15.)).is_err());
    assert!(penplot::bucket_region(&[closed], Point::new(0., 15.)).is_err());
}
#[test]
fn all_patterns_clip_holes_density_increases_drawing_and_roundtrip_matches() {
    let region = penplot::bucket_region(
        &[ring(0., 0., 40., 30.), ring(10., 10., 10., 10.)],
        Point::new(5., 5.),
    )
    .unwrap();
    for pattern in [Pattern::Dots, Pattern::Diagonal, Pattern::Cross] {
        let low = planned(&region, fill(pattern, 20.));
        let high = planned(&region, fill(pattern, 70.));
        let length = |paths: &Vec<Stroke>| {
            paths
                .iter()
                .flat_map(|s| s.points.windows(2))
                .map(|p| p[0].distance(p[1]))
                .sum::<f64>()
        };
        assert!(length(&high) > length(&low) * 1.5, "{pattern:?}");
        assert!(high.iter().all(|s| !s.filled && !s.closed));
        for s in &high {
            for pair in s.points.windows(2) {
                for i in 1..10 {
                    let t = i as f64 / 10.;
                    let p = Point::new(
                        pair[0].x + (pair[1].x - pair[0].x) * t - 5.,
                        pair[0].y + (pair[1].y - pair[0].y) * t - 5.,
                    );
                    assert!(penplot::region_contains(&region, p), "{pattern:?} {p:?}");
                }
            }
        }
        let project = Project {
            layers: vec![ProjectLayer {
                name: "Bucket fill".into(),
                visible: true,
                strokes: region.clone(),
                raster: Some(fill(pattern, 70.)),
                text: None,
            }],
            options: JobOptions {
                x: 5.,
                y: 5.,
                ..Default::default()
            },
            profile: Profile::default(),
            import_options: ImportOptions::default(),
        };
        let bytes = project.to_json().unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
            5
        );
        let restored = Project::from_json(&bytes).unwrap();
        assert_eq!(restored, project);
        let routes = planned(
            &restored.layers[0].strokes,
            restored.layers[0].raster.clone().unwrap(),
        );
        assert_eq!(routes, high);
        let output = penplot::generate(&high, &project.profile).unwrap();
        assert!(output.draw_mm > 0.);
        assert!(!output.gcode.contains("G28"));
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("bucket.penplot");
        let target = dir.path().join("PLOT.GCODE");
        std::fs::write(&input, bytes).unwrap();
        let run = std::process::Command::new(env!("CARGO_BIN_EXE_penplot"))
            .args(["convert", "--calibrated", "-o"])
            .arg(&target)
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(std::fs::read_to_string(target).unwrap(), output.gcode);
    }
}
#[test]
fn bucket_patterns_are_not_removed_by_image_noise_filter_and_old_documents_stay_v2() {
    let region = penplot::bucket_region(&[ring(0., 0., 20., 20.)], Point::new(10., 10.)).unwrap();
    let mut source = vec![];
    penplot::append_layer(&mut source, &region, 0).unwrap();
    let options = JobOptions {
        min_image_stroke_mm: 1000.,
        ..Default::default()
    };
    let result = penplot::plan_with_raster(
        &source,
        &options,
        &[(0, fill(Pattern::Dots, 35.))].into_iter().collect(),
    )
    .unwrap();
    assert!(!result.strokes.is_empty());
    assert_eq!(result.removed_count, 0);
    let old = Project {
        layers: vec![],
        options: JobOptions::default(),
        profile: Profile::default(),
        import_options: ImportOptions::default(),
    };
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&old.to_json().unwrap()).unwrap()["version"],
        2
    );
    let mut bad = PatternSettings {
        density: f64::NAN,
        ..Default::default()
    };
    assert!(bad.validate().is_err());
    bad.density = 101.;
    assert!(bad.validate().is_err());
}

#[test]
fn pattern_on_paper_edge_exports_without_negative_roundoff() {
    let source = penplot::import_bytes(
        "bucket-card.svg",
        include_bytes!("../examples/bucket-card.svg"),
        &ImportOptions::default(),
    )
    .unwrap()
    .strokes;
    for pattern in [Pattern::Diagonal, Pattern::Cross] {
        let region = penplot::bucket_region(&source, Point::new(42., 20.)).unwrap();
        let mut boundaries = Vec::new();
        penplot::append_layer(&mut boundaries, &region, 0).unwrap();
        let paths = penplot::plan_with_raster(
            &boundaries,
            &JobOptions::default(),
            &[(0, fill(pattern, 50.))].into_iter().collect(),
        )
        .unwrap()
        .strokes;
        assert!(
            paths
                .iter()
                .flat_map(|s| &s.points)
                .all(|p| p.x >= 0. && p.y >= 0.)
        );
        penplot::generate(&paths, &Profile::default()).unwrap();
    }
}
