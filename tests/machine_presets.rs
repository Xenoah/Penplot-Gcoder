use penplot::{
    MachinePreset, Point, Profile, Stroke, generate, generate_air_run, generate_with_travel,
};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn paths() -> [Stroke; 1] {
    [Stroke {
        points: vec![Point::new(20.0, 30.0), Point::new(40.0, 30.0)],
        layer: "Sample".into(),
        group: 0,
        closed: false,
        filled: false,
    }]
}

#[test]
fn named_presets_round_trip_and_default_heights_are_not_commandable() {
    for (preset, size) in [
        (MachinePreset::BambuP1s, 256.0),
        (MachinePreset::VoxelabAries, 200.0),
    ] {
        let p = Profile::for_machine(preset);
        assert_eq!((p.bed_x, p.bed_y, p.max_z), (size, size, size));
        assert_eq!((p.pen_down_z, p.pen_up_z), (0.0, 0.0));
        assert!(p.check_layout(&paths()).is_ok());
        let raw = serde_json::to_string(&p).unwrap();
        let (reloaded, warnings) = Profile::from_json(&raw).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(p, reloaded);
        assert!(generate(&paths(), &reloaded).is_err());
        assert!(generate_air_run(&paths(), &reloaded).is_err());
    }
    let k9 = Profile::k9();
    assert!(
        !serde_json::to_string(&k9)
            .unwrap()
            .contains("machine_preset")
    );
    assert_eq!(k9.travel_speed, 40.0);
    assert!(generate(&paths(), &k9).is_ok());
}

#[test]
fn bambu_export_gate_survives_profile_renaming_and_valid_abs_z_values() {
    let mut p = Profile::for_machine(MachinePreset::BambuP1s);
    p.pen_down_z = 2.0;
    p.pen_up_z = 6.0;
    p.name = "Renamed custom-looking file".into();
    assert!(p.validate().is_ok());
    assert!(generate(&paths(), &p).unwrap_err().contains("layout-only"));
    assert!(
        generate_air_run(&paths(), &p)
            .unwrap_err()
            .contains("layout-only")
    );
    p.bed_y = 256.1;
    assert!(p.check_layout(&paths()).unwrap_err().contains("nominal"));
}

#[test]
fn aries_uses_two_absolute_targets_in_the_same_frame_and_no_startup_or_dwell() {
    let mut p = Profile::for_machine(MachinePreset::VoxelabAries);
    p.pen_down_z = 2.0;
    p.pen_up_z = 6.0;
    p.offset_z = 0.5;
    let job = generate(&paths(), &p).unwrap();
    let commands: Vec<_> = job.gcode.lines().filter(|s| !s.starts_with(';')).collect();
    assert_eq!(
        commands,
        [
            "G21",
            "G90",
            "G1 Z6.500 F120.000",
            "G1 X20.000 Y30.000 F1200.000",
            "G1 Z2.500 F120.000",
            "G1 X40.000 Y30.000 F600.000",
            "G1 Z6.500 F120.000",
        ]
    );
    assert!(commands.iter().all(|line| matches!(
        line.split_ascii_whitespace().next(),
        Some("G21" | "G90" | "G1")
    )));
    let run = generate_air_run(&paths(), &p).unwrap();
    assert!(!run.gcode.contains("Z2.500"));
    assert!(run.gcode.ends_with("G1 Z6.500 F120.000\n"));
    p.touchdown_delay = 1;
    assert!(generate(&paths(), &p).unwrap_err().contains("G4"));
    p.touchdown_delay = 0;
    p.pen_up_z = 2.0001;
    assert!(
        generate_air_run(&paths(), &p)
            .unwrap_err()
            .contains("resolution")
    );
}

#[test]
fn cli_creates_presets_and_does_not_write_unapproved_machine_files() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path();
    let sample = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/alignment-card.svg");
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_penplot"))
            .current_dir(work)
            .args(args)
            .output()
            .unwrap()
    };
    let art = sample.to_str().unwrap();
    for (profile, flag, why) in [
        ("p1s.json", "--p1s", "layout-only"),
        ("aries.json", "--aries", "measured Draw Z"),
    ] {
        assert!(run(&["profile-init", profile, flag]).status.success());
        let checked = run(&["check", "--profile", profile, "--fit", "5", art]);
        assert!(checked.status.success(), "{checked:?}");
        assert!(String::from_utf8_lossy(&checked.stdout).contains("layout only"));
        let blocked = run(&[
            "convert",
            "--profile",
            profile,
            "--calibrated",
            "--air-run",
            "--fit",
            "5",
            "-o",
            "must-not-exist.gcode",
            art,
        ]);
        assert!(!blocked.status.success());
        assert!(String::from_utf8_lossy(&blocked.stderr).contains(why));
        assert!(!work.join("must-not-exist.gcode").exists());
    }
    let mut measured = Profile::for_machine(MachinePreset::VoxelabAries);
    measured.pen_down_z = 3.0;
    measured.pen_up_z = 8.0;
    fs::write(
        work.join("measured.json"),
        serde_json::to_vec(&measured).unwrap(),
    )
    .unwrap();
    let made = run(&[
        "convert",
        "--profile",
        "measured.json",
        "--calibrated",
        "--air-run",
        "--fit",
        "95",
        "-o",
        "airrun.gcode",
        art,
    ]);
    assert!(made.status.success(), "{made:?}");
    let gcode = fs::read_to_string(work.join("airrun.gcode")).unwrap();
    assert!(gcode.contains("Z8.000"));
    assert!(!gcode.contains("Z3.000"));
}

#[test]
fn smooth_travel_does_not_bypass_preset_export_z_or_dwell_limits() {
    let mut strokes = paths().to_vec();
    strokes[0].points = vec![Point::new(5., 5.), Point::new(10., 10.)];
    strokes.push(Stroke {
        points: vec![Point::new(30., 10.), Point::new(35., 5.)],
        ..strokes[0].clone()
    });
    let mut p1s = Profile::for_machine(MachinePreset::BambuP1s);
    p1s.pen_down_z = 2.;
    p1s.pen_up_z = 6.;
    p1s.check_layout(&strokes).unwrap();
    for air in [false, true] {
        assert!(
            generate_with_travel(&strokes, &p1s, air, true)
                .unwrap_err()
                .contains("layout-only")
        );
    }
    let mut aries = Profile::for_machine(MachinePreset::VoxelabAries);
    aries.pen_down_z = 2.;
    aries.pen_up_z = 6.;
    aries.offset_z = 0.5;
    let plot = generate_with_travel(&strokes, &aries, false, true).unwrap();
    let dry = generate_with_travel(&strokes, &aries, true, true).unwrap();
    assert_eq!(plot.curved_travel.len(), 1);
    assert_eq!(
        plot.curved_travel[0].paper_points,
        dry.curved_travel[0].paper_points
    );
    assert!(!plot.gcode.lines().any(|line| line.starts_with("G4")));
    assert!(
        dry.gcode
            .lines()
            .filter(|line| line.starts_with("G1 Z"))
            .all(|line| line.starts_with("G1 Z6.500 "))
    );
    aries.liftup_delay = 1;
    assert!(
        generate_with_travel(&strokes, &aries, false, true)
            .unwrap_err()
            .contains("G4")
    );
}
