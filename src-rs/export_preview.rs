//! Read-only inspection of the commands we actually emitted, not the input geometry.
use eframe::egui::{self, Color32, Rect, Sense, Stroke, Vec2};
use penplot::{Origin, Output, Point, Profile};
use std::path::PathBuf;
use std::sync::Arc;

const MAX_RENDER_SEGMENTS: usize = 24_000;
const DRAW: Color32 = Color32::from_rgb(29, 60, 99);
const TRAVEL: Color32 = Color32::from_rgb(209, 104, 20);

#[derive(Debug, Clone, Copy)]
struct Segment {
    a: Point,
    b: Point,
    pen_up: bool,
    color: Color32,
}

#[derive(Debug)]
struct CommandPreview {
    bed: Point,
    segments: Vec<Segment>,
    down_count: usize,
    up_count: usize,
    stride: usize,
}

impl CommandPreview {
    fn from_commands(gcode: &str, profile: &Profile, air_run: bool, limit: usize) -> Self {
        let bed = Point::new(profile.bed_x, profile.bed_y);
        let (mut add_x, mut add_y) = (-profile.offset_x, -profile.offset_y);
        if profile.origin == Origin::Center {
            add_x += bed.x * 0.5;
            add_y += bed.y * 0.5;
        }
        let up = (profile.clearance_z() * 1000.0).round() / 1000.0;
        let xy_count = gcode
            .lines()
            .filter(|line| line.starts_with("G1 X"))
            .count();
        let stride = if xy_count <= limit {
            1
        } else {
            // Separate pen-state sampling can round up at most one extra segment.
            xy_count.div_ceil(limit.saturating_sub(1).max(1)).max(1)
        };
        let mut result = Self {
            bed,
            segments: Vec::with_capacity(xy_count.div_ceil(stride).saturating_add(1)),
            down_count: 0,
            up_count: 0,
            stride,
        };
        let mut previous = None;
        let mut pen_up = true;
        let mut color = DRAW;
        for line in gcode.lines() {
            if let Some(ink) = line.strip_prefix("; INK ") {
                if let Some([r, g, b]) = penplot::ink_rgb(ink) {
                    color = Color32::from_rgb(r, g, b);
                }
            } else if let Some(z) = line.strip_prefix("G1 Z") {
                if let Some(z) = z
                    .split_ascii_whitespace()
                    .next()
                    .and_then(|p| p.parse::<f64>().ok())
                {
                    pen_up = air_run || z >= up;
                }
            } else if let Some(xy) = line.strip_prefix("G1 X") {
                let mut items = xy.split_ascii_whitespace();
                let x = items.next().and_then(|v| v.parse::<f64>().ok());
                let y = items
                    .next()
                    .and_then(|v| v.strip_prefix('Y'))
                    .and_then(|v| v.parse::<f64>().ok());
                let (Some(x), Some(y)) = (x, y) else {
                    continue;
                };
                if !x.is_finite() || !y.is_finite() {
                    continue;
                }
                let next = Point::new(x + add_x, y + add_y);
                // Before the first XY command the physical carriage location is unknown.
                if let Some(before) = previous.replace(next) {
                    let count = if pen_up {
                        &mut result.up_count
                    } else {
                        &mut result.down_count
                    };
                    if *count % stride == 0 {
                        result.segments.push(Segment {
                            a: before,
                            b: next,
                            pen_up,
                            color,
                        });
                    }
                    *count += 1;
                }
            }
        }
        result
    }

    fn show(&self, ui: &mut egui::Ui, show_travel: bool) {
        let width = ui.available_width().max(100.0);
        let (response, painter) = ui.allocate_painter(Vec2::new(width, 260.0), Sense::hover());
        let scale = ((response.rect.width() - 24.0) / self.bed.x as f32)
            .min((response.rect.height() - 24.0) / self.bed.y as f32);
        let size = Vec2::new(self.bed.x as f32, self.bed.y as f32) * scale;
        let paper = Rect::from_center_size(response.rect.center(), size);
        let map = |p: Point| paper.left_bottom() + Vec2::new(p.x as f32, -p.y as f32) * scale;
        painter.rect_filled(response.rect, 5.0, Color32::from_rgb(37, 45, 52));
        painter.rect_filled(paper, 0.0, Color32::from_rgb(250, 249, 241));
        for pen_up in [true, false] {
            if pen_up && !show_travel {
                continue;
            }
            let ink = Stroke::new(
                if pen_up { 0.8 } else { 1.25 },
                if pen_up { TRAVEL } else { DRAW },
            );
            for segment in self.segments.iter().filter(|s| s.pen_up == pen_up) {
                painter.line_segment(
                    [map(segment.a), map(segment.b)],
                    if pen_up {
                        ink
                    } else {
                        Stroke::new(ink.width, segment.color)
                    },
                );
            }
        }
        painter.text(
            paper.left_top() + Vec2::new(4.0, 3.0),
            egui::Align2::LEFT_TOP,
            format!("{:.2} × {:.2} mm · +Y ↑", self.bed.x, self.bed.y),
            egui::FontId::proportional(11.0),
            Color32::from_rgb(70, 76, 77),
        );
    }
}

