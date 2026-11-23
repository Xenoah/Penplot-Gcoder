//! Object selection/deletion adapter. Kept separate for rebasing onto the restored
//! planner and for future text objects (which select/delete as ordinary layers).
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Selection {
    Layer(usize),
    Mark { layer: usize, stroke: usize },
}
impl Selection {
    pub(super) fn layer(self) -> usize {
        match self {
            Self::Layer(layer) | Self::Mark { layer, .. } => layer,
        }
    }
    fn with_layer(self, layer: usize) -> Self {
        match self {
            Self::Layer(_) => Self::Layer(layer),
            Self::Mark { stroke, .. } => Self::Mark { layer, stroke },
        }
    }
    pub(super) fn contains(self, target: Self) -> bool {
        match self {
            Self::Layer(layer) => layer == target.layer(),
            Self::Mark { .. } => self == target,
        }
    }
    fn delete_label(self) -> &'static str {
        match self {
            Self::Layer(_) => "Delete layer",
            Self::Mark { .. } => "Delete mark",
        }
    }
}

/// A menu keeps its own target and document revision. It never acts on whatever
/// happens to be selected when its Delete button is eventually clicked.
#[derive(Clone, Copy, Debug)]
pub(super) struct ObjectContext {
    target: Option<Selection>,
    revision: u64,
}

fn is_mark_layer(layer: &Layer) -> bool {
    layer.text.is_none()
        && layer.raster.is_none()
        && layer.name == "Canvas marks"
        && !layer.strokes.is_empty()
        && layer.strokes.iter().all(|s| s.layer == "Canvas marks")
}

fn segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0. {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0., 1.);
    p.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

fn hit_test(
    strokes: &[Stroke],
    targets: &[Option<Selection>],
    p: Point,
    tolerance: f64,
) -> Option<Selection> {
    if !p.x.is_finite() || !p.y.is_finite() || !tolerance.is_finite() || tolerance < 0. {
        return None;
    }
    // Last painted geometry wins at overlaps. No bounding-box or pen-up hit.
    strokes
        .iter()
        .zip(targets)
        .rev()
        .find_map(|(stroke, target)| {
            let target = (*target)?;
            let hit = stroke
                .points
                .windows(2)
                .any(|s| segment_distance(p, s[0], s[1]) <= tolerance)
                || (stroke.closed
                    && stroke.points.len() >= 2
                    && segment_distance(p, stroke.points[0], *stroke.points.last().unwrap())
                        <= tolerance);
            hit.then_some(target)
        })
}

impl Studio {
    pub(super) fn build_route_targets(&self) -> Vec<Option<Selection>> {
        let Ok(combined) = self.combined() else {
            return vec![None; self.planned.len()];
        };
        let mut identities = BTreeMap::new();
        let mut cursor = 0;
        for (layer_index, layer) in self
            .layers
            .iter()
            .enumerate()
            .filter(|(_, layer)| layer.visible)
        {
            for (stroke_index, source) in combined[cursor..cursor + layer.strokes.len()]
                .iter()
                .enumerate()
            {
                let target = if is_mark_layer(layer) {
                    Selection::Mark {
                        layer: layer_index,
                        stroke: stroke_index,
                    }
                } else {
                    Selection::Layer(layer_index)
                };
                let identity = (layer_index, source.group);
                // Ambiguous old mark groups cannot safely identify one mark.
                identities
                    .entry(identity)
                    .and_modify(|existing| {
                        if *existing != Some(target) {
                            *existing = None;
                        }
                    })
                    .or_insert(Some(target));
            }
            cursor += layer.strokes.len();
        }
        self.planned
            .iter()
            .map(|s| {
                let layer = s.layer.split_once(':')?.0.parse::<usize>().ok()?;
                identities.get(&(layer, s.group)).copied().flatten()
            })
            .collect()
    }

