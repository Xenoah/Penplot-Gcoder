//! Workspaces change presentation only; document, camera and import transactions live in Studio.
use super::*;

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq)]
pub(super) enum Workspace {
    #[default]
    Edit,
    Import,
    Preview,
}

impl Workspace {
    fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Import => "Import",
            Self::Preview => "G-code Preview",
        }
    }
}

fn dock_widths(width: f32) -> (f32, f32) {
    (
        (width * 0.19).clamp(150.0, 240.0),
        (width * 0.22).clamp(180.0, 280.0),
    )
}

impl Studio {
    pub(super) fn switch_workspace(&mut self, workspace: Workspace) {
        if self.workspace != workspace {
            self.workspace = workspace;
            // A pointer gesture cannot continue in a different work surface.
            // Neither committed geometry nor its history/camera is changed.
            self.placement_drag = None;
            self.object_context = None;
        }
        self.show_gcode = workspace == Workspace::Preview;
    }

    pub(super) fn workspace_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for workspace in [Workspace::Edit, Workspace::Import, Workspace::Preview] {
                if ui
                    .selectable_label(self.workspace == workspace, workspace.label())
                    .clicked()
                {
                    self.switch_workspace(workspace);
                }
            }
            if self.raster_import.is_busy() {
                ui.label(
                    RichText::new("Image import pending · return to Import")
                        .small()
                        .color(WARN),
                );
            }
        });
    }

    pub(super) fn workspace_ui(&mut self, ctx: &egui::Context) {
        let (left, right) = dock_widths(ctx.screen_rect().width());
        egui::SidePanel::left("tool_dock").exact_width(left).resizable(false).show(ctx, |ui| {
            egui::ScrollArea::vertical().id_salt(("tools", self.workspace)).show(ui, |ui| {
                match self.workspace {
                    Workspace::Edit => {
                        section(ui, "TOOLBOX");
                        ui.add_enabled_ui(!self.raster_import.is_busy() && self.text_dialog.is_none(), |ui| {
                            for tool in [Tool::Pan, Tool::Transform, Tool::Rotate, Tool::Bucket, Tool::Line, Tool::Rectangle, Tool::Ellipse, Tool::Freehand] {
                                if ui.add_sized([ui.available_width(), 28.0],
                                    egui::Button::selectable(self.tools == tool, tool.label_localized(self.japanese))).clicked() {
                                    self.tools = tool;
                                    if tool == Tool::Bucket {self.isometric=false; self.grayscale_preview=true;}
                                    self.draft.clear();
                                    self.placement_drag = None;
                                }
                            }
                            if ui.add_sized([ui.available_width(), 28.0], egui::Button::new("Text…")).clicked() {
                                self.open_text_dialog(None);
                            }
                            ui.separator();
                            section(ui, "TOOL OPTIONS");
                            ui.small(match self.tools {
                                Tool::Bucket => "Click a closed region to fill it with a monochrome pen pattern.",
                                Tool::Transform => "Select a layer. Drag inside to move it, or drag a corner to scale it.",
                                Tool::Rotate => "Drag the selected layer’s round handle. Shift snaps to 15°.",
                                Tool::Pan => "Drag to pan. Scroll to zoom. Click a path to select.",
                                _ => "Drag on the bed to draw. Escape cancels the current stroke.",
                            });
                            if self.tools == Tool::Bucket {self.bucket_panel(ui);} else {self.placement_panel(ui);}
                        });
                    }
                    Workspace::Import => {
                        section(ui, "SOURCES");
                        if ui.add_enabled(!self.raster_import.is_busy() && self.import_task.is_none() && self.import_dialog.is_none(), egui::Button::new("Choose artwork…")).clicked() {
                            self.select_files(&ui.ctx().clone());
                        }
                        ui.small("SVG / DXF / PNG / JPEG");
                        self.import_panel(ui);
                        ui.separator();
                        self.min_stroke_controls(ui);
                        ui.separator();
                        for notice in &self.notices { ui.colored_label(WARN, notice); }
                    }
                    Workspace::Preview => {
                        self.machine_panel(ui);
                        ui.separator();
                        self.min_stroke_controls(ui);
                        ui.separator();
                        self.route_panel(ui);
                    }
                }
            });
        });
        egui::SidePanel::right("layer_dock")
            .exact_width(right)
            .resizable(false)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("layers_scroll")
                    .show(ui, |ui| {
                        ui.small("Visible layers are included in the output.");
                        self.artwork_panel(ui);
                    });
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            #[cfg(test)]
            {
                self.canvas_rect = ui.available_rect_before_wrap();
            }
            match self.workspace {
                Workspace::Import if self.raster_import.is_busy() => {
                    if self.import_task.is_some() {
                        ui.spinner();
                        ui.label("Reading vector files… image settings will follow.");
                    } else if let Some((item, settings)) =
                        self.raster_import.show(ui, &self.import_options)
                    {
                        self.add_raster_layer(item, settings);
                    }
                }
                Workspace::Preview => self.preview_workspace(ui),
                _ => {
                    ui.add_enabled_ui(!self.raster_import.is_busy(), |ui| self.canvas(ui));
                }
            }
        });
    }

    fn preview_workspace(&mut self, ui: &mut egui::Ui) {
        if let Some(reason) = self.profile.export_blocker() {
            ui.heading("G-code Preview");
            ui.colored_label(super::WARN, reason);
            ui.label("You can inspect the artwork layout in Edit. This profile does not expose runnable commands here.");
            return;
        }
        if self.export_inspection.is_none() && self.can_export() {
            self.inspect_gcode(self.air_first);
        }
        ui.horizontal_wrapped(|ui| {
            ui.heading("G-code Preview");
            if ui
                .add_enabled(
                    self.can_export(),
                    egui::Button::new("Refresh from current job"),
                )
                .clicked()
            {
                self.inspect_gcode(self.air_first);
            }
        });
        if let Some(inspection) = &mut self.export_inspection {
            egui::ScrollArea::vertical()
                .id_salt("gcode_workspace")
                .show(ui, |ui| {
                    ui.strong(inspection.title());
                    inspection.show(ui, self.revision);
                });
        } else {
            ui.label("Generate a route by importing or drawing artwork.");
            ui.small("This workspace inspects emitted commands. Editing does not overwrite a saved preview.");
        }
    }

    pub(super) fn export_unavailable_reason(&self) -> String {
        if self.text_dialog.is_some() || self.raster_import.is_busy() {
            "Apply or cancel the pending text/image import before exporting.".into()
        } else if self.import_dialog.is_some() || self.import_task.is_some() {
            "Wait for the import to finish before exporting.".into()
        } else if self.placement_drag.is_some() {
            "Finish or cancel the current placement gesture before exporting.".into()
        } else if let Some(reason) = self.profile.export_blocker() {
            reason.into()
        } else if let Some(error) = &self.error {
            format!("Cannot export: {error}")
        } else if !self.layers.iter().any(|layer| layer.visible) {
            "No visible artwork to export. Add or show a layer.".into()
        } else if self.preview_dirty || self.plan_task.is_some() {
            "Preparing the current route…".into()
        } else {
            "No drawable paths remain. Check Minimum image stroke and the visible layers.".into()
        }
    }

    pub(super) fn request_export(&mut self, air_run: bool) {
        let pending_edit = self.text_dialog.is_some()
            || self.raster_import.is_busy()
            || self.import_dialog.is_some()
            || self.import_task.is_some()
            || self.placement_drag.is_some();
        if !pending_edit
            && (self.preview_dirty || self.plan_task.is_some())
            && self.layers.iter().any(|layer| layer.visible)
        {
            self.pending_export = Some(air_run);
            self.status = "Preparing the current route before export…".into();
        } else {
            self.pending_export = None;
            self.save_gcode(air_run);
        }
    }

    pub(super) fn finish_pending_export(&mut self) {
        if self.pending_export.is_some() && !self.preview_dirty && self.plan_task.is_none() {
            let air_run = self.pending_export.take().unwrap();
            // Revalidate after the worker completes; never save an older snapshot.
            self.save_gcode(air_run);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_machine_presets_cancels_pending_export_without_losing_artwork() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(10., 10.), Point::new(30., 20.)], false)
            .unwrap();
        app.rebuild();
        assert!(app.can_export());
        app.inspect_gcode(false);

        app.pending_export = Some(false);
        app.air_first = false;
        app.apply_machine_preset(Some(MachinePreset::BambuP1s));
        app.rebuild();
        assert!(app.pending_export.is_none());
        assert!(!app.can_export());
        assert!(app.air_first);
        assert!(
            app.export_commands(false)
                .unwrap_err()
                .contains("layout-only")
        );
        assert!(
            app.export_commands(true)
                .unwrap_err()
                .contains("layout-only")
        );
        assert_eq!(app.planned.len(), 1);
        assert_eq!(app.layers.len(), 1);

        app.apply_machine_preset(Some(MachinePreset::VoxelabAries));
        app.rebuild();
        assert!(!app.can_export());
        assert!(app.export_unavailable_reason().contains("measured Draw Z"));
        app.profile.pen_up_z = 6.;
        app.profile.pen_down_z = 2.;
        app.invalidate();
        app.rebuild();
        assert!(app.can_export());
        assert!(
            app.export_commands(false)
                .unwrap()
                .gcode
                .contains("G1 Z2.000")
        );
        assert!(
            !app.export_commands(true)
                .unwrap()
                .gcode
                .contains("G1 Z2.000")
        );
    }

    #[test]
    fn tabs_preserve_document_camera_selection_import_options_and_export_snapshot() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(10., 10.), Point::new(30., 20.)], false)
            .unwrap();
        app.rebuild();
        app.frame.zoom = 2.25;
        app.frame.shift = Vec2::new(45., -12.);
        app.import_options.threshold = 87;
        app.selected = Some(Selection::Layer(0));
        app.inspect_gcode(false);
        let revision = app.revision;
        let history = app.undo.len();
        let snapshot = app.export_commands(false).unwrap();
        let inspection_ptr = app.export_inspection.as_ref().unwrap() as *const ExportInspection;
        for workspace in [Workspace::Import, Workspace::Preview, Workspace::Edit] {
            app.switch_workspace(workspace);
            assert_eq!(app.revision, revision);
            assert_eq!(app.undo.len(), history);
            assert_eq!(app.selected, Some(Selection::Layer(0)));
            assert_eq!(app.import_options.threshold, 87);
            assert_eq!(app.frame.zoom, 2.25);
            assert_eq!(app.frame.shift, Vec2::new(45., -12.));
            assert!(app.can_export());
            assert_eq!(app.export_commands(false).unwrap().gcode, snapshot.gcode);
        }
        app.options.x = f64::NAN;
        app.invalidate();
        app.rebuild();
        for workspace in [Workspace::Edit, Workspace::Import, Workspace::Preview] {
            app.switch_workspace(workspace);
            assert!(!app.can_export());
            assert!(
                app.export_commands(false)
                    .unwrap_err()
                    .contains("Cannot export")
            );
        }
        assert_eq!(
            app.export_inspection.as_ref().unwrap() as *const ExportInspection,
            inspection_ptr
        );
    }

    #[test]
    fn deferred_export_does_not_reuse_old_commands_after_invalid_edit() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(10., 10.), Point::new(30., 20.)], false)
            .unwrap();
        app.rebuild();
        assert!(app.can_export());
        app.options.scale = f64::NAN;
        app.invalidate();
        app.request_export(false);
        assert_eq!(app.pending_export, Some(false));
        app.rebuild();
        app.finish_pending_export();
        assert!(app.pending_export.is_none());
        assert!(app.output.is_none());
        assert!(app.status.contains("Cannot export"));
        assert!(app.export_inspection.is_none());
    }

    #[test]
    fn leaving_edit_cancels_only_the_uncommitted_move_or_scale() {
        for scale in [false, true] {
            for destination in [Workspace::Import, Workspace::Preview] {
                let mut app = Studio::default();
                app.add_mark(vec![Point::new(10., 10.), Point::new(22., 20.)], false)
                    .unwrap();
                app.rebuild();
                app.project_dirty = false;
                let revision = app.revision;
                let history = app.undo.len();
                let original = app.options.clone();
                let commands = app.export_commands(false).unwrap();
                let bounds = app.placement_bounds().unwrap();
                app.placement_drag = Some(PlacementDrag {
                    bounds,
                    cursor_start: bounds[0],
                    handle: if scale {
                        PlacementHandle::Scale(bounds[2])
                    } else {
                        PlacementHandle::Move
                    },
                    layer: 0,
                    revision,
                    motion: if scale {
                        PlacementMotion::Scale(bounds[2], 1.5)
                    } else {
                        PlacementMotion::Move(Point::new(3., 5.))
                    },
                });
                assert!(!app.can_export());
                app.switch_workspace(destination);
                assert!(app.placement_drag.is_none());
                assert_eq!(app.options, original);
                assert_eq!(app.revision, revision);
                assert_eq!(app.undo.len(), history);
                assert!(!app.project_dirty);
                assert!(app.can_export());
                assert_eq!(app.export_commands(false).unwrap().gcode, commands.gcode);
            }
        }
    }

    #[test]
    fn saved_preview_survives_edits_and_inspect_uses_current_air_run_setting() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(10., 10.), Point::new(22., 20.)], false)
            .unwrap();
        app.rebuild();
        let saved_revision = app.revision;
        let commands = app.export_commands(false).unwrap();
        app.open_gcode_snapshot(Some(PathBuf::from("saved.GCODE")), false, commands);
        app.switch_workspace(Workspace::Edit);
        app.options.x += 5.;
        app.invalidate();
        app.rebuild();
        app.switch_workspace(Workspace::Preview);
        let inspection = app.export_inspection.as_ref().unwrap();
        assert_eq!(inspection.revision, saved_revision);
        assert_eq!(inspection.title(), "Exported plot G-code");
        assert_eq!(
            inspection.path.as_ref().unwrap(),
            &PathBuf::from("saved.GCODE")
        );
        let ctx = egui::Context::default();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.preview_workspace(ui));
        });
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.job.text.contains("Snapshot of earlier commands"))));
        app.air_first = true;
        app.inspect_gcode(app.air_first);
        let inspection = app.export_inspection.as_ref().unwrap();
        assert_eq!(inspection.title(), "Inspect air-run G-code");
        assert_eq!(inspection.revision, app.revision);
        assert!(inspection.path.is_none());
    }

    #[test]
    fn narrow_workspace_leaves_room_for_the_canvas() {
        for width in [760., 900., 1240., 1920.] {
            let (left, right) = dock_widths(width);
            assert!(width - left - right >= 400.);
            let context = egui::Context::default();
            let mut app = Studio::default();
            for workspace in [Workspace::Edit, Workspace::Import, Workspace::Preview] {
                app.switch_workspace(workspace);
                let output = context.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 560.))),
                        ..Default::default()
                    },
                    |ctx| app.update_ui(ctx),
                );
                assert!(!output.shapes.is_empty());
                assert!(
                    app.canvas_rect.width() >= 380.,
                    "{workspace:?}: {:?}",
                    app.canvas_rect
                );
            }
        }
    }
}
