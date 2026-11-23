//! Image import keeps the prior successful outline while a newer threshold is traced.
use super::*;
use penplot::TraceMode;

pub(super) const DEBOUNCE: Duration = Duration::from_millis(140);

#[derive(Default)]
pub(super) struct RasterImport {
    queue: std::collections::VecDeque<PathBuf>,
    draft: Option<Draft>,
}

type TraceResult = Result<Imported, String>;

struct Draft {
    path: PathBuf,
    options: ImportOptions,
    fill: penplot::RasterSettings,
    last_edit: std::time::Instant,
    revision: u64,
    completed: Option<u64>,
    preview: Option<Imported>,
    error: Option<String>,
    worker: Option<(u64, mpsc::Receiver<TraceResult>)>,
}

impl Draft {
    fn new(path: PathBuf, options: ImportOptions) -> Self {
        let now = std::time::Instant::now();
        Self {
            path,
            options,
            fill: Default::default(),
            last_edit: now.checked_sub(DEBOUNCE).unwrap_or(now),
            revision: 0,
            completed: None,
            preview: None,
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
                    self.accept(revision, result);
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
        let revision = self.revision;
        let path = self.path.clone();
        let ctx = ctx.clone();
        let (sender, worker) = mpsc::channel();
        self.worker = Some((revision, worker));
        std::thread::spawn(move || {
            let result = import_one(&path, &options);
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
            ui.horizontal(|ui| {
                ui.label("Threshold");
                ui.add(egui::Slider::new(&mut draft.options.threshold, 0..=255));
            });
            ui.horizontal(|ui| {
                ui.label("Image DPI");
                ui.add(
                    egui::DragValue::new(&mut draft.options.dpi)
                        .range(1.0..=9600.0)
                        .speed(1.0),
                );
            });
            ui.checkbox(&mut draft.options.invert, "Trace light instead of dark");
            trace_mode_controls(ui, &mut draft.options.trace);
            if draft.options != before {
                draft.invalidate();
            }
            ui.separator();
            if draft.options.trace.is_outline() {
                raster_fill_controls(ui, &mut draft.fill, "raster_import_fill");
            } else {
                ui.small("Centerline paths are single strokes and are not hatched. Isolated dots may be omitted.");
            }
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
                    if draft.options.trace.is_outline() { " Fill is calculated after placement." } else { "" }
                ));
            }
            preview_outlines(ui, draft.preview.as_ref());
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
            if !draft.options.trace.is_outline() {
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

fn preview_outlines(ui: &mut egui::Ui, preview: Option<&Imported>) {
    let (rect, painter) = ui.allocate_painter(
        Vec2::new(ui.available_width().clamp(100.0, 680.0), 210.0),
        Sense::hover(),
    );
    let rect = rect.rect;
    painter.rect_filled(rect, 4.0, PAPER);
    let Some(preview) = preview else {
        return;
    };
    let strokes = &preview.artwork.strokes;
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
    let ink = Ink::new(1.0, Color32::from_rgb(32, 40, 48));
    for stroke in strokes {
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
            assert!(!app.can_export());
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
