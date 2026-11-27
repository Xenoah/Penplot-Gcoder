//! Image import keeps the prior successful outline while a newer threshold is traced.
use super::*;
use penplot::{RasterMode, TraceMode};

pub(super) const DEBOUNCE: Duration = Duration::from_millis(140);

#[derive(Default)]
pub(super) struct RasterImport {
    queue: std::collections::VecDeque<PathBuf>,
    draft: Option<Draft>,
}

type TraceResult = Result<(Imported, Vec<Stroke>), String>;

struct Draft {
    path: PathBuf,
    options: ImportOptions,
    fill: penplot::RasterSettings,
    last_edit: std::time::Instant,
    revision: u64,
    completed: Option<u64>,
    preview: Option<Imported>,
    preview_routes: Vec<Stroke>,
    error: Option<String>,
    worker: Option<(u64, mpsc::Receiver<TraceResult>)>,
}

impl Draft {
    fn new(path: PathBuf, options: ImportOptions) -> Self {
        let now = std::time::Instant::now();
        let mut fill = penplot::RasterSettings {
            pen_width_mm: options.processing.pen_width_mm,
            ..Default::default()
        };
        if options.processing.mode == RasterMode::Color {
            fill.fill = Fill::Lines;
            fill.outline = false;
        }
        Self {
            path,
            options,
            fill,
            last_edit: now.checked_sub(DEBOUNCE).unwrap_or(now),
            revision: 0,
            completed: None,
            preview: None,
            preview_routes: Vec::new(),
            error: None,
            worker: None,
        }
    }

    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.last_edit = std::time::Instant::now();
        self.error = None;
    }

    fn is_current(&self) -> bool {
        self.completed == Some(self.revision)
    }

    fn can_add(&self) -> bool {
        self.is_current()
            && self.error.is_none()
            && self.preview.is_some()
            && self.fill.validate().is_ok()
    }

    fn accept(&mut self, revision: u64, result: Result<Imported, String>) {
        if self.revision != revision {
            return;
        }
        self.completed = Some(revision);
        match result {
            Ok(artwork) => {
                self.preview = Some(artwork);
                self.error = None;
            }
            Err(error) => {
                self.error = Some(error);
                // Keep the last successful image as a labeled reference. Never
                // allow this older result to be added after the latest failure.
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some((revision, worker)) = &self.worker {
            let revision = *revision;
            match worker.try_recv() {
                Ok(result) => {
                    self.worker = None;
                    match result {
                        Ok((item, routes)) => {
                            if self.revision == revision {
                                self.preview_routes = routes;
                            }
                            self.accept(revision, Ok(item));
                        }
                        Err(e) => self.accept(revision, Err(e)),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.accept(
                        revision,
                        Err("Preview worker stopped. Change an option to retry.".into()),
                    );
                }
                Err(mpsc::TryRecvError::Empty) => return,
            }
        }
        if self.is_current() {
            return;
        }
        if let Some(wait) = DEBOUNCE.checked_sub(self.last_edit.elapsed()) {
            ctx.request_repaint_after(wait);
            return;
        }

        let options = self.options.clone();
        let fill = self.fill.clone();
        let revision = self.revision;
        let path = self.path.clone();
        let ctx = ctx.clone();
        let (sender, worker) = mpsc::channel();
        self.worker = Some((revision, worker));
        std::thread::spawn(move || {
            let result = (|| {
                let item = import_one(&path, &options)?;
                let mut source = Vec::new();
                penplot::append_layer(&mut source, &item.artwork.strokes, 0)?;
                let layers = [(0, fill)].into_iter().collect();
                let routes =
                    penplot::plan_with_raster(&source, &JobOptions::default(), &layers)?.strokes;
                Ok((item, routes))
            })();
            let _ = sender.send(result);
            ctx.request_repaint();
        });
    }
}

impl RasterImport {
    pub(super) fn extend(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        self.queue.extend(paths);
    }

    pub(super) fn is_busy(&self) -> bool {
        self.draft.is_some() || !self.queue.is_empty()
    }

    fn cancel_current(&mut self) {
        self.draft = None;
    }

    /// Includes the queued-before-first-frame case: avoid acting on another
    /// layer or opening a second document before the modal becomes visible.
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        defaults: &ImportOptions,
    ) -> Option<(Imported, penplot::RasterSettings)> {
        let ctx = ui.ctx().clone();
        if self.draft.is_none() {
            self.draft = self
                .queue
                .pop_front()
                .map(|path| Draft::new(path, defaults.clone()));
        }
        let draft = self.draft.as_mut()?;
        let mut add = false;
        let mut cancel = false;
        egui::ScrollArea::vertical().id_salt("raster_import").show(ui, |ui| {
            ui.heading("Import image");
            ui.label(draft.path.file_name().unwrap_or_default().to_string_lossy());
            let before = draft.options.clone();
            let before_fill = draft.fill.clone();
            processing_controls(ui, &mut draft.options);
            if before.processing.pen_width_mm != draft.options.processing.pen_width_mm { draft.fill.pen_width_mm = draft.options.processing.pen_width_mm; }
            if before.processing.mode != draft.options.processing.mode && draft.options.processing.mode == RasterMode::Color {
                draft.fill.fill = Fill::Lines;
                draft.fill.outline = false;
            }
            ui.separator();
            if regions(&draft.options) {
                raster_fill_controls(ui, &mut draft.fill, "raster_import_fill");
            } else {
                draft.fill.fill = Fill::None;
                ui.small("Single centerline strokes; no outline or hatch. Tiny isolated dots may be omitted.");
            }
            if draft.options != before || draft.fill != before_fill { draft.invalidate(); }
            ui.separator();
            draft.poll(&ctx);
            ui.strong("Preview paths");
            if !draft.is_current() {
                ui.label("Updating… showing last valid paths when available.");
            } else if let Some(error) = &draft.error {
                ui.colored_label(WARN, error);
                if draft.preview.is_some() {
                    ui.small("Showing previous paths; the current settings cannot be imported.");
                }
            } else if let Some(preview) = &draft.preview {
                ui.small(format!(
                    "{} paths.{}",
                    preview.artwork.strokes.len(),
                    if regions(&draft.options) { " Hatch shown at import size; spacing stays in paper mm after placement." } else { "" }
                ));
            }
            if draft.options.processing.mode == RasterMode::Color {
                let colors: std::collections::BTreeSet<_> = draft.preview_routes.iter().filter_map(|s| penplot::ink_rgb(&s.layer)).collect();
                ui.horizontal_wrapped(|ui| { for [r,g,b] in colors {
                    ui.colored_label(Color32::from_rgb(r,g,b), "■");
                    ui.monospace(format!("#{r:02X}{g:02X}{b:02X}"));
                }});
                ui.small("One editable layer per color. Export colors writes separate pen passes with shared paper coordinates.");
            }
            preview_outlines(ui, &draft.preview_routes);
            ui.horizontal(|ui| {
                cancel = ui.button("Cancel").clicked();
                add = ui
                    .add_enabled(draft.can_add(), egui::Button::new("Add image"))
                    .clicked();
            });
        });
        if add {
            let mut draft = self.draft.take().expect("add requires image draft");
            ctx.request_repaint();
            if !regions(&draft.options) {
                draft.fill.fill = Fill::None;
            }
            return draft.preview.take().map(|item| (item, draft.fill));
        }
        if cancel {
            self.cancel_current();
            ctx.request_repaint();
        }
        None
    }
}

fn regions(options: &ImportOptions) -> bool {
    options.processing.mode == RasterMode::Color
        || (options.processing.mode == RasterMode::Threshold && options.trace.is_outline())
}

pub(super) fn processing_controls(ui: &mut egui::Ui, options: &mut ImportOptions) {
    let label = match options.processing.mode {
        RasterMode::Threshold => "Threshold (classic)",
        RasterMode::CleanLine => "Clean line art",
        RasterMode::Color => "Color hatch",
    };
    egui::ComboBox::from_id_salt("image_processing")
        .selected_text(label)
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut options.processing.mode,
                RasterMode::Threshold,
                "Threshold (classic)",
            );
            ui.selectable_value(
                &mut options.processing.mode,
                RasterMode::CleanLine,
                "Clean line art",
            );
            ui.selectable_value(
                &mut options.processing.mode,
                RasterMode::Color,
                "Color hatch",
            );
        });
    ui.horizontal(|ui| {
        ui.label("Image DPI");
        ui.add(
            egui::DragValue::new(&mut options.dpi)
                .range(1.0..=9600.0)
                .speed(1.0),
        );
    });
    pen_width_controls(
        ui,
        &mut options.processing.pen_width_mm,
        "Ignore image details thinner than pen",
    );
    if options.processing.pen_width_mm > 0. {
        ui.small("Measured at the image DPI before placement. Reimport if the final drawing scale changes.");
    }
    match options.processing.mode {
        RasterMode::Threshold => {
            ui.add(egui::Slider::new(&mut options.threshold, 0..=255).text("Threshold"));
            ui.checkbox(&mut options.invert, "Trace light instead of dark");
            trace_mode_controls(ui, &mut options.trace);
        }
        RasterMode::CleanLine => {
            ui.small("Local background correction + connected speckle removal + centerline trace.");
            ui.add(
                egui::Slider::new(&mut options.processing.radius, 2..=128)
                    .text("Background radius (px)"),
            );
            ui.add(
                egui::Slider::new(&mut options.processing.contrast, 1..=80).text("Ink contrast"),
            );
            ui.add(
                egui::Slider::new(&mut options.processing.speckle, 0..=100)
                    .text("Remove dots ≤ px"),
            );
            ui.checkbox(&mut options.invert, "Trace light instead of dark");
            ui.small("Lower contrast keeps faint lines. Increase radius for broader strokes; speckle 0 keeps small marks.");
        }
        RasterMode::Color => {
            ui.add(
                egui::Slider::new(&mut options.processing.palette_size, 2..=8).text("Maximum pens"),
            );
            ui.checkbox(
                &mut options.processing.exclude_background,
                "Exclude white background",
            );
            if options.processing.exclude_background {
                ui.add(
                    egui::Slider::new(&mut options.processing.background_cutoff, 128..=255)
                        .text("White cutoff"),
                );
            }
            ui.small("Transparent pixels stay empty. Colors are estimated from the image; choose matching pens.");
        }
    }
}

