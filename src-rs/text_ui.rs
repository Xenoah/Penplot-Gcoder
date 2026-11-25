//! Text editing is transactional: original outlines survive cancel, font loss and errors.
use super::*;
use penplot::text_outline::{self, FontFace, FontIdentity};
use penplot::{TextLayer, TraceMode};

type OutlineResult = Result<(Artwork, FontIdentity), String>;
struct Catalogue {
    faces: Vec<FontFace>,
    exact: Option<usize>,
}

pub(super) struct TextDialog {
    target: Option<usize>,
    document_revision: u64,
    original_font: Option<FontIdentity>,
    content: String,
    em_mm: f64,
    tolerance_mm: f64,
    trace: TraceMode,
    x: f64,
    y: f64,
    rotation_deg: f64,
    faces: Vec<FontFace>,
    chosen: Option<usize>,
    catalogue: Option<mpsc::Receiver<Catalogue>>,
    file_task: Option<mpsc::Receiver<Result<Vec<FontFace>, String>>>,
    worker: Option<(u64, mpsc::Receiver<OutlineResult>)>,
    revision: u64,
    preview: Option<(u64, Artwork, FontIdentity)>,
    replacement_confirmed: bool,
    error: Option<String>,
}

impl TextDialog {
    fn from_metadata(
        target: Option<usize>,
        document_revision: u64,
        metadata: Option<&TextLayer>,
        world: [f64; 6],
    ) -> Self {
        let original_font = metadata.map(|text| text.font.clone());
        let mut draft = Self {
            target,
            document_revision,
            original_font: original_font.clone(),
            content: metadata.map_or_else(String::new, |t| t.content.clone()),
            em_mm: metadata.map_or(10., |t| t.em_mm),
            tolerance_mm: metadata.map_or(0.025, |t| t.tolerance_mm),
            trace: metadata.map_or(TraceMode::Outline, |t| t.trace),
            x: 10.,
            y: 10.,
            rotation_deg: 0.,
            faces: Vec::new(),
            chosen: None,
            catalogue: None,
            file_task: None,
            worker: None,
            revision: 0,
            preview: None,
            replacement_confirmed: false,
            error: None,
        };
        if let Some(metadata) = metadata {
            let physical = compose(world, metadata.transform);
            let gain = physical[0].hypot(physical[1]);
            draft.em_mm *= gain;
            draft.tolerance_mm = (draft.tolerance_mm * gain).clamp(0.001, 1.);
            draft.x = physical[4];
            draft.y = physical[5];
            draft.rotation_deg = physical[1].atan2(physical[0]).to_degrees();
        }
        draft
    }

