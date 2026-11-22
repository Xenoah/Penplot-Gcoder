//! Deterministic, offline G-code export for a manually homed Cartesian machine.
use crate::{Point, Stroke};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Write as _;

const SCHEMA: u32 = 1;
const MAX_PROFILE_BYTES: usize = 256 * 1024;
const MAX_POINTS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    LeftBottom,
    Center,
}

/// Paper coordinates are always left-bottom based. Center origin subtracts
/// half the bed size from the commanded X/Y after adding the pen offsets.
/// Limits and Z values are user-measured, never inferred from a firmware name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    version: u32,
    pub name: String,
    pub bed_x: f64,
    pub bed_y: f64,
    pub max_z: f64,
    pub origin: Origin,
    pub offset_x: f64,
    pub offset_y: f64,
    pub offset_z: f64,
    pub pen_up_z: f64,
    pub pen_down_z: f64,
    /// All speeds in the profile are in mm/s; emitted G-code F is mm/min.
    pub draw_speed: f64,
    pub travel_speed: f64,
    pub pen_up_speed: f64,
    pub pen_down_speed: f64,
    pub touchdown_delay: u32,
    pub liftup_delay: u32,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            version: SCHEMA,
            name: "Demo (measure before use)".to_owned(),
            bed_x: 100.0,
            bed_y: 100.0,
            max_z: 10.0,
            origin: Origin::LeftBottom,
            offset_x: 0.0,
            offset_y: 0.0,
            offset_z: 0.0,
            pen_up_z: 5.0,
            pen_down_z: 2.0,
            draw_speed: 10.0,
            travel_speed: 20.0,
            pen_up_speed: 2.0,
            pen_down_speed: 2.0,
            touchdown_delay: 0,
            liftup_delay: 0,
        }
    }
}

impl Profile {
    /// The bed and all motion values are starting points, not verified K9 limits.
    pub fn k9() -> Self {
        Self {
            name: "EasyThreed K9 (manual HOME; measure before use)".to_owned(),
            ..Self::default()
        }
    }

    pub fn from_json(text: &str) -> Result<(Self, Vec<String>), String> {
        if text.len() > MAX_PROFILE_BYTES {
            return Err("Profile exceeds 256 KiB".into());
        }
        let text = text.trim_start_matches('\u{feff}');
        let value: Value = serde_json::from_str(text).map_err(|e| format!("Invalid JSON: {e}"))?;
        let obj = value.as_object().ok_or("Profile must be a JSON object")?;
        if obj.contains_key("version") {
            // Deserialize the original JSON to reject duplicate versioned fields.
            let profile: Self =
                serde_json::from_str(text).map_err(|e| format!("Invalid profile: {e}"))?;
            profile.validate()?;
            return Ok((profile, Vec::new()));
        }
        let machine = match obj.get("machine") {
            Some(v) if v.is_object() => v,
            Some(_) => return Err("Legacy machine settings must be an object".into()),
            None => &value,
        };
        let pen = obj.get("pen");
        if pen.is_some_and(|v| !v.is_object()) {
            return Err("Legacy pen settings must be an object".into());
        }
        let name = obj
            .get("name")
            .or_else(|| machine.get("profile_name"))
            .and_then(Value::as_str)
            .unwrap_or("Legacy profile");
        let mut profile = if name.eq_ignore_ascii_case("k9") {
            Self::k9()
        } else {
            Self {
                name: name.to_owned(),
                ..Self::default()
            }
        };
        profile.bed_x = machine
            .get("bed_x")
            .and_then(Value::as_f64)
            .ok_or("Legacy bed_x is missing or invalid")?;
        profile.bed_y = machine
            .get("bed_y")
            .and_then(Value::as_f64)
            .ok_or("Legacy bed_y is missing or invalid")?;
        profile.origin = match machine.get("origin").and_then(Value::as_str) {
            None if machine.get("origin").is_none() => Origin::LeftBottom,
            Some("left_bottom") => Origin::LeftBottom,
            Some("center") => Origin::Center,
            _ => return Err("Unsupported legacy origin".into()),
        };
        if let Some(pen) = pen {
            for (key, output) in [
                ("offset_x", &mut profile.offset_x),
                ("offset_y", &mut profile.offset_y),
            ] {
                if let Some(v) = pen.get(key) {
                    *output = v
                        .as_f64()
                        .ok_or_else(|| format!("Invalid legacy pen {key}"))?;
                }
            }
        }
        profile.validate()?;
        let mut warnings = vec![
            "Legacy profile: imported bed size, origin, and XY offsets only. Re-measure Z and verify all limits and speeds before export.".to_owned()
        ];
        if obj.contains_key("gcode")
            || obj.contains_key("speed")
            || obj.contains_key("pen")
            || obj.contains_key("path")
            || obj.contains_key("fill")
        {
            warnings.push("Legacy templates, lift settings, feed rates, firmware overrides, placement and fill settings are not carried over.".into());
        }
        Ok((profile, warnings))
    }