pub(super) fn pen_width_controls(ui: &mut egui::Ui, width: &mut f64, label: &str) {
    let mut on = *width > 0.;
    if ui.checkbox(&mut on, label).changed() {
        *width = if on { 0.3 } else { 0. };
    }
    if on {
        ui.horizontal(|ui| {
            ui.label("Pen width");
            ui.add(
                egui::DragValue::new(width)
                    .range(0.05..=3.)
                    .speed(0.01)
                    .suffix(" mm"),
            );
        });
    }
}

pub(super) fn trace_mode_controls(ui: &mut egui::Ui, trace: &mut TraceMode) -> bool {
    let before = *trace;
    ui.horizontal(|ui| {
        ui.label("Trace");
        ui.selectable_value(trace, TraceMode::Outline, "Outline");
        ui.selectable_value(trace, TraceMode::Centerline, "Centerline");
    });
    *trace != before
}

pub(super) fn raster_fill_controls(
    ui: &mut egui::Ui,
    settings: &mut penplot::RasterSettings,
    id: &str,
) {
    let label = match settings.fill {
        Fill::None => "Outline only",
        Fill::Lines => "Hatch fill",
        Fill::Cross => "Cross-hatch",
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(label)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut settings.fill, Fill::None, "Outline only");
            ui.selectable_value(&mut settings.fill, Fill::Lines, "Hatch fill");
            ui.selectable_value(&mut settings.fill, Fill::Cross, "Cross-hatch");
        });
    if settings.fill != Fill::None {
        pen_width_controls(
            ui,
            &mut settings.pen_width_mm,
            "Keep fill inside pen-width clearance",
        );
        ui.checkbox(&mut settings.outline, "Draw outlines");
        ui.horizontal(|ui| {
            ui.label("Spacing (mm)");
            ui.add(
                egui::DragValue::new(&mut settings.hatch_spacing)
                    .range(0.02..=1000.0)
                    .speed(0.05),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Angle (deg)");
            ui.add(
                egui::DragValue::new(&mut settings.hatch_angle)
                    .range(-360.0..=360.0)
                    .speed(1.0),
            );
        });
    }
}