    fn valid_selection(&self, target: Selection) -> bool {
        self.layers
            .get(target.layer())
            .is_some_and(|layer| match target {
                Selection::Layer(_) => true,
                Selection::Mark { stroke, .. } => {
                    is_mark_layer(layer) && stroke < layer.strokes.len()
                }
            })
    }

    /// Deletion is a single history edit. Preserve the global placement shift so
    /// removing a leftmost object cannot move all the remaining artwork.
    pub(super) fn delete_object(&mut self, target: Selection) -> bool {
        if !self.valid_selection(target) {
            return false;
        }
        let (sin, cos) = self
            .options
            .rotation_deg
            .rem_euclid(360.)
            .to_radians()
            .sin_cos();
        let before = projected_min(&self.layers, self.options.scale, sin, cos);
        self.selected = Some(target);
        self.record_layers();
        match target {
            Selection::Layer(layer) => {
                self.layers.remove(layer);
            }
            Selection::Mark { layer, stroke } => {
                let mut strokes = self.layers[layer].strokes.to_vec();
                strokes.remove(stroke);
                if strokes.is_empty() {
                    self.layers.remove(layer);
                } else {
                    self.layers[layer].strokes = strokes.into();
                }
            }
        }
        if let (Some(before), Some(after)) = (
            before,
            projected_min(&self.layers, self.options.scale, sin, cos),
        ) {
            self.options.x += after.x - before.x;
            self.options.y += after.y - before.y;
        }
        self.selected = None;
        self.draft.clear();
        self.invalidate();
        self.status = "Deleted selected object. Ctrl+Z restores it.".into();
        true
    }

    fn object_edits_blocked(&self, ctx: &egui::Context) -> bool {
        self.text_dialog.is_some()
            || self.raster_import.is_busy()
            || self.placement_drag.is_some()
            || self.import_task.is_some()
            || self.import_dialog.is_some()
            || self.pending_open.is_some()
            || self.confirm_quit
            || ctx.memory(|m| m.top_modal_layer().is_some())
            || ctx.dragged_id().is_some()
            || !self.draft.is_empty()
    }