    pub fn effective_area(&self) -> (f64, f64, f64, f64) {
        (
            0.0_f64.max(-self.offset_x),
            0.0_f64.max(-self.offset_y),
            self.bed_x.min(self.bed_x - self.offset_x),
            self.bed_y.min(self.bed_y - self.offset_y),
        )
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != SCHEMA {
            return Err(format!("Unsupported profile version {}", self.version));
        }
        if self.name.trim().is_empty()
            || self.name.chars().count() > 160
            || self.name.chars().any(char::is_control)
        {
            return Err("Profile name must have 1–160 printable characters".into());
        }
        for (label, value) in [
            ("bed width", self.bed_x),
            ("bed depth", self.bed_y),
            ("maximum Z", self.max_z),
        ] {
            if !(0.001..=10_000.0).contains(&value) {
                return Err(format!("Invalid {label}: specify 0.001–10000 mm"));
            }
        }
        for (label, value, max) in [
            ("draw speed", self.draw_speed, 300.0),
            ("travel speed", self.travel_speed, 300.0),
            ("lift speed", self.pen_up_speed, 30.0),
            ("lowering speed", self.pen_down_speed, 30.0),
        ] {
            if !(0.001..=max).contains(&value) {
                return Err(format!("Invalid {label}: specify 0.001–{max} mm/s"));
            }
        }
        if self.touchdown_delay > 60_000 || self.liftup_delay > 60_000 {
            return Err("Pen delays must be between 0 and 60000 ms".into());
        }
        if !self.offset_x.is_finite() || !self.offset_y.is_finite() {
            return Err("XY offsets must be finite".into());
        }
        let (x0, y0, x1, y1) = self.effective_area();
        if x1 - x0 < 0.001 || y1 - y0 < 0.001 {
            return Err("XY offsets leave no usable drawing area".into());
        }
        if !(-10_000.0..=10_000.0).contains(&self.offset_z) {
            return Err("Z offset must be finite and within ±10000 mm".into());
        }
        let (up, down) = (
            self.pen_up_z + self.offset_z,
            self.pen_down_z + self.offset_z,
        );
        for z in [up, down] {
            if !(0.0..=self.max_z).contains(&z) || !(0.0..=self.max_z).contains(&rounded(z)) {
                return Err("Commanded Z heights, including offset and rounding, must be within the Z limits".into());
            }
        }
        if up <= down || rounded(up) <= rounded(down) {
            return Err("Pen-up Z must exceed pen-down Z at 0.001 mm command resolution".into());
        }
        Ok(())
    }