pub(crate) struct ExportInspection {
    pub(crate) path: Option<PathBuf>,
    pub(crate) revision: u64,
    pub(crate) air_run: bool,
    pub(crate) commands: Arc<Output>,
    preview: CommandPreview,
    show_travel: bool,
    show_commands: bool,
}

impl ExportInspection {
    pub(crate) fn new(
        path: Option<PathBuf>,
        revision: u64,
        air_run: bool,
        commands: Arc<Output>,
        profile: &Profile,
    ) -> Self {
        let preview =
            CommandPreview::from_commands(&commands.gcode, profile, air_run, MAX_RENDER_SEGMENTS);
        Self {
            path,
            revision,
            air_run,
            commands,
            preview,
            show_travel: air_run,
            show_commands: false,
        }
    }

    pub(crate) fn title(&self) -> &'static str {
        match (self.path.is_some(), self.air_run) {
            (true, true) => "Exported air-run G-code",
            (true, false) => "Exported plot G-code",
            (false, true) => "Generated air-run G-code",
            (false, false) => "Generated plot G-code",
        }
    }

    pub(crate) fn show(&mut self, ui: &mut egui::Ui, current_revision: u64) {
        if let Some(path) = &self.path {
            ui.label(path.display().to_string());
        }
        if self.revision != current_revision {
            ui.small("Snapshot of earlier commands. Current workspace changes are not shown here.");
        }
        let ideal = format_duration(self.commands.seconds);
        if self.air_run {
            ui.label(format!(
                "Air run · pen raised throughout · {:.1} mm XY · ~{ideal}",
                self.commands.draw_mm + self.commands.travel_mm,
            ));
        } else {
            ui.label(format!(
                "{:.1} mm draw · {:.1} mm pen-up travel · ~{ideal}",
                self.commands.draw_mm, self.commands.travel_mm,
            ));
        }
        ui.small("Ideal motion + dwell only; acceleration, firmware overhead and initial XY/Z moves are excluded.");
        ui.horizontal_wrapped(|ui| {
            if !self.air_run {
                ui.label(
                    egui::RichText::new("Pen down: layer color")
                        .color(Color32::from_rgb(117, 184, 244)),
                );
                ui.checkbox(&mut self.show_travel, "Show pen-up moves");
            }
            if self.show_travel {
                ui.label(egui::RichText::new("Orange: pen up").color(TRAVEL));
            }
            ui.checkbox(&mut self.show_commands, "Show G-code text");
            if ui.button("Copy entire file").clicked() {
                ui.ctx().copy_text(self.commands.gcode.clone());
            }
        });
        ui.small(format!(
            "{} pen-down segments · {} pen-up segments",
            self.preview.down_count, self.preview.up_count
        ));
        self.preview.show(ui, self.show_travel);
        if self.preview.stride > 1 {
            ui.small(format!(
                "Large route: showing 1 in {} segments per pen state. The G-code is complete.",
                self.preview.stride
            ));
        }
        if self.show_commands {
            // Generated ASCII, but keep the slice boundary robust to future comments.
            let mut end = self.commands.gcode.len().min(48_000);
            while !self.commands.gcode.is_char_boundary(end) {
                end -= 1;
            }
            let mut snippet = &self.commands.gcode[..end];
            if end != self.commands.gcode.len() {
                ui.small("Text preview limited to the first 48 KB. Copy preserves the full file.");
            }
            egui::ScrollArea::both().max_height(190.0).show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut snippet)
                        .code_editor()
                        .desired_width(f32::INFINITY),
                );
            });
        }
    }
}