fn preview_outlines(ui: &mut egui::Ui, strokes: &[Stroke]) {
    let (rect, painter) = ui.allocate_painter(
        Vec2::new(ui.available_width().clamp(100.0, 680.0), 210.0),
        Sense::hover(),
    );
    let rect = rect.rect;
    painter.rect_filled(rect, 4.0, PAPER);
    if strokes.is_empty() {
        return;
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for p in strokes.iter().flat_map(|s| &s.points) {
        minx = minx.min(p.x);
        miny = miny.min(p.y);
        maxx = maxx.max(p.x);
        maxy = maxy.max(p.y);
    }
    let midx = (minx + maxx) * 0.5;
    let midy = (miny + maxy) * 0.5;
    let scale = (f64::from(rect.width() - 16.0) / (maxx - minx).max(1e-9))
        .min(f64::from(rect.height() - 16.0) / (maxy - miny).max(1e-9));
    let center = rect.center();
    for stroke in strokes {
        let ink = Ink::new(1.0, route_color(&stroke.layer));
        let mut coords: Vec<_> = stroke
            .points
            .iter()
            .map(|p| {
                center + Vec2::new(((p.x - midx) * scale) as f32, ((midy - p.y) * scale) as f32)
            })
            .collect();
        if stroke.closed && coords.first() != coords.last() {
            coords.push(coords[0]);
        }
        painter.add(egui::Shape::line(coords, ink));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo(name: &str) -> Imported {
        Imported {
            name: name.into(),
            artwork: Artwork::default(),
        }
    }

    #[test]
    fn color_import_worker_adds_editable_layers_and_scaling_keeps_other_pen_fixed() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/color-card.png");
        let mut options = ImportOptions::default();
        options.processing.mode = RasterMode::Color;
        let mut draft = Draft::new(path, options);
        assert_eq!(draft.fill.fill, Fill::Lines);
        assert!(!draft.fill.outline);
        let ctx = egui::Context::default();
        for _ in 0..200 {
            draft.poll(&ctx);
            if draft.is_current() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(draft.can_add(), "{:?}", draft.error);
        assert!(draft.preview_routes.iter().all(|s| !s.closed));
        let mut app = Studio::default();
        app.add_raster_layer(draft.preview.take().unwrap(), draft.fill);
        assert_eq!(app.layers.len(), 3);
        app.rebuild();
        let before = penplot::color_jobs(&app.planned, &app.profile, false, false).unwrap();
        app.transform_layer(2, PlacementMotion::Scale(Point::new(0., 0.), 1.1))
            .unwrap();
        app.rebuild();
        let after = penplot::color_jobs(&app.planned, &app.profile, false, false).unwrap();
        assert_eq!(before[1].color, after[1].color);
        assert_eq!(before[1].output.gcode, after[1].output.gcode);
        let project = app.as_project();
        assert!(
            std::str::from_utf8(&project.to_json().unwrap())
                .unwrap()
                .contains("\"version\":4")
        );
        assert_eq!(
            Project::from_json(&project.to_json().unwrap()).unwrap(),
            project
        );
    }

    #[test]
    fn mixed_import_waits_for_vectors_before_starting_the_image_draft() {
        let mut app = Studio::default();
        let (_sender, receiver) = mpsc::channel::<ImportReport>();
        app.import_task = Some(receiver);
        app.raster_import.extend([PathBuf::from("queued.png")]);
        app.switch_workspace(Workspace::Import);
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.update_ui(ctx));
        assert!(app.raster_import.draft.is_none());
        assert_eq!(app.raster_import.queue.len(), 1);
        assert!(app.layers.is_empty());
    }

    #[test]
    fn import_workspace_round_trip_retains_draft_and_cancel_allows_reimport() {
        let mut app = Studio::default();
        let mut draft = Draft::new("demo.png".into(), ImportOptions::default());
        draft.options.threshold = 64;
        draft.options.trace = TraceMode::Centerline;
        draft.fill.hatch_spacing = 2.5;
        draft.accept(0, Ok(demo("retained")));
        app.raster_import.draft = Some(draft);
        for workspace in [
            Workspace::Import,
            Workspace::Edit,
            Workspace::Preview,
            Workspace::Import,
        ] {
            app.switch_workspace(workspace);
            let draft = app.raster_import.draft.as_ref().unwrap();
            assert_eq!(draft.options.threshold, 64);
            assert_eq!(draft.options.trace, TraceMode::Centerline);
            assert_eq!(draft.fill.hatch_spacing, 2.5);
            assert_eq!(draft.preview.as_ref().unwrap().name, "retained");
            assert!(draft.can_add());
            assert!(!app.can_generate());
        }
        app.raster_import.cancel_current();
        assert!(!app.raster_import.is_busy());
        app.raster_import.extend([PathBuf::from("second.png")]);
        assert!(app.raster_import.is_busy());
        assert_eq!(
            app.raster_import.queue.front().unwrap(),
            &PathBuf::from("second.png")
        );
        assert!(app.layers.is_empty());
    }

    #[test]
    fn threshold_change_keeps_previous_preview_and_ignores_stale_worker() {
        let mut draft = Draft::new("a.png".into(), ImportOptions::default());
        draft.accept(0, Ok(demo("first")));
        assert!(draft.can_add());
        draft.options.trace = TraceMode::Centerline;
        draft.invalidate();
        assert_eq!(draft.preview.as_ref().unwrap().name, "first");
        assert!(!draft.can_add());
        draft.accept(0, Ok(demo("stale")));
        assert_eq!(draft.preview.as_ref().unwrap().name, "first");
        draft.accept(1, Err("no pixels".into()));
        assert!(draft.is_current());
        assert!(!draft.can_add());
        assert_eq!(draft.preview.as_ref().unwrap().name, "first");
        draft.invalidate();
        draft.accept(2, Ok(demo("latest")));
        assert!(draft.can_add());
        assert_eq!(draft.preview.unwrap().name, "latest");
    }
}