    fn command_point(&self, point: Point) -> Result<Point, String> {
        let (x0, y0, x1, y1) = self.effective_area();
        // Permit arithmetic noise at edges (e.g. rotating by 90 degrees), never
        // more than one millionth of the command resolution.
        const EPS: f64 = 1e-9;
        if !(x0 - EPS..=x1 + EPS).contains(&point.x) || !(y0 - EPS..=y1 + EPS).contains(&point.y) {
            return Err(format!(
                "({:.3}, {:.3}) mm is outside the effective drawing area",
                point.x, point.y
            ));
        }
        let (cx, cy) = match self.origin {
            Origin::LeftBottom => (0.0, 0.0),
            Origin::Center => (self.bed_x / 2.0, self.bed_y / 2.0),
        };
        let limits = (self.bed_x - cx, self.bed_y - cy);
        let x = (point.x.clamp(x0, x1) + self.offset_x - cx).clamp(-cx, limits.0);
        let y = (point.y.clamp(y0, y1) + self.offset_y - cy).clamp(-cy, limits.1);
        let command = Point::new(rounded(x), rounded(y));
        if !(-cx..=limits.0).contains(&command.x) || !(-cy..=limits.1).contains(&command.y) {
            return Err("An XY command would exceed the machine limits after rounding".into());
        }
        let paper_x = command.x + cx - self.offset_x;
        let paper_y = command.y + cy - self.offset_y;
        if !(-EPS..=self.bed_x + EPS).contains(&paper_x)
            || !(-EPS..=self.bed_y + EPS).contains(&paper_y)
        {
            return Err("The pen would exceed the bed after XY rounding".into());
        }
        Ok(command)
    }
}

#[derive(Debug)]
pub struct Output {
    pub gcode: String,
    pub draw_mm: f64,
    /// Travel from one stroke to the next. The initial XY is unknown.
    pub travel_mm: f64,
    /// Nominal movement and dwell only; excludes initial XY/Z, acceleration,
    /// firmware overhead, and any homing done manually before running.
    pub seconds: f64,
}

fn rounded(value: f64) -> f64 {
    let v = (value * 1000.0).round() / 1000.0;
    if v == 0.0 { 0.0 } else { v }
}

fn xy_line(out: &mut String, point: Point, speed: f64) {
    writeln!(
        out,
        "G1 X{:.3} Y{:.3} F{:.3}",
        point.x,
        point.y,
        speed * 60.0
    )
    .expect("writing to String");
}

fn z_line(out: &mut String, height: f64, speed: f64) {
    writeln!(out, "G1 Z{height:.3} F{:.3}", speed * 60.0).expect("writing to String");
}

fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

/// Validates every coordinate before returning an export. Degenerate paths
/// after machine rounding are ignored; separate strokes are never joined.
/// Assumes the operator verified limits, manually homed, and cleared travel.
pub fn generate(strokes: &[Stroke], profile: &Profile) -> Result<Output, String> {
    generate_job(strokes, profile, false)
}

/// Uses the same XY geometry and feeds, but never lowers the pen. Full profile
/// validation and manual homing are still required.
pub fn generate_air_run(strokes: &[Stroke], profile: &Profile) -> Result<Output, String> {
    generate_job(strokes, profile, true)
}