    /// Run after widgets and modals, so text/number/IME editors get the event first.
    pub(super) fn object_shortcuts(&mut self, ctx: &egui::Context) {
        if self.object_edits_blocked(ctx)
            || ctx.wants_keyboard_input()
            || ctx.input(|i| i.pointer.any_down())
            || egui::Popup::is_any_open(ctx)
            || self.help_open
            || self.show_gcode
        {
            return;
        }
        let (undo, redo) = ctx.input(|i| {
            (
                i.modifiers.command && !i.modifiers.shift && i.key_pressed(Key::Z),
                i.modifiers.command
                    && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z))),
            )
        });
        if redo {
            self.redo();
        } else if undo {
            self.undo();
        } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Delete))
            && let Some(target) = self.selected
        {
            self.delete_object(target);
        }
    }

    fn open_object_context(&mut self, target: Option<Selection>) {
        self.selected = target;
        self.object_context = Some(ObjectContext {
            target,
            revision: self.revision,
        });
    }

    fn context_delete_target(&self) -> Option<Selection> {
        let context = self.object_context?;
        let target = context.target?;
        (context.revision == self.revision && self.valid_selection(target)).then_some(target)
    }

    pub(super) fn object_canvas_ui(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
        map: CanvasMap,
    ) {
        if self.object_edits_blocked(ui.ctx()) {
            return;
        }
        let current = !self.preview_dirty && self.plan_task.is_none();
        if response.secondary_clicked()
            || ((self.tools == Tool::Pan || self.tools.placement()) && response.clicked())
        {
            let target = if current {
                response.interact_pointer_pos().and_then(|p| {
                    hit_test(
                        &self.planned,
                        &self.route_targets,
                        map.paper(p),
                        6.0 / f64::from(map.scale),
                    )
                })
            } else {
                None
            };
            self.selected = target;
            if response.secondary_clicked() {
                self.open_object_context(target);
            }
        }
        let mut delete = None;
        let mut edit_text = None;
        response.context_menu(|ui| {
            if let Some(target) = self.context_delete_target() {
                ui.label(&self.layers[target.layer()].name);
                if self.layers[target.layer()].text.is_some() && ui.button("Edit text…").clicked()
                {
                    edit_text = Some(target.layer());
                    ui.close();
                }
                if ui.button(target.delete_label()).clicked() {
                    delete = Some(target);
                    ui.close();
                }
                if ui.button("Deselect").clicked() {
                    self.selected = None;
                    ui.close();
                }
            } else {
                ui.label("No object here. Select a drawn line or a layer.");
            }
        });
        if let Some(index) = edit_text {
            self.open_text_dialog(Some(index));
        }
        if let Some(target) = delete {
            self.delete_object(target);
        }
    }

    pub(super) fn artwork_panel(&mut self, ui: &mut egui::Ui) {
        section(ui, "LAYERS");
        if self.layers.is_empty() {
            ui.label(RichText::new("Drop SVG, DXF, PNG or JPEG files here.").weak());
        }
        let blocked = self.object_edits_blocked(ui.ctx());
        let mut action = None;
        let mut delete = None;
        let mut selected = self.selected;
        let mut context = self.object_context;
        let mut edit_text = None;
        for (index, layer) in self.layers.iter().enumerate() {
            ui.add_enabled_ui(!blocked, |ui| {
                ui.horizontal(|ui| {
                    let mut visible = layer.visible;
                    if ui
                        .checkbox(&mut visible, "")
                        .on_hover_text("Include in drawing")
                        .changed()
                    {
                        action = Some((index, LayerAction::Visibility(visible)));
                    }
                    let response = ui.selectable_label(
                        selected.is_some_and(|s| s.layer() == index),
                        format!("{}  ·  {}", layer.name, layer.strokes.len()),
                    );
                    let target = Selection::Layer(index);
                    if response.clicked() {
                        selected = Some(target);
                    }
                    if response.secondary_clicked() {
                        selected = Some(target);
                        context = Some(ObjectContext {
                            target: Some(target),
                            revision: self.revision,
                        });
                    }
                    response.context_menu(|ui| {
                        if context.is_some_and(|c| {
                            c.revision == self.revision && c.target == Some(target)
                        }) {
                            ui.label(&layer.name);
                            if layer.text.is_some() && ui.button("Edit text…").clicked() {
                                edit_text = Some(index);
                                ui.close();
                            }
                            if ui.button("Delete layer").clicked() {
                                delete = Some(target);
                                ui.close();
                            }
                            if ui.button("Deselect").clicked() {
                                selected = None;
                                ui.close();
                            }
                        } else {
                            ui.label("Object changed. Reopen this menu.");
                        }
                    });
                })
            });
        }
        self.selected = selected;
        self.object_context = context;
        if let Some(target) = self.selected.filter(|&target| self.valid_selection(target)) {
            let index = target.layer();
            ui.small(match target {
                Selection::Layer(_) => "Selected: whole layer",
                Selection::Mark { .. } => "Selected: one canvas mark",
            });
            ui.add_enabled_ui(!blocked, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(index > 0, egui::Button::new("Move up"))
                        .clicked()
                    {
                        action = Some((index, LayerAction::Move(index - 1)));
                    }
                    if ui
                        .add_enabled(
                            index + 1 < self.layers.len(),
                            egui::Button::new("Move down"),
                        )
                        .clicked()
                    {
                        action = Some((index, LayerAction::Move(index + 1)));
                    }
                    if self.layers[index].text.is_some() && ui.button("Edit text…").clicked() {
                        edit_text = Some(index);
                    }
                    if ui.button(target.delete_label()).clicked() {
                        delete = Some(target);
                    }
                })
            });
        }
        if let Some(index) = edit_text {
            self.open_text_dialog(Some(index));
        }
        if let Some(target) = delete {
            self.delete_object(target);
        } else if let Some((index, action)) = action {
            self.record_layers();
            match action {
                LayerAction::Visibility(visible) => self.layers[index].visible = visible,
                LayerAction::Move(target) => {
                    self.layers.swap(index, target);
                    self.selected = self.selected.map(|s| s.with_layer(target));
                }
            }
            self.invalidate();
        }
        if let Some(index) = self
            .selected
            .map(Selection::layer)
            .filter(|&index| index < self.layers.len())
            && let Some(mut settings) = self.layers[index].raster.clone()
        {
            ui.separator();
            section(ui, "IMAGE LAYER FILL");
            if self.layers[index].strokes.iter().any(|s| s.filled) {
                raster_ui::raster_fill_controls(ui, &mut settings, "selected_raster_fill");
            } else {
                ui.small("Centerline paths do not support hatching.");
            }
            if self.layers[index].raster.as_ref() != Some(&settings) {
                self.record_layers();
                self.layers[index].raster = Some(settings);
                self.invalidate();
            }
            ui.small(
                "Reimport the original image to change trace mode, threshold, DPI, or inversion.",
            );
        }
        ui.separator();
        self.placement_panel(ui);
        ui.separator();
        self.import_panel(ui);
        if !self.notices.is_empty() {
            ui.separator();
            section(ui, "IMPORT NOTES");
            for notice in &self.notices {
                ui.colored_label(WARN, notice);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks() -> Studio {
        let mut app = Studio::default();
        for y in [10., 20., 30.] {
            app.add_mark(vec![Point::new(10., y), Point::new(30., y)], false)
                .unwrap();
        }
        app.rebuild();
        app
    }

    fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn frame(ctx: &egui::Context, app: &mut Studio, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700., 500.))),
                modifiers: events
                    .iter()
                    .find_map(|event| match event {
                        egui::Event::Key { modifiers, .. } => Some(*modifiers),
                        _ => None,
                    })
                    .unwrap_or_default(),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let (response, _) =
                        ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
                    app.object_canvas_ui(
                        ui,
                        &response,
                        CanvasMap {
                            anchor: Pos2::new(40., 350.),
                            scale: 5.,
                        },
                    );
                });
                app.object_shortcuts(ctx);
            },
        )
    }

    fn click(
        ctx: &egui::Context,
        app: &mut Studio,
        at: Pos2,
        button: egui::PointerButton,
    ) -> egui::FullOutput {
        frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
        frame(
            ctx,
            app,
            vec![egui::Event::PointerButton {
                pos: at,
                button,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        frame(
            ctx,
            app,
            vec![egui::Event::PointerButton {
                pos: at,
                button,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
    }

    fn button_center(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
        fn find(shape: &egui::Shape, label: &str) -> Option<Pos2> {
            match shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, label)),
                _ => None,
            }
        }
        output.shapes.iter().find_map(|s| find(&s.shape, label))
    }

    #[test]
    fn keyboard_delete_targets_one_mark_and_undo_redo_restore_selection_and_caches() {
        let mut app = marks();
        let original = app.layers[0].strokes.to_vec();
        let context = egui::Context::default();
        frame(&context, &mut app, vec![]);
        click(
            &context,
            &mut app,
            Pos2::new(140., 250.),
            egui::PointerButton::Primary,
        );
        assert_eq!(
            app.selected,
            Some(Selection::Mark {
                layer: 0,
                stroke: 1
            })
        );
        assert_eq!(app.route_targets.len(), 3);
        frame(
            &context,
            &mut app,
            vec![key(Key::Delete, egui::Modifiers::NONE)],
        );
        assert_eq!(app.layers[0].strokes.len(), 2);
        assert_eq!(app.layers[0].strokes[0], original[0]);
        assert_eq!(app.layers[0].strokes[1], original[2]);
        assert!(app.selected.is_none());
        assert!(app.output.is_none() && !app.can_export());
        assert!(app.route_targets.is_empty() && app.planned.is_empty());
        assert!(app.project_dirty && app.preview_dirty && !app.can_export());
        frame(
            &context,
            &mut app,
            vec![key(Key::Z, egui::Modifiers::COMMAND)],
        );
        assert_eq!(&*app.layers[0].strokes, original.as_slice());
        assert_eq!(
            app.selected,
            Some(Selection::Mark {
                layer: 0,
                stroke: 1
            })
        );
        app.rebuild();
        assert_eq!(app.route_targets.len(), 3);
        assert!(app.output.is_some());
        frame(
            &context,
            &mut app,
            vec![key(Key::Y, egui::Modifiers::COMMAND)],
        );
        assert_eq!(app.layers[0].strokes.len(), 2);
    }

    #[test]
    fn canvas_context_targets_cursor_mark_and_blank_has_no_delete_action() {
        let mut app = marks();
        let context = egui::Context::default();
        frame(&context, &mut app, vec![]);
        app.selected = Some(Selection::Mark {
            layer: 0,
            stroke: 0,
        });
        click(
            &context,
            &mut app,
            Pos2::new(140., 200.),
            egui::PointerButton::Secondary,
        );
        let output = frame(&context, &mut app, vec![]);
        assert_eq!(
            app.selected,
            Some(Selection::Mark {
                layer: 0,
                stroke: 2
            })
        );
        let button = button_center(&output, "Delete mark").expect("context delete action");
        click(&context, &mut app, button, egui::PointerButton::Primary);
        assert_eq!(app.layers[0].strokes.len(), 2);
        assert_eq!(app.layers[0].strokes[1].points[0].y, 20.);
        app.rebuild();
        app.selected = Some(Selection::Layer(0));
        click(
            &context,
            &mut app,
            Pos2::new(400., 400.),
            egui::PointerButton::Secondary,
        );
        let output = frame(&context, &mut app, vec![]);
        assert!(app.selected.is_none());
        assert!(button_center(&output, "Delete layer").is_none());
        assert!(button_center(&output, "Delete mark").is_none());
        frame(
            &context,
            &mut app,
            vec![key(Key::Delete, egui::Modifiers::NONE)],
        );
        assert_eq!(app.layers[0].strokes.len(), 2);
    }

    #[test]
    fn layer_list_context_selects_and_removes_that_layer_not_the_previous_selection() {
        let mut app = marks();
        let mark = app.layers[0].strokes[0].clone();
        app.layers = vec![
            Layer {
                name: "first.svg".into(),
                strokes: vec![mark.clone()].into(),
                visible: true,
                raster: None,
                text: None,
            },
            Layer {
                name: "second.png".into(),
                strokes: vec![mark].into(),
                visible: true,
                raster: None,
                text: None,
            },
        ];
        app.selected = Some(Selection::Layer(0));
        app.rebuild();
        let ctx = egui::Context::default();
        let panel = |app: &mut Studio, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(700., 700.))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.artwork_panel(ui));
                    app.object_shortcuts(ctx);
                },
            )
        };
        let output = panel(&mut app, vec![]);
        let pos = button_center(&output, "second.png  ·  1").unwrap();
        panel(&mut app, vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            panel(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        let output = panel(&mut app, vec![]);
        assert_eq!(app.selected, Some(Selection::Layer(1)));
        // The popup is the last painted Delete layer text (the sidebar also has one).
        fn last_delete(output: &egui::FullOutput) -> Pos2 {
            output
                .shapes
                .iter()
                .rev()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == "Delete layer" => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
                .unwrap()
        }
        let pos = last_delete(&output);
        panel(&mut app, vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            panel(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        assert_eq!(app.layers.len(), 1);
        assert_eq!(app.layers[0].name, "first.svg");
        app.undo();
        assert_eq!(app.layers[1].name, "second.png");
    }

    #[test]
    fn source_mapping_keeps_imports_whole_and_matches_closed_edges_not_travel_or_blank() {
        let mut app = marks();
        let mut rect = app.layers[0].strokes[0].clone();
        rect.points = vec![
            Point::new(50., 10.),
            Point::new(60., 10.),
            Point::new(60., 20.),
            Point::new(50., 20.),
        ];
        rect.closed = true;
        rect.filled = true;
        rect.layer = "#010203".into();
        app.layers.insert(
            0,
            Layer {
                name: "hidden.svg".into(),
                strokes: vec![rect.clone()].into(),
                visible: false,
                raster: None,
                text: None,
            },
        );
        app.layers.push(Layer {
            name: "image.png".into(),
            strokes: vec![rect].into(),
            visible: true,
            raster: None,
            text: None,
        });
        app.options.fill = Fill::Lines;
        app.options.hatch_angle = 0.;
        app.rebuild();
        assert_eq!(
            hit_test(&app.planned, &app.route_targets, Point::new(50., 15.), 0.05),
            Some(Selection::Layer(2))
        );
        assert_eq!(
            hit_test(
                &app.planned,
                &app.route_targets,
                Point::new(55., 15.5),
                0.05
            ),
            Some(Selection::Layer(2))
        );
        assert_eq!(
            hit_test(&app.planned, &app.route_targets, Point::new(10., 15.), 0.05),
            None
        );
        assert_eq!(
            hit_test(&app.planned, &app.route_targets, Point::new(0., 0.), 0.05),
            None
        );
        assert!(app.delete_object(Selection::Layer(2)));
        assert_eq!(app.layers.len(), 2);
        assert_eq!(app.layers[1].strokes.len(), 3);
    }

    #[test]
    fn deletion_preserves_placed_coordinates_across_rotation_and_scale_and_last_item_is_safe() {
        let mut app = marks();
        app.options.rotation_deg = 47.;
        app.options.scale = 1.8;
        app.rebuild();
        let surviving = app.planned[1..].to_vec();
        assert!(app.delete_object(Selection::Mark {
            layer: 0,
            stroke: 0
        }));
        app.rebuild();
        for (actual, before) in app.planned.iter().zip(&surviving) {
            for (a, b) in actual.points.iter().zip(&before.points) {
                assert!(a.distance(*b) < 1e-10);
            }
        }
        assert!(app.delete_object(Selection::Layer(0)));
        assert!(app.layers.is_empty() && app.planned.is_empty() && app.route_targets.is_empty());
        assert!(!app.can_export());
        app.rebuild();
        assert!(app.output.is_none());
        assert!(penplot::generate_air_run(&app.planned, &app.profile).is_err());
        app.undo();
        app.rebuild();
        assert_eq!(app.layers[0].strokes.len(), 2);
        assert!(app.output.is_some());
    }

    #[test]
    fn menus_and_picking_are_invalidated_and_new_marks_keep_unique_source_groups() {
        let mut app = marks();
        let target = Selection::Mark {
            layer: 0,
            stroke: 1,
        };
        app.open_object_context(Some(target));
        assert_eq!(app.context_delete_target(), Some(target));
        app.invalidate();
        assert!(app.context_delete_target().is_none());
        assert!(app.route_targets.is_empty());
        app.rebuild();
        app.delete_object(target);
        app.add_mark(vec![Point::new(10., 40.), Point::new(30., 40.)], false)
            .unwrap();
        assert_eq!(
            app.layers[0]
                .strokes
                .iter()
                .map(|s| s.group)
                .collect::<Vec<_>>(),
            vec![0, 2, 3]
        );
        app.rebuild();
        assert!(app.route_targets.iter().all(Option::is_some));
        assert_eq!(
            hit_test(&app.planned, &app.route_targets, Point::new(20., 40.), 0.1),
            Some(Selection::Mark {
                layer: 0,
                stroke: 2
            })
        );
    }

    #[test]
    fn raster_and_placement_work_block_deletion_and_stale_export() {
        let mut app = marks();
        let ctx = egui::Context::default();
        app.raster_import.extend([PathBuf::from("queued.png")]);
        assert!(!app.can_export());
        frame(
            &ctx,
            &mut app,
            vec![key(Key::Delete, egui::Modifiers::NONE)],
        );
        assert_eq!(app.layers[0].strokes.len(), 3);
        app.raster_import = RasterImport::default();
        let bounds = app.placement_bounds().unwrap();
        app.placement_drag = Some(PlacementDrag {
            bounds,
            cursor_start: bounds[0],
            handle: PlacementHandle::Move,
            before: app.options.clone(),
            motion: PlacementMotion::Move(Point::new(1., 2.)),
        });
        assert!(!app.can_export());
        frame(
            &ctx,
            &mut app,
            vec![key(Key::Delete, egui::Modifiers::NONE)],
        );
        assert_eq!(app.layers[0].strokes.len(), 3);
        app.placement_drag = None;
        let (sender, receiver) = mpsc::channel();
        app.plan_task = Some(receiver);
        assert!(!app.can_export());
        assert!(app.delete_object(Selection::Mark {
            layer: 0,
            stroke: 0
        }));
        assert!(!app.can_export());
        assert!(app.route_targets.is_empty());
        drop(sender);
    }

    #[test]
    fn delete_belongs_to_numeric_editors_while_they_have_focus() {
        let mut app = marks();
        let ctx = egui::Context::default();
        let mut number = 123.0;
        let mut id = None;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.add(egui::DragValue::new(&mut number));
                id = Some(response.id);
                response.request_focus();
            });
        });
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key(Key::Delete, egui::Modifiers::NONE)],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.add(egui::DragValue::new(&mut number));
                });
                assert!(ctx.wants_keyboard_input());
                app.object_shortcuts(ctx);
            },
        );
        assert_eq!(app.layers[0].strokes.len(), 3);
        assert!(id.is_some());
    }

    #[test]
    fn delete_does_not_steal_text_editing_modal_drag_or_drawing_input() {
        let mut app = marks();
        app.selected = Some(Selection::Layer(0));
        let ctx = egui::Context::default();
        let mut text = "ab".to_owned();
        let edit = |app: &mut Studio, text: &mut String, events| {
            ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add(egui::TextEdit::singleline(text).id(egui::Id::new("text")))
                            .request_focus();
                    });
                    app.object_shortcuts(ctx);
                },
            )
        };
        edit(&mut app, &mut text, vec![]);
        edit(
            &mut app,
            &mut text,
            vec![key(Key::Home, egui::Modifiers::NONE)],
        );
        edit(
            &mut app,
            &mut text,
            vec![key(Key::Delete, egui::Modifiers::NONE)],
        );
        assert_eq!(text, "b");
        assert_eq!(app.layers.len(), 1);
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("text")));
        // App modal state protects its first frame: egui exposes the modal
        // layer only from the preceding frame.
        app.confirm_quit = true;
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key(Key::Delete, egui::Modifiers::NONE)],
                ..Default::default()
            },
            |ctx| {
                egui::Modal::new(egui::Id::new("modal")).show(ctx, |ui| {
                    ui.label("modal");
                });
                app.object_shortcuts(ctx);
            },
        );
        assert_eq!(app.layers.len(), 1);
        app.confirm_quit = false;
        let ctx = egui::Context::default();
        app.draft.push(Point::new(2., 3.));
        frame(
            &ctx,
            &mut app,
            vec![key(Key::Delete, egui::Modifiers::NONE)],
        );
        assert_eq!(app.layers.len(), 1);
        app.draft.clear();
        frame(&ctx, &mut app, vec![]);
        let pos = Pos2::new(500., 400.);
        frame(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(pos + egui::vec2(30., 10.)),
                key(Key::Delete, egui::Modifiers::NONE),
            ],
        );
        assert_eq!(app.layers.len(), 1);
    }
}