    fn new(
        target: Option<usize>,
        document_revision: u64,
        metadata: Option<&TextLayer>,
        world: [f64; 6],
    ) -> Self {
        let mut draft = Self::from_metadata(target, document_revision, metadata, world);
        let original_font = draft.original_font.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let faces = text_outline::list_installed_fonts();
            let exact = original_font.as_ref().and_then(|identity| {
                faces.iter().position(|face| {
                    face.family == identity.family
                        && face.style == identity.style
                        && face.face_index == identity.face_index
                        && text_outline::font_identity(face).as_ref() == Ok(identity)
                })
            });
            let _ = sender.send(Catalogue { faces, exact });
        });
        draft.catalogue = Some(receiver);
        draft
    }

    fn invalidate(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.preview = None;
        self.error = None;
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(task) = &self.catalogue {
            match task.try_recv() {
                Ok(catalogue) => {
                    self.catalogue = None;
                    self.chosen = catalogue.exact.or_else(|| {
                        self.original_font
                            .is_none()
                            .then_some(0)
                            .filter(|_| !catalogue.faces.is_empty())
                    });
                    self.faces = catalogue.faces;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.catalogue = None;
                    self.error = Some("Font discovery stopped. Choose a local font file.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(task) = &self.file_task {
            match task.try_recv() {
                Ok(result) => {
                    self.file_task = None;
                    match result {
                        Ok(faces) if !faces.is_empty() => {
                            let next = self.faces.len();
                            self.faces.extend(faces);
                            self.chosen = Some(next);
                            self.replacement_confirmed = false;
                            self.invalidate();
                        }
                        Ok(_) => {
                            self.error = Some("This file contains no supported font faces.".into())
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.file_task = None;
                    self.error =
                        Some("Font file inspection stopped. Choose the file again.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some((revision, worker)) = &self.worker {
            let revision = *revision;
            match worker.try_recv() {
                Ok(result) => {
                    self.worker = None;
                    if revision == self.revision {
                        match result {
                            Ok((artwork, identity)) => {
                                self.preview = Some((revision, artwork, identity))
                            }
                            Err(error) => self.error = Some(error),
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.worker = None;
                    self.error = Some("Text outline worker stopped. Preview again.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.catalogue.is_some() || self.file_task.is_some() || self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }

    fn generate(&mut self) {
        let Some(font) = self.chosen.and_then(|index| self.faces.get(index)).cloned() else {
            self.error = Some("Choose a font family and face, or a local font file.".into());
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let (text, em_mm, tolerance) = (self.content.clone(), self.em_mm, self.tolerance_mm);
        let trace = self.trace;
        std::thread::spawn(move || {
            let _ = sender.send(text_outline::trace_text_with_identity(
                &font, &text, em_mm, tolerance, trace,
            ));
        });
        self.error = None;
        self.preview = None;
        self.worker = Some((self.revision, receiver));
    }

    fn replacement_needed(&self) -> bool {
        self.original_font.as_ref().is_some_and(|original| {
            self.preview
                .as_ref()
                .is_some_and(|(_, _, current)| original != current)
        })
    }

    fn can_apply(&self) -> bool {
        self.worker.is_none()
            && self.catalogue.is_none()
            && self.file_task.is_none()
            && self
                .preview
                .as_ref()
                .is_some_and(|(rev, art, _)| *rev == self.revision && !art.strokes.is_empty())
            && (!self.replacement_needed() || self.replacement_confirmed)
    }

    fn show(&mut self, ctx: &egui::Context) -> (bool, bool) {
        self.poll(ctx);
        let (mut apply, mut cancel) = (false, false);
        let response = egui::Modal::new(egui::Id::new("text_editor")).show(ctx, |ui| {
            ui.set_width(570.0_f32.min((ctx.screen_rect().width()-40.).max(240.)));
            egui::ScrollArea::vertical().max_height((ctx.screen_rect().height()-70.).max(220.)).show(ui, |ui| {
                ui.heading(if self.target.is_some() { "Edit text" } else { "Add text" });
                ui.label("Horizontal text · 2,000 Unicode characters / 100 lines");
                let mut changed = ui.add(egui::TextEdit::multiline(&mut self.content)
                    .id(egui::Id::new("text_body")).desired_rows(4).desired_width(f32::INFINITY)
                    .char_limit(text_outline::MAX_TEXT_CHARS).hint_text("日本語 / Text")).changed();
                if self.catalogue.is_some() { ui.horizontal(|ui| { ui.spinner(); ui.label("Finding installed fonts…"); }); }
                let old_chosen = self.chosen;
                let family = self.chosen.and_then(|i| self.faces.get(i)).map(|f| f.family.clone());
                ui.horizontal(|ui| {
                    ui.label("Family");
                    egui::ComboBox::from_id_salt("text_family").width(280.)
                        .selected_text(family.as_deref().unwrap_or("Choose a family"))
                        .show_ui(ui, |ui| {
                            let mut families = std::collections::BTreeSet::new();
                            for (i, font) in self.faces.iter().enumerate() {
                                if families.insert(&font.family) && ui.selectable_label(family.as_ref() == Some(&font.family), &font.family).clicked() {
                                    self.chosen = Some(i);
                                }
                            }
                        });
                });
                let family = self.chosen.and_then(|i| self.faces.get(i)).map(|f| f.family.clone());
                ui.horizontal(|ui| {
                    ui.label("Face");
                    let face_label = self.chosen.and_then(|i| self.faces.get(i)).map(|f| format!("{} · face {}", f.style, f.face_index));
                    egui::ComboBox::from_id_salt("text_face").width(280.)
                        .selected_text(face_label.as_deref().unwrap_or("Choose a face"))
                        .show_ui(ui, |ui| {
                            for (i, font) in self.faces.iter().enumerate().filter(|(_,font)| Some(&font.family)==family.as_ref()) {
                                ui.selectable_value(&mut self.chosen, Some(i), format!("{} · face {}", font.style, font.face_index)).on_hover_text(font.path.display().to_string());
                            }
                        });
                    if ui.add_enabled(self.catalogue.is_none() && self.file_task.is_none(), egui::Button::new("Font file…")).clicked()
                        && let Some(path) = rfd::FileDialog::new().add_filter("OpenType / TrueType", &["ttf", "otf", "ttc", "otc"]).pick_file() {
                        let (sender, receiver) = mpsc::channel();
                        std::thread::spawn(move || { let _ = sender.send(text_outline::list_font_faces(&path)); });
                        self.file_task = Some(receiver);
                    }
                });
                if old_chosen != self.chosen { self.replacement_confirmed = false; changed = true; }
                if self.original_font.is_some() && self.chosen.is_none() && self.catalogue.is_none() {
                    ui.colored_label(WARN, "The exact saved font is unavailable. Saved paths are unchanged. Choose a replacement font and preview it before applying.");
                }
                ui.horizontal(|ui| {
                    ui.label("Em size (mm)");
                    changed |= ui.add(egui::DragValue::new(&mut self.em_mm).range(0.1..=500.).speed(0.1)).changed();
                    ui.label("Curve tolerance (mm)");
                    changed |= ui.add(egui::DragValue::new(&mut self.tolerance_mm).range(0.001..=1.).speed(0.005)).changed();
                });
                ui.horizontal(|ui| {
                    ui.label("Anchor X / Y (mm)");
                    changed |= ui.add(egui::DragValue::new(&mut self.x).range(-1e8..=1e8).speed(0.5)).changed();
                    changed |= ui.add(egui::DragValue::new(&mut self.y).range(-1e8..=1e8).speed(0.5)).changed();
                });
                ui.small("Size and position are in the current bed view. Em size is not the visible letter height. Existing rotation is preserved; later job scaling also scales text.");
                changed |= raster_ui::trace_mode_controls(ui, &mut self.trace);
                if self.trace == TraceMode::Centerline {
                    ui.small("Centerlines are inferred from the font shape, not handwritten stroke order.");
                }
                if changed { self.invalidate(); }
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.chosen.is_some() && self.worker.is_none() && self.file_task.is_none() && self.catalogue.is_none(), egui::Button::new("Preview paths")).clicked() { self.generate(); }
                    if self.worker.is_some() || self.file_task.is_some() { ui.spinner(); }
                });
                if let Some(error) = &self.error { ui.colored_label(WARN, error); }
                if let Some((_,artwork,identity)) = &self.preview {
                    ui.label(format!("{} · {} · {} paths",identity.family, identity.style, artwork.strokes.len()));
                    outline_preview(ui, &artwork.strokes);
                    for warning in &artwork.warnings { ui.colored_label(WARN, warning); }
                }
                if self.replacement_needed() {
                    ui.colored_label(WARN, "This font differs from the saved font. Review the new paths.");
                    ui.checkbox(&mut self.replacement_confirmed, "Use this replacement font for this text");
                }
                ui.small("TTF/OTF/TTC/OTC outlines; variable fonts use default axes. Missing or bitmap/color-only glyphs report an error. Font files stay on this PC and are never embedded.");
                ui.horizontal(|ui| {
                    apply = ui.add_enabled(self.can_apply(), egui::Button::new(if self.target.is_some() { "Apply text" } else { "Add text" })).clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
        });
        (apply, cancel || response.should_close())
    }
}

fn outline_preview(ui: &mut egui::Ui, strokes: &[Stroke]) {
    let (response, painter) =
        ui.allocate_painter(egui::vec2(ui.available_width(), 145.), Sense::hover());
    painter.rect_filled(response.rect, 4., PAPER);
    let points = strokes.iter().flat_map(|s| &s.points);
    let mut bounds = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for p in points {
        bounds.0 = bounds.0.min(p.x);
        bounds.1 = bounds.1.min(p.y);
        bounds.2 = bounds.2.max(p.x);
        bounds.3 = bounds.3.max(p.y);
    }
    let scale = ((response.rect.width() - 16.) as f64 / (bounds.2 - bounds.0).max(0.01))
        .min((response.rect.height() - 16.) as f64 / (bounds.3 - bounds.1).max(0.01));
    let at = |p: Point| {
        Pos2::new(
            response.rect.left() + 8. + ((p.x - bounds.0) * scale) as f32,
            response.rect.bottom() - 8. - ((p.y - bounds.1) * scale) as f32,
        )
    };
    // Render the whole result without allocating a second flattened geometry buffer.
    for stroke in strokes {
        for pair in stroke.points.windows(2) {
            painter.line_segment(
                [at(pair[0]), at(pair[1])],
                Ink::new(1.2, Color32::from_rgb(29, 60, 99)),
            );
        }
        if stroke.closed
            && let (Some(a), Some(b)) = (stroke.points.first(), stroke.points.last())
        {
            painter.line_segment(
                [at(*a), at(*b)],
                Ink::new(1.2, Color32::from_rgb(29, 60, 99)),
            );
        }
    }
}

fn compose(l: [f64; 6], r: [f64; 6]) -> [f64; 6] {
    [
        l[0] * r[0] + l[2] * r[1],
        l[1] * r[0] + l[3] * r[1],
        l[0] * r[2] + l[2] * r[3],
        l[1] * r[2] + l[3] * r[3],
        l[0] * r[4] + l[2] * r[5] + l[4],
        l[1] * r[4] + l[3] * r[5] + l[5],
    ]
}
fn inverse(m: [f64; 6]) -> Result<[f64; 6], String> {
    let det = m[0] * m[3] - m[1] * m[2];
    if !m.iter().all(|x| x.is_finite()) || !det.is_finite() || det.abs() < 1e-16 {
        return Err("Correct the artwork placement before editing text.".into());
    }
    Ok([
        m[3] / det,
        -m[1] / det,
        -m[2] / det,
        m[0] / det,
        (m[2] * m[5] - m[3] * m[4]) / det,
        (m[1] * m[4] - m[0] * m[5]) / det,
    ])
}

impl Studio {
    fn text_world_transform(&self) -> [f64; 6] {
        let (sin, cos) = self
            .options
            .rotation_deg
            .rem_euclid(360.)
            .to_radians()
            .sin_cos();
        let scale = self.options.scale;
        let shift = projected_min(&self.layers, scale, sin, cos)
            .map_or(Point::new(0., 0.), |min| {
                Point::new(self.options.x - min.x, self.options.y - min.y)
            });
        [
            scale * cos,
            scale * sin,
            -scale * sin,
            scale * cos,
            shift.x,
            shift.y,
        ]
    }

    pub(super) fn open_text_dialog(&mut self, target: Option<usize>) {
        if self.text_dialog.is_some()
            || self.raster_import.is_busy()
            || self.import_task.is_some()
            || self.import_dialog.is_some()
            || self.placement_drag.is_some()
            || !self.draft.is_empty()
        {
            return;
        }
        let metadata = target
            .and_then(|index| self.layers.get(index))
            .and_then(|layer| layer.text.as_ref());
        if target.is_some() && metadata.is_none() {
            return;
        }
        self.text_dialog = Some(TextDialog::new(
            target,
            self.revision,
            metadata,
            self.text_world_transform(),
        ));
    }

    pub(super) fn show_text_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.text_dialog.take() else {
            return;
        };
        let (apply, cancel) = dialog.show(ctx);
        if apply {
            match self.apply_text(&dialog) {
                Ok(()) => return,
                Err(error) => dialog.error = Some(error),
            }
        }
        if !cancel {
            self.text_dialog = Some(dialog);
        }
    }

    fn apply_text(&mut self, dialog: &TextDialog) -> Result<(), String> {
        if dialog.document_revision != self.revision {
            return Err("The document changed. Cancel and reopen text editing.".into());
        }
        if !dialog.can_apply() {
            return Err("Preview the current text and confirm any replacement font first.".into());
        }
        if dialog.target.is_none() && self.layers.len() >= MAX_LAYER_COUNT {
            return Err("Layer limit reached. Delete a layer first.".into());
        }
        let (_, artwork, identity) = dialog.preview.as_ref().ok_or("Preview text first.")?;
        let (sin, cos) = dialog.rotation_deg.to_radians().sin_cos();
        let physical = [cos, sin, -sin, cos, dialog.x, dialog.y];
        let metadata = TextLayer {
            content: dialog.content.clone(),
            font: identity.clone(),
            em_mm: dialog.em_mm,
            tolerance_mm: dialog.tolerance_mm,
            trace: dialog.trace,
            transform: compose(inverse(self.text_world_transform())?, physical),
        };
        metadata.validate()?;
        let mut strokes = artwork.strokes.clone();
        for stroke in &mut strokes {
            for point in &mut stroke.points {
                *point = metadata.map(*point);
                if !(-1e8..=1e8).contains(&point.x) || !(-1e8..=1e8).contains(&point.y) {
                    return Err("Text exceeds document coordinate limits.".into());
                }
            }
        }
        let (paths, vertices) = self.geometry_size();
        let old = dialog.target.and_then(|index| self.layers.get(index));
        if dialog.target.is_some() && old.is_none() {
            return Err("The text object no longer exists.".into());
        }
        let old_paths = old.map_or(0, |layer| layer.strokes.len());
        let old_points = old.map_or(0, |layer| {
            layer.strokes.iter().map(|s| s.points.len()).sum::<usize>()
        });
        if paths - old_paths + strokes.len() > penplot::MAX_STROKES
            || vertices - old_points + strokes.iter().map(|s| s.points.len()).sum::<usize>()
                > penplot::MAX_POINTS
        {
            return Err("Text exceeds workspace geometry limits.".into());
        }
        let name = format!(
            "Text: {}",
            dialog
                .content
                .lines()
                .next()
                .unwrap_or("Text")
                .chars()
                .filter(|c| !c.is_control())
                .take(40)
                .collect::<String>()
        );
        let layer = Layer {
            name,
            strokes: strokes.into(),
            visible: old.is_none_or(|layer| layer.visible),
            raster: None,
            text: Some(metadata),
        };
        let (sin, cos) = self
            .options
            .rotation_deg
            .rem_euclid(360.)
            .to_radians()
            .sin_cos();
        let before = projected_min(&self.layers, self.options.scale, sin, cos);
        self.record_layers();
        let index = if let Some(index) = dialog.target {
            self.layers[index] = layer;
            index
        } else {
            self.layers.push(layer);
            self.layers.len() - 1
        };
        if let Some(after) = projected_min(&self.layers, self.options.scale, sin, cos) {
            if let Some(before) = before {
                self.options.x += after.x - before.x;
                self.options.y += after.y - before.y;
            } else {
                self.options.x = after.x;
                self.options.y = after.y;
            }
        }
        self.selected = Some(Selection::Layer(index));
        self.switch_workspace(Workspace::Edit);
        self.invalidate();
        self.status = "Text outlines saved. Select the layer and Edit text… to change it.".into();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> FontFace {
        text_outline::list_font_faces(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/font/synthetic.ttf"),
        )
        .unwrap()
        .remove(0)
    }
    fn prepare(app: &Studio, target: Option<usize>, content: &str) -> TextDialog {
        let metadata = target.and_then(|i| app.layers[i].text.as_ref());
        let mut draft =
            TextDialog::from_metadata(target, app.revision, metadata, app.text_world_transform());
        draft.content = content.into();
        let font = font();
        let (art, id) = text_outline::outline_text_with_identity(
            &font,
            content,
            draft.em_mm,
            draft.tolerance_mm,
        )
        .unwrap();
        draft.faces = vec![font];
        draft.chosen = Some(0);
        draft.preview = Some((draft.revision, art, id));
        draft
    }
    fn near(a: Point, b: Point) {
        assert!(a.distance(b) < 1e-8, "{a:?} != {b:?}");
    }

    #[test]
    fn resized_text_keeps_its_paper_size_and_position_when_reedited() {
        let mut app = Studio::default();
        let draft = prepare(&app, None, "O");
        app.apply_text(&draft).unwrap();
        app.transform_layer(0, PlacementMotion::Scale(Point::new(5., 5.), 1.5))
            .unwrap();
        let resized = app.layers[0].text.as_ref().unwrap();
        let physical = compose(app.text_world_transform(), resized.transform);
        let edit = prepare(&app, Some(0), "OO");
        assert!((edit.em_mm - 15.).abs() < 1e-8);
        assert!((edit.x - physical[4]).abs() < 1e-8);
        assert!((edit.y - physical[5]).abs() < 1e-8);
        app.apply_text(&edit).unwrap();
        let updated = app.layers[0].text.as_ref().unwrap();
        let after = compose(app.text_world_transform(), updated.transform);
        assert!((updated.em_mm * after[0].hypot(after[1]) - 15.).abs() < 1e-8);
        near(
            Point::new(physical[4], physical[5]),
            Point::new(after[4], after[5]),
        );
    }

    #[test]
    fn text_is_a_separate_object_with_saved_holes_and_delete_undo_redo() {
        let mut app = Studio::default();
        let draft = prepare(&app, None, "fi 日本語\nO");
        app.apply_text(&draft).unwrap();
        assert_eq!(app.layers.len(), 1);
        assert_eq!(app.layers[0].strokes.len(), 6);
        assert_eq!(
            app.layers[0].strokes[4].group,
            app.layers[0].strokes[5].group
        );
        assert!(app.preview_dirty && app.output.is_none());
        app.options.min_image_stroke_mm = 10_000.;
        app.rebuild();
        assert!(app.can_export());
        assert_eq!(app.planned.len(), 6);
        assert!(
            app.route_targets
                .iter()
                .all(|t| *t == Some(Selection::Layer(0)))
        );
        let first = app.as_project().to_json().unwrap();
        assert!(app.delete_object(Selection::Layer(0)));
        assert!(app.layers.is_empty() && !app.can_export());
        app.undo();
        assert_eq!(first, app.as_project().to_json().unwrap());
        app.redo();
        assert!(app.layers.is_empty());
        app.undo();
        let draft = prepare(&app, Some(0), "O日本語");
        app.apply_text(&draft).unwrap();
        assert_eq!(app.layers.len(), 1);
        assert_eq!(app.layers[0].text.as_ref().unwrap().content, "O日本語");
        app.undo();
        assert_eq!(first, app.as_project().to_json().unwrap());
    }

    #[test]
    fn centerline_text_reopens_in_the_same_trace_mode_and_plots_one_ring() {
        let mut app = Studio::default();
        let mut draft = prepare(&app, None, "O");
        draft.trace = TraceMode::Centerline;
        let (art, id) = text_outline::trace_text_with_identity(
            &draft.faces[0],
            "O",
            draft.em_mm,
            draft.tolerance_mm,
            draft.trace,
        )
        .unwrap();
        draft.preview = Some((draft.revision, art, id));
        app.apply_text(&draft).unwrap();
        app.rebuild();
        assert_eq!(app.planned.len(), 1);
        assert!(app.planned[0].closed && !app.planned[0].filled);
        assert!(app.can_export());
        let json = app.as_project().to_json().unwrap();
        let project = Project::from_json(&json).unwrap();
        let metadata = project.layers[0].text.as_ref().unwrap();
        assert_eq!(metadata.trace, TraceMode::Centerline);
        let reopened = TextDialog::from_metadata(
            Some(0),
            app.revision,
            Some(metadata),
            app.text_world_transform(),
        );
        assert_eq!(reopened.trace, TraceMode::Centerline);
        assert_eq!(project.layers[0].strokes.len(), 1);
    }

    #[test]
    fn project_reproduces_gcode_without_the_font_or_automatic_substitution() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("private-font.ttf");
        std::fs::copy(font().path, &path).unwrap();
        let mut app = Studio::default();
        let mut draft = prepare(&app, None, "O日本語\nfi");
        draft.faces = text_outline::list_font_faces(&path).unwrap();
        let (art, id) =
            text_outline::outline_text_with_identity(&draft.faces[0], &draft.content, 10., 0.025)
                .unwrap();
        draft.preview = Some((0, art, id));
        app.apply_text(&draft).unwrap();
        app.rebuild();
        let expected = app.output.as_ref().unwrap().gcode.clone();
        let json = app.as_project().to_json().unwrap();
        let text = std::str::from_utf8(&json).unwrap();
        assert!(text.contains("\"version\":3"));
        assert!(!text.contains("private-font") && !text.contains(temp.path().to_str().unwrap()));
        std::fs::remove_file(path).unwrap();
        let mut other = Studio::default();
        other.apply_project(
            PathBuf::from("other-pc.penplot"),
            Project::from_json(&json).unwrap(),
        );
        other.rebuild();
        assert_eq!(expected, other.output.as_ref().unwrap().gcode);
        let missing = TextDialog::from_metadata(
            Some(0),
            other.revision,
            other.layers[0].text.as_ref(),
            other.text_world_transform(),
        );
        assert!(missing.chosen.is_none() && !missing.can_apply());
        assert!(other.apply_text(&missing).is_err());
        assert_eq!(json, other.as_project().to_json().unwrap());
    }

    #[test]
    fn editing_keeps_placed_size_rotation_anchor_and_other_objects() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(10., 10.), Point::new(22., 15.)], false)
            .unwrap();
        app.options.rotation_deg = 31.;
        app.options.scale = 1.7;
        app.options.x = 20.;
        app.options.y = 20.;
        app.options.optimize = false;
        app.rebuild();
        let other = app.planned[0].points.clone();
        let mut draft = prepare(&app, None, "O");
        draft.x = 40.;
        draft.y = 40.;
        app.apply_text(&draft).unwrap();
        app.rebuild();
        for (a, b) in other.iter().zip(&app.planned[0].points) {
            near(*a, *b);
        }
        let text_before = app.planned[1..].to_vec();
        let draft = prepare(&app, Some(1), "O");
        assert!((draft.em_mm - 10.).abs() < 1e-8);
        assert!((draft.x - 40.).abs() < 1e-8);
        app.apply_text(&draft).unwrap();
        app.rebuild();
        for (a, b) in text_before.iter().zip(&app.planned[1..]) {
            for (a, b) in a.points.iter().zip(&b.points) {
                near(*a, *b);
            }
        }
        app.options.scale *= 2.;
        app.options.rotation_deg += 20.;
        app.invalidate();
        app.rebuild();
        let draft = prepare(&app, Some(1), "O");
        assert!((draft.em_mm - 20.).abs() < 1e-8);
        assert!((draft.rotation_deg - 20.).abs() < 1e-8);
        let before = app.planned.clone();
        app.apply_text(&draft).unwrap();
        app.rebuild();
        for (a, b) in before.iter().zip(&app.planned) {
            for (a, b) in a.points.iter().zip(&b.points) {
                near(*a, *b);
            }
        }
    }

    #[test]
    fn replacement_stale_results_and_document_revision_need_review() {
        let mut app = Studio::default();
        app.apply_text(&prepare(&app, None, "O")).unwrap();
        let mut draft = prepare(&app, Some(0), "fi");
        draft.original_font.as_mut().unwrap().sha256 = "a".repeat(64);
        let before = app.as_project().to_json().unwrap();
        let undo = app.undo.len();
        assert!(draft.replacement_needed() && !draft.can_apply());
        assert!(app.apply_text(&draft).is_err());
        assert_eq!(before, app.as_project().to_json().unwrap());
        assert_eq!(undo, app.undo.len());
        draft.replacement_confirmed = true;
        assert!(draft.can_apply());
        app.apply_text(&draft).unwrap();
        let mut stale = prepare(&app, Some(0), "O");
        let (_, art, id) = stale.preview.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        stale.worker = Some((stale.revision, receiver));
        stale.invalidate();
        sender.send(Ok((art, id))).unwrap();
        stale.poll(&egui::Context::default());
        assert!(stale.preview.is_none() && !stale.can_apply());
        let stale = prepare(&app, Some(0), "O");
        app.invalidate();
        assert!(
            app.apply_text(&stale)
                .unwrap_err()
                .contains("document changed")
        );
    }

    #[test]
    fn text_modal_accepts_unicode_ime_events_without_deleting_the_selected_layer() {
        let mut app = Studio::default();
        app.apply_text(&prepare(&app, None, "O")).unwrap();
        let mut dialog = prepare(&app, Some(0), "O");
        dialog.content.clear();
        dialog.preview = None;
        app.text_dialog = Some(dialog);
        let ctx = egui::Context::default();
        let frame = |app: &mut Studio, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1100., 850.))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    app.show_text_dialog(ctx);
                    app.object_shortcuts(ctx);
                },
            )
        };
        let _ = frame(&mut app, vec![]);
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("text_body")));
        let _ = frame(&mut app, vec![egui::Event::Ime(egui::ImeEvent::Enabled)]);
        let _ = frame(
            &mut app,
            vec![egui::Event::Ime(egui::ImeEvent::Preedit("にほんご".into()))],
        );
        let _ = frame(
            &mut app,
            vec![egui::Event::Ime(egui::ImeEvent::Commit("日本語".into()))],
        );
        let _ = frame(
            &mut app,
            vec![
                egui::Event::Ime(egui::ImeEvent::Disabled),
                egui::Event::Text("\nfi".into()),
            ],
        );
        assert_eq!(app.text_dialog.as_ref().unwrap().content, "日本語\nfi");
        let _ = frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::Delete,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(app.layers.len(), 1);
        assert_eq!(app.layers[0].text.as_ref().unwrap().content, "O");
    }

    #[test]
    fn standard_local_fonts_shape_mixed_text_when_available() {
        for (path, text) in [
            (
                "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                "Office e\u{301} ffi\nمرحبا",
            ),
            (
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                "がぎぐげご パピプペポ\n日本語 ABC 123",
            ),
        ] {
            if !Path::new(path).is_file() {
                continue;
            }
            let font = text_outline::list_font_faces(Path::new(path))
                .unwrap()
                .remove(0);
            let (art, id) =
                text_outline::outline_text_with_identity(&font, text, 10., 0.025).unwrap();
            assert!(!art.strokes.is_empty());
            assert_eq!(id.sha256.len(), 64);
            assert!(art.strokes.iter().all(|s| s.closed && s.points.len() >= 2));
            let (paths, same_id) = text_outline::trace_text_with_identity(
                &font,
                text,
                10.,
                0.025,
                TraceMode::Centerline,
            )
            .unwrap();
            assert_eq!(id, same_id);
            assert!(paths.strokes.iter().any(|s| !s.closed));
            assert!(
                paths
                    .strokes
                    .iter()
                    .all(|s| !s.filled && s.points.len() >= 2)
            );
        }
    }
}