fn generate_job(strokes: &[Stroke], profile: &Profile, air_run: bool) -> Result<Output, String> {
    profile.validate()?;
    let mut remaining = MAX_POINTS;
    let up = rounded(profile.pen_up_z + profile.offset_z);
    let down = rounded(profile.pen_down_z + profile.offset_z);
    let mut result = Output {
        gcode: if air_run {
            "; AIR RUN: maintains pen-up Z throughout\n"
        } else {
            ""
        }
        .to_owned(),
        draw_mm: 0.0,
        travel_mm: 0.0,
        seconds: 0.0,
    };
    result
        .gcode
        .push_str("; Penplot-Gcoder / manual HOME before run\nG21\nG90\n");
    z_line(&mut result.gcode, up, profile.pen_up_speed);
    let mut current = None;
    for (index, stroke) in strokes.iter().enumerate() {
        remaining = remaining
            .checked_sub(stroke.points.len().saturating_add(1))
            .ok_or("Export exceeds the point limit")?;
        let mut points = Vec::with_capacity(stroke.points.len());
        for (point_index, &point) in stroke.points.iter().enumerate() {
            let command = profile
                .command_point(point)
                .map_err(|e| format!("Path {}, point {}: {e}", index + 1, point_index + 1))?;
            if points
                .last()
                .copied()
                .is_some_and(|last| distance(last, command) == 0.0)
            {
                continue;
            }
            points.push(command);
        }
        if points.len() < 2 {
            continue;
        }
        let first = points[0];
        if stroke.closed && distance(first, *points.last().expect("nonempty")) != 0.0 {
            points.push(first);
        }

        if let Some(previous) = current {
            result.travel_mm += distance(previous, first);
        }
        xy_line(&mut result.gcode, first, profile.travel_speed);
        if !air_run {
            z_line(&mut result.gcode, down, profile.pen_down_speed);
        }
        if profile.touchdown_delay != 0 {
            writeln!(result.gcode, "G4 P{}", profile.touchdown_delay).expect("writing to String");
        }
        for segment in points.windows(2) {
            result.draw_mm += distance(segment[0], segment[1]);
            xy_line(&mut result.gcode, segment[1], profile.draw_speed);
        }
        if profile.liftup_delay != 0 {
            writeln!(result.gcode, "G4 P{}", profile.liftup_delay).expect("writing to String");
        }
        z_line(&mut result.gcode, up, profile.pen_up_speed);
        result.seconds += f64::from(profile.touchdown_delay + profile.liftup_delay) / 1000.0;
        if !air_run {
            result.seconds +=
                (up - down) / profile.pen_up_speed + (up - down) / profile.pen_down_speed;
        }
        current = points.last().copied();
    }
    if current.is_none() {
        return Err("No drawable strokes at command resolution".into());
    }
    result.seconds += result.draw_mm / profile.draw_speed + result.travel_mm / profile.travel_speed;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(coords: &[(f64, f64)], closed: bool) -> Stroke {
        Stroke {
            points: coords.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            closed,
            layer: "Example".into(),
            group: 0,
            filled: false,
        }
    }

    #[test]
    fn disjoint_paths_and_eof_are_lifted_and_output_is_reproducible() {
        let profile = Profile {
            offset_x: 1.0,
            offset_y: -1.0,
            offset_z: 0.5,
            touchdown_delay: 20,
            liftup_delay: 25,
            ..Profile::default()
        };
        let paths = [
            stroke(&[(0.0, 1.0), (3.0, 1.0)], false),
            stroke(&[(7.0, 1.0), (7.0, 7.0)], false),
        ];
        let output = generate(&paths, &profile).unwrap();
        assert_eq!(
            output.gcode,
            concat!(
                "; Penplot-Gcoder / manual HOME before run\n",
                "G21\nG90\n",
                "G1 Z5.500 F120.000\n",
                "G1 X1.000 Y0.000 F1200.000\n",
                "G1 Z2.500 F120.000\n",
                "G4 P20\n",
                "G1 X4.000 Y0.000 F600.000\n",
                "G4 P25\n",
                "G1 Z5.500 F120.000\n",
                "G1 X8.000 Y0.000 F1200.000\n",
                "G1 Z2.500 F120.000\n",
                "G4 P20\n",
                "G1 X8.000 Y6.000 F600.000\n",
                "G4 P25\n",
                "G1 Z5.500 F120.000\n",
            )
        );
        assert_eq!(output.draw_mm, 9.0);
        assert_eq!(output.travel_mm, 4.0);
        assert!((output.seconds - 7.19).abs() < 1e-8);
        assert_eq!(output.gcode, generate(&paths, &profile).unwrap().gcode);
    }

    #[test]
    fn air_run_uses_same_xy_and_ends_at_up_height() {
        let profile = Profile::default();
        let paths = [stroke(&[(1.0, 4.0), (3.0, 4.0)], false)];
        let regular = generate(&paths, &profile).unwrap();
        let air = generate_air_run(&paths, &profile).unwrap();
        let xy = |g: &str| {
            g.lines()
                .filter(|l| l.starts_with("G1 X"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(xy(&regular.gcode), xy(&air.gcode));
        assert!(air.gcode.starts_with("; AIR RUN:"));
        assert!(air.gcode.ends_with("G1 Z5.000 F120.000\n"));
        assert!(
            air.gcode
                .lines()
                .filter(|l| l.starts_with("G1 Z"))
                .all(|l| l.starts_with("G1 Z5.000"))
        );
        assert!((air.seconds - 0.2).abs() < 1e-8);
    }

    #[test]
    fn invalid_art_or_offsets_are_blocked_even_if_only_one_is_out_of_bounds() {
        let profile = Profile {
            offset_x: 10.0,
            offset_y: -10.0,
            ..Profile::default()
        };
        assert_eq!(profile.effective_area(), (0.0, 10.0, 90.0, 100.0));
        let generate_for = |start| generate(&[stroke(&[start, (50.0, 50.0)], false)], &profile);
        assert!(generate_for((-1.0, 55.0)).is_err()); // paper only
        assert!(generate_for((91.0, 55.0)).is_err()); // machine only
        assert!(generate_for((45.0, 9.99)).is_err());
        assert!(generate_for((45.0, 101.0)).is_err());
        assert!(generate_for((f64::NAN, 45.0)).is_err());
        assert!(generate_for((0.0, 100.0)).is_ok());
        assert!(generate_for((90.0, 10.0)).is_ok());
    }

    #[test]
    fn center_origin_is_applied_only_to_machine_commands() {
        let profile = Profile {
            origin: Origin::Center,
            offset_x: 10.0,
            ..Profile::default()
        };
        let output = generate(&[stroke(&[(0.0, 0.0), (90.0, 100.0)], false)], &profile).unwrap();
        assert!(output.gcode.contains("G1 X-40.000 Y-50.000 F1200.000"));
        assert!(output.gcode.contains("G1 X50.000 Y50.000 F600.000"));
        assert_eq!(profile.effective_area(), (0.0, 0.0, 90.0, 100.0));
    }

    #[test]
    fn rounded_commands_cannot_exceed_configured_machine_limits() {
        let profile = Profile {
            bed_x: 56.9999,
            ..Profile::default()
        };
        let paths = [stroke(&[(0.0, 0.0), (56.9999, 5.0)], false)];
        assert!(
            generate(&paths, &profile)
                .unwrap_err()
                .contains("after rounding")
        );
        let rounding_with_offset = Profile {
            offset_x: 0.0004,
            ..Profile::default()
        };
        assert!(
            generate(&paths, &rounding_with_offset)
                .unwrap_err()
                .contains("pen would exceed the bed")
        );
        let too_high = Profile {
            max_z: 4.9999,
            pen_up_z: 4.9999,
            ..Profile::default()
        };
        assert!(too_high.validate().is_err());
        let collapsed = Profile {
            pen_up_z: 2.0001,
            ..Profile::default()
        };
        assert!(collapsed.validate().is_err());
        let unsafe_z = Profile {
            offset_z: -2.1,
            ..Profile::default()
        };
        assert!(unsafe_z.validate().is_err());
    }

    #[test]
    fn closure_works_with_or_without_an_explicit_duplicate() {
        let profile = Profile::default();
        let a = generate(
            &[stroke(&[(10.0, 10.0), (20.0, 10.0), (20.0, 20.0)], true)],
            &profile,
        )
        .unwrap();
        let b = generate(
            &[stroke(
                &[(10.0, 10.0), (20.0, 10.0), (20.0, 20.0), (10.0, 10.0)],
                true,
            )],
            &profile,
        )
        .unwrap();
        assert_eq!(a.gcode, b.gcode);
        assert!(a.gcode.contains("G1 X10.000 Y10.000 F600.000\nG1 Z"));
    }

    #[test]
    fn degenerate_or_outside_single_point_is_not_emitted() {
        let profile = Profile::default();
        let collapsed = stroke(&[(0.0, 0.0), (0.0001, 0.0002)], false);
        assert!(
            generate(&[collapsed], &profile)
                .unwrap_err()
                .contains("No drawable")
        );
        let outside = stroke(&[(101.0, 101.0)], false);
        assert!(
            generate(&[outside], &profile)
                .unwrap_err()
                .contains("drawing area")
        );
        assert!(generate(&[], &profile).is_err());
    }

    #[test]
    fn migration_for_all_bundled_profiles_and_native_roundtrip() {
        for (raw, (x, y)) in [
            (include_str!("../profiles/printers/K9.json"), (100.0, 100.0)),
            (
                include_str!("../profiles/printers/Ender-3.json"),
                (220.0, 220.0),
            ),
            (
                include_str!("../profiles/printers/Prusa-MK3.json"),
                (250.0, 210.0),
            ),
            (
                include_str!("../profiles/printers/Creality-CR10.json"),
                (300.0, 300.0),
            ),
        ] {
            let (profile, warnings) = Profile::from_json(raw).unwrap();
            assert!(!warnings.is_empty());
            assert_eq!((profile.bed_x, profile.bed_y), (x, y));
            assert_eq!(profile.draw_speed, Profile::default().draw_speed);
            assert_eq!(profile.pen_up_z, Profile::default().pen_up_z);
            let (restored, warnings) =
                Profile::from_json(&serde_json::to_string(&profile).unwrap()).unwrap();
            assert_eq!(profile, restored);
            assert!(warnings.is_empty());
        }
    }

    #[test]
    fn legacy_raw_commands_do_not_reach_the_output() {
        let raw = r#"{"machine":{"bed_x":100,"bed_y":95,"origin":"center"},
            "pen":{"offset_x":3.5,"offset_y":-2.5,"pen_down_z":199},
            "gcode":{"start_code":"G28\nM104 S260","end_code":"M84","pen_up_code":"M280 P0 S180"}}"#;
        let (profile, warnings) = Profile::from_json(raw).unwrap();
        assert_eq!(profile.origin, Origin::Center);
        assert_eq!((profile.offset_x, profile.offset_y), (3.5, -2.5));
        assert_eq!(warnings.len(), 2);
        let output = generate(&[stroke(&[(10.0, 10.0), (11.0, 11.0)], false)], &profile).unwrap();
        for l in output.gcode.lines().filter(|l| !l.starts_with(';')) {
            assert!(matches!(
                l.split_ascii_whitespace().next().unwrap(),
                "G1" | "G21" | "G90" | "G4"
            ));
        }
    }

    #[test]
    fn malformed_profiles_and_bad_parameters_fail_explicitly() {
        for raw in [
            "",
            "[]",
            "{}",
            r#"{"bed_x": 100, "bed_y": 100, "origin": null}"#,
            r#"{"bed_x": 100, "bed_y": "100"}"#,
            r#"{"version": 2, "bed_x": 100, "bed_y": 100}"#,
        ] {
            assert!(Profile::from_json(raw).is_err(), "accepted {raw}");
        }
        for profile in [
            Profile {
                draw_speed: f64::NAN,
                ..Profile::default()
            },
            Profile {
                pen_down_speed: 0.0,
                ..Profile::default()
            },
            Profile {
                touchdown_delay: 60_001,
                ..Profile::default()
            },
            Profile {
                offset_x: 100.0,
                ..Profile::default()
            },
            Profile {
                bed_x: f64::INFINITY,
                ..Profile::default()
            },
        ] {
            assert!(profile.validate().is_err(), "{profile:?}");
        }
    }
}