pub(crate) fn format_duration(seconds: f64) -> String {
    let s = seconds.round().max(0.0) as u64;
    if s >= 3600 {
        format!("{} h {:02} min {:02} s", s / 3600, (s % 3600) / 60, s % 60)
    } else if s >= 60 {
        format!("{} min {:02} s", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use penplot::Stroke;

    fn path(points: &[(f64, f64)]) -> Stroke {
        Stroke {
            points: points.iter().map(|&(x, y)| Point::new(x, y)).collect(),
            closed: false,
            filled: false,
            group: 0,
            layer: "test".into(),
        }
    }

    #[test]
    fn command_preview_uses_emitted_black_red_and_blue_pen_comments() {
        let profile = Profile::default();
        let paths: Vec<_> = ["#000000", "#DC4848", "#377DE1"]
            .into_iter()
            .enumerate()
            .map(|(i, layer)| penplot::Stroke {
                points: vec![
                    Point::new(10. + 10. * i as f64, 10.),
                    Point::new(15. + 10. * i as f64, 10.),
                ],
                layer: layer.into(),
                group: 0,
                closed: false,
                filled: false,
            })
            .collect();
        let out = penplot::generate(&paths, &profile).unwrap();
        let preview =
            CommandPreview::from_commands(&out.gcode, &profile, false, MAX_RENDER_SEGMENTS);
        let colors: Vec<_> = preview
            .segments
            .iter()
            .filter(|s| !s.pen_up)
            .map(|s| s.color)
            .collect();
        assert_eq!(
            colors,
            vec![
                Color32::BLACK,
                Color32::from_rgb(220, 72, 72),
                Color32::from_rgb(55, 125, 225)
            ]
        );
    }

    #[test]
    fn emitted_preview_uses_rounded_commands_and_inverse_center_origin() {
        let mut profile = Profile::default();
        profile.origin = Origin::Center;
        profile.offset_x = 1.0;
        profile.offset_y = -0.5;
        profile.offset_z = 0.8;
        let paths = [
            path(&[(5.1234, 4.5678), (5.1234001, 4.5678), (8.9876, 10.2345)]),
            path(&[(10.0, 11.0), (11.0, 11.0)]),
        ];
        let plot = penplot::generate(&paths, &profile).unwrap();
        let air = penplot::generate_air_run(&paths, &profile).unwrap();
        let proof = CommandPreview::from_commands(&plot.gcode, &profile, false, 99);
        let air_proof = CommandPreview::from_commands(&air.gcode, &profile, true, 99);
        assert_eq!((proof.down_count, proof.up_count), (2, 1));
        assert_eq!((air_proof.down_count, air_proof.up_count), (0, 3));
        assert!((proof.segments[0].a.x - 5.123).abs() < 1e-9);
        assert!((proof.segments[0].a.y - 4.568).abs() < 1e-9);
        for (draw, dry) in proof.segments.iter().zip(&air_proof.segments) {
            assert_eq!((draw.a, draw.b), (dry.a, dry.b));
            assert!(dry.pen_up);
        }
    }

    #[test]
    fn preview_matches_rounded_curve_in_plot_and_air_modes() {
        let mut profile = Profile::default();
        profile.origin = Origin::Center;
        profile.offset_x = 0.25;
        profile.offset_y = -0.125;
        let paths = [
            path(&[(5., 5.), (10., 10.)]),
            path(&[(30., 10.), (35., 5.)]),
        ];
        let plot = penplot::generate_with_travel(&paths, &profile, false, true).unwrap();
        let air = penplot::generate_with_travel(&paths, &profile, true, true).unwrap();
        let preview = CommandPreview::from_commands(&plot.gcode, &profile, false, 99);
        let dry = CommandPreview::from_commands(&air.gcode, &profile, true, 99);
        let curve = &plot.curved_travel[0].paper_points;
        assert_eq!(preview.down_count, 2);
        assert_eq!(preview.up_count, curve.len() - 1);
        assert_eq!(dry.down_count, 0);
        assert_eq!(dry.up_count, preview.up_count + preview.down_count);
        for (segment, expected) in preview
            .segments
            .iter()
            .filter(|p| p.pen_up)
            .zip(curve.windows(2))
        {
            assert!(segment.a.distance(expected[0]) < 1e-10);
            assert!(segment.b.distance(expected[1]) < 1e-10);
        }
        for (regular, dry) in preview.segments.iter().zip(&dry.segments) {
            assert_eq!((regular.a, regular.b), (dry.a, dry.b));
        }
    }

    #[test]
    fn retains_full_pen_state_counts_when_display_is_sampled() {
        let paths = [path(&[
            (2.0, 2.0),
            (3.0, 3.0),
            (4.0, 2.0),
            (5.0, 3.0),
            (6.0, 2.0),
        ])];
        let profile = Profile::default();
        let plot = penplot::generate(&paths, &profile).unwrap();
        let preview = CommandPreview::from_commands(&plot.gcode, &profile, false, 2);
        assert!(preview.stride > 1);
        assert!(preview.segments.len() <= 2);
        assert_eq!((preview.down_count, preview.up_count), (4, 0));
    }

    #[test]
    fn duration_is_rounded_before_splitting_into_units() {
        assert_eq!(format_duration(26.49), "26 s");
        assert_eq!(format_duration(59.5), "1 min 00 s");
        assert_eq!(format_duration(3661.0), "1 h 01 min 01 s");
    }
}
