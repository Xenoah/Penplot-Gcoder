use image::{Rgba, RgbaImage};
use penplot::*;
fn image_bytes() -> Vec<u8> {
    let mut im = RgbaImage::from_pixel(120, 80, Rgba([255, 255, 255, 255]));
    for x in 5..110 {
        im.put_pixel(x, 8, Rgba([0, 0, 0, 255]));
    }
    for y in 30..38 {
        for x in 20..100 {
            im.put_pixel(x, y, Rgba([0, 0, 0, 255]));
        }
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    im.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}
#[test]
fn image_pen_width_removes_long_thin_lines_in_outline_centerline_and_clean_modes() {
    let bytes = image_bytes();
    for (mode, trace) in [
        (RasterMode::Threshold, TraceMode::Outline),
        (RasterMode::Threshold, TraceMode::Centerline),
        (RasterMode::CleanLine, TraceMode::Centerline),
        (RasterMode::Color, TraceMode::Outline),
    ] {
        let mut options = ImportOptions {
            dpi: 254.,
            trace,
            ..Default::default()
        };
        options.processing.mode = mode;
        let old = import_bytes("ink.png", &bytes, &options).unwrap();
        assert!(old.strokes.iter().flat_map(|s| &s.points).any(|p| p.y > 7.));
        options.processing.pen_width_mm = 0.4;
        let filtered = import_bytes("ink.png", &bytes, &options).unwrap();
        assert!(!filtered.strokes.is_empty());
        assert!(
            filtered
                .strokes
                .iter()
                .flat_map(|s| &s.points)
                .all(|p| p.y < 6.)
        );
    }
}
#[test]
fn project_width_roundtrip_and_cli_output_match_and_legacy_format_is_unchanged() {
    let source = import_bytes(
        "card.svg",
        include_bytes!("../examples/bucket-card.svg"),
        &ImportOptions::default(),
    )
    .unwrap();
    let region = bucket_region(&source.strokes, Point::new(40., 20.)).unwrap();
    let settings = RasterSettings {
        outline: false,
        fill: Fill::Lines,
        pen_width_mm: 0.5,
        ..Default::default()
    };
    let project = Project {
        layers: vec![ProjectLayer {
            name: "fill".into(),
            visible: true,
            strokes: region,
            raster: Some(settings.clone()),
            text: None,
        }],
        options: JobOptions::default(),
        profile: Profile::default(),
        import_options: ImportOptions::default(),
    };
    let bytes = project.to_json().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["version"],
        6
    );
    let restored = Project::from_json(&bytes).unwrap();
    assert_eq!(restored, project);
    let mut strokes = Vec::new();
    append_layer(&mut strokes, &restored.layers[0].strokes, 0).unwrap();
    let paths = plan_with_raster(
        &strokes,
        &restored.options,
        &[(0, settings)].into_iter().collect(),
    )
    .unwrap()
    .strokes;
    let expected = generate(&paths, &restored.profile).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("p.penplot");
    let output = dir.path().join("PLOT.GCODE");
    std::fs::write(&input, bytes).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_penplot"))
        .args(["convert", "--calibrated", "-o"])
        .arg(&output)
        .arg(input)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read_to_string(output).unwrap(), expected.gcode);
    let mut old = project;
    old.layers.clear();
    let bytes = old.to_json().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["version"], 2);
    assert!(!String::from_utf8(bytes).unwrap().contains("pen_width"));
}
