#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use eframe::egui::{self, Color32, FontId, Key, Pos2, Rect, RichText, Sense, Stroke as Ink, Vec2};
use penplot::{
    Artwork, Fill, ImportOptions, JobOptions, MachinePreset, Origin, Output, Point, Profile,
    Project, ProjectLayer, Stroke,
};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

mod export_preview;
use export_preview::{ExportInspection, format_duration};
mod text_ui;
use text_ui::TextDialog;
mod object_edit;
use object_edit::{ObjectContext, Selection};

mod workspace;
use workspace::Workspace;

mod bucket_ui;
mod raster_ui;
use raster_ui::RasterImport;

const APP_NAME: &str = "Penplot-Gcoder";

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        #[cfg(target_os = "windows")]
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(APP_NAME)
            .set_description(&error)
            .show();
        std::process::exit(1);
    }
}

const ACCENT: Color32 = Color32::from_rgb(96, 224, 193);
const PAPER: Color32 = Color32::from_rgb(248, 248, 238);
const BED: Color32 = Color32::from_rgb(186, 200, 204);
const WARN: Color32 = Color32::from_rgb(255, 184, 108);
const GRID: Color32 = Color32::from_rgb(226, 231, 219);
const MAX_LAYER_COUNT: usize = 128;
const MAX_UNDO_STATES: usize = 32;

// Each edit snapshots the layer metadata, but the artwork is shared until edited.
#[derive(Clone)]
struct Layer {
    name: String,
    strokes: Arc<[Stroke]>,
    visible: bool,
    raster: Option<penplot::RasterSettings>,
    text: Option<penplot::TextLayer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Bucket,
    Pan,
    Transform,
    Rotate,
    Line,
    Rectangle,
    Ellipse,
    Freehand,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Self::Bucket => "Bucket fill",
            Self::Pan => "Pan",
            Self::Transform => "Move / Scale",
            Self::Rotate => "Rotate",
            Self::Line => "Line",
            Self::Rectangle => "Box",
            Self::Ellipse => "Ellipse",
            Self::Freehand => "Freehand",
        }
    }

    fn placement(self) -> bool {
        matches!(self, Self::Transform | Self::Rotate)
    }

    fn label_localized(self, japanese: bool) -> &'static str {
        match (self, japanese) {
            (Self::Bucket, true) => "バケツ塗り",
            (Self::Transform, true) => "移動・拡大縮小",
            (Self::Rotate, true) => "回転",
            _ => self.label(),
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    layers: Vec<Layer>,
    options: JobOptions,
    selected: Option<Selection>,
}

struct Imported {
    name: String,
    artwork: Artwork,
}

struct ImportReport {
    items: Vec<Imported>,
    notices: Vec<String>,
}

struct PlanReport {
    revision: u64,
    removed: (usize, f64),
    result: Result<(Vec<Stroke>, Result<Output, String>), String>,
}

struct Studio {
    #[cfg(test)]
    canvas_rect: Rect,
    layers: Vec<Layer>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    text_dialog: Option<TextDialog>,
    selected: Option<Selection>,
    route_targets: Vec<Option<Selection>>,
    object_context: Option<ObjectContext>,
    import_options: ImportOptions,
    profile: Profile,
    options: JobOptions,
    workspace: Workspace,
    pending_export: Option<bool>,
    tools: Tool,
    bucket_defaults: penplot::PatternSettings,
    grayscale_preview: bool,
    frame: Camera,
    isometric: bool,
    z_exaggeration: f64,
    draft: Vec<Point>,
    notices: Vec<String>,
    profile_notices: Vec<String>,
    status: String,
    error: Option<String>,
    planned: Vec<Stroke>,
    output: Option<Arc<Output>>,
    placement_drag: Option<PlacementDrag>,
    placement_bounds_cache: Option<(u64, usize, Option<[Point; 4]>)>,
    show_travel: bool,
    play: bool,
    shown: usize,
    last_tick: f64,
    import_task: Option<mpsc::Receiver<ImportReport>>,
    raster_import: RasterImport,
    raster_removed: (usize, f64),
    import_dialog: Option<mpsc::Receiver<Option<Vec<PathBuf>>>>,
    last_export_dir: Option<PathBuf>,
    project_file: Option<PathBuf>,
    project_dirty: bool,
    pending_open: Option<(PathBuf, Project)>,
    confirm_quit: bool,
    allow_quit: bool,
    preview_dirty: bool,
    show_gcode: bool,
    export_inspection: Option<ExportInspection>,
    air_first: bool,
    revision: u64,
    plan_task: Option<mpsc::Receiver<PlanReport>>,
    help_open: bool,
    japanese: bool,
}

impl Default for Studio {
    fn default() -> Self {
        Self {
            #[cfg(test)]
            canvas_rect: Rect::NOTHING,
            layers: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            selected: None,
            text_dialog: None,
            route_targets: Vec::new(),
            object_context: None,
            import_options: ImportOptions::default(),
            profile: Profile::default(),
            options: JobOptions::default(),
            workspace: Workspace::Edit,
            pending_export: None,
            tools: Tool::Pan,
            bucket_defaults: penplot::PatternSettings::default(),
            grayscale_preview: false,
            frame: Camera::default(),
            isometric: false,
            z_exaggeration: 3.0,
            draft: Vec::new(),
            notices: Vec::new(),
            profile_notices: Vec::new(),
            status: "Import artwork or draw on the bed to begin.".into(),
            error: None,
            planned: Vec::new(),
            output: None,
            placement_drag: None,
            placement_bounds_cache: None,
            show_travel: false,
            play: false,
            shown: 0,
            last_tick: 0.0,
            import_task: None,
            raster_import: RasterImport::default(),
            raster_removed: (0, 0.0),
            import_dialog: None,
            last_export_dir: None,
            project_file: None,
            project_dirty: false,
            pending_open: None,
            confirm_quit: false,
            allow_quit: false,
            preview_dirty: true,
            show_gcode: false,
            export_inspection: None,
            air_first: true,
            revision: 0,
            plan_task: None,
            help_open: false,
            japanese: false,
        }
    }
}

pub fn run() -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([760.0, 560.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            let mut visuals = egui::Visuals::dark();
            visuals.panel_fill = Color32::from_rgb(26, 32, 40);
            visuals.window_fill = Color32::from_rgb(30, 37, 45);
            visuals.extreme_bg_color = Color32::from_rgb(15, 20, 26);
            visuals.faint_bg_color = Color32::from_rgb(34, 44, 53);
            visuals.selection.bg_fill = Color32::from_rgb(47, 104, 98);
            visuals.selection.stroke = Ink::new(1.0, ACCENT);
            cc.egui_ctx.set_visuals(visuals);
            cc.egui_ctx.style_mut(|style| {
                style.spacing.item_spacing = egui::vec2(8.0, 7.0);
                style.spacing.button_padding = egui::vec2(8.0, 4.0);
            });
            let japanese = install_system_font(&cc.egui_ctx);
            let mut app = Studio {
                japanese,
                ..Studio::default()
            };
            let paths: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
            if paths.len() == 1
                && paths[0]
                    .extension()
                    .is_some_and(|s| s.eq_ignore_ascii_case("penplot"))
            {
                match penplot::read_limited(&paths[0]).and_then(|bytes| Project::from_json(&bytes))
                {
                    Ok(project) => app.apply_project(paths[0].clone(), project),
                    Err(error) => app.status = error,
                }
            } else {
                app.begin_import(&cc.egui_ctx, paths);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| format!("Could not open the desktop window: {error}"))
}

impl eframe::App for Studio {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_ui(ctx);
    }
}

impl Studio {
    fn update_ui(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.viewport().close_requested())
            && self.project_dirty
            && !self.allow_quit
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_quit = true;
        }
        self.take_file_selection(ctx);
        self.poll_import();
        if ctx.input(|input| input.key_pressed(Key::Escape) || !input.focused) {
            self.draft.clear();
            self.placement_drag = None;
        }
        if self.placement_drag.is_none() {
            self.shortcuts(ctx);
        }
        let dropped = ctx.input(|input| input.raw.dropped_files.clone());
        if !dropped.is_empty() {
            let paths = dropped.into_iter().filter_map(|file| file.path).collect();
            self.begin_import(ctx, paths);
        }
        self.poll_plan(ctx);
        self.step_player(ctx);
        self.finish_pending_export();
        egui::TopBottomPanel::top("header").show(ctx, |ui| self.header(ui, ctx));
        egui::TopBottomPanel::bottom("timeline").show(ctx, |ui| self.timeline(ui));
        self.workspace_ui(ctx);
        self.help_window(ctx);
        self.confirm_project_actions(ctx);
        let text_was_open = self.text_dialog.is_some();
        self.show_text_dialog(ctx);
        if !text_was_open {
            self.object_shortcuts(ctx);
        }
        if self.preview_dirty && self.plan_task.is_none() {
            ctx.request_repaint();
        }
    }
}

impl Studio {
    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui| {
            ui.heading(RichText::new(APP_NAME).strong().color(ACCENT));
            ui.label(
                RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                    .small()
                    .weak(),
            );
            ui.separator();
            let busy = self.text_dialog.is_some()
                || self.import_task.is_some()
                || self.import_dialog.is_some()
                || self.raster_import.is_busy();
            if ui
                .add_enabled(!busy, egui::Button::new("Import…"))
                .clicked()
            {
                self.select_files(ctx);
            }
            if ui
                .add_enabled(
                    !busy && self.placement_drag.is_none(),
                    egui::Button::new("Text…"),
                )
                .clicked()
            {
                self.open_text_dialog(None);
            }
            ui.add_enabled_ui(!busy, |ui| {
                ui.menu_button("Project", |ui| {
                    if ui.button("Open project…").clicked() {
                        ui.close();
                        self.open_project_dialog();
                    }
                    if ui.button("Save project…").clicked() {
                        ui.close();
                        self.save_project_dialog();
                    }
                });
            });
            if self.project_dirty {
                ui.label(RichText::new("Unsaved").small().color(WARN));
            }
            if busy {
                ui.spinner();
            }
            if ui
                .add_enabled(!busy && !self.undo.is_empty(), egui::Button::new("Undo"))
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(!busy && !self.redo.is_empty(), egui::Button::new("Redo"))
                .clicked()
            {
                self.redo();
            }
            if ui
                .button(if self.japanese { "使い方" } else { "Help" })
                .clicked()
            {
                self.help_open = true;
            }
        });
        ui.horizontal_wrapped(|ui| {
            let enabled = self.can_export();
            let title = if self.air_first {
                "Export air run…"
            } else {
                "Export G-code…"
            };
            if ui
                .button(title)
                .on_hover_text("Export the current document from any workspace.")
                .clicked()
            {
                self.request_export(self.air_first);
            }
            if ui.add_enabled(enabled, egui::Button::new("Export colors…"))
                .on_hover_text("Write a NEW folder with one G-code file per pen and a pen-change guide. Keeps paper XY fixed.")
                .clicked() { self.save_colors(self.air_first); }
            ui.checkbox(&mut self.air_first, "Air run first")
                .on_hover_text("Air run follows the same XY route but never lowers the pen.");
            if ui
                .add_enabled(enabled, egui::Button::new("Inspect G-code"))
                .clicked()
            {
                self.inspect_gcode(self.air_first);
            }
        });
        if let Some(reason) = self.profile.export_blocker() {
            ui.colored_label(WARN, reason);
        }
        ui.separator();
        self.workspace_tabs(ui);
        ui.add_space(5.0);
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.raster_import.is_busy() || self.text_dialog.is_some() {
            return;
        }
        let (open, save, shift) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(Key::O),
                i.modifiers.command && i.key_pressed(Key::S),
                i.modifiers.shift,
            )
        });
        if open {
            if shift {
                self.open_project_dialog();
            } else {
                self.select_files(ctx);
            }
        }
        if save {
            self.save_project_dialog();
        }
    }

    fn select_files(&mut self, ctx: &egui::Context) {
        if self.import_task.is_some()
            || self.import_dialog.is_some()
            || self.raster_import.is_busy()
        {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        // Windows and Linux dialogs can be driven by a worker. Cocoa needs the main thread.
        #[cfg(not(target_os = "macos"))]
        std::thread::spawn(move || {
            let _ = sender.send(artwork_dialog());
            ctx.request_repaint();
        });
        #[cfg(target_os = "macos")]
        {
            let _ = sender.send(artwork_dialog());
            ctx.request_repaint();
        }
        self.import_dialog = Some(receiver);
    }

    fn begin_import(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>) {
        if self.text_dialog.is_some() {
            return;
        }
        if paths.is_empty() {
            return;
        }
        self.switch_workspace(Workspace::Import);
        if self.import_task.is_some() || self.raster_import.is_busy() {
            self.status = "An import is already in progress; try again when it finishes.".into();
            return;
        }
        let available = MAX_LAYER_COUNT.saturating_sub(self.layers.len());
        if paths.len() > available {
            self.notices = vec![format!(
                "Layer limit: {} of {} selected files can be imported.",
                available,
                paths.len()
            )];
        } else {
            self.notices.clear();
        }
        let (rasters, paths): (Vec<_>, Vec<_>) = paths
            .into_iter()
            .take(available)
            .partition(|path| penplot::is_raster_filename(&path.to_string_lossy()));
        self.raster_import.extend(rasters);
        if paths.is_empty() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        let options = self.import_options.clone();
        let (mut paths_used, mut points_used) = self.geometry_size();
        self.status = "Reading and tracing artwork…".into();
        std::thread::spawn(move || {
            let mut notices = Vec::new();
            let items = paths
                .iter()
                .filter_map(|path| match import_one(path, &options) {
                    Ok(item) => {
                        let strokes = item.artwork.strokes.len();
                        let vertices = item.artwork.strokes.iter().map(|s| s.points.len()).sum::<usize>();
                        if paths_used + strokes > penplot::MAX_STROKES || points_used + vertices > penplot::MAX_POINTS {
                            notices.push(format!("{}: workspace geometry limit reached; remove other layers before retrying.", item.name));
                            None
                        } else {
                            paths_used += strokes;
                            points_used += vertices;
                            Some(item)
                        }
                    }
                    Err(message) => {
                        notices.push(message);
                        None
                    }
                })
                .collect();
            let _ = sender.send(ImportReport { items, notices });
            ctx.request_repaint();
        });
        self.import_task = Some(receiver);
    }

    fn add_raster_layer(&mut self, item: Imported, raster: penplot::RasterSettings) {
        let (paths, points) = self.geometry_size();
        if self.layers.len() >= MAX_LAYER_COUNT
            || paths + item.artwork.strokes.len() > penplot::MAX_STROKES
            || points
                + item
                    .artwork
                    .strokes
                    .iter()
                    .map(|s| s.points.len())
                    .sum::<usize>()
                > penplot::MAX_POINTS
        {
            self.notices
                .push(format!("{}: workspace geometry limit reached.", item.name));
            return;
        }
        let mut groups = std::collections::BTreeMap::<String, Vec<Stroke>>::new();
        for stroke in item.artwork.strokes {
            groups.entry(stroke.layer.clone()).or_default().push(stroke);
        }
        if self.layers.len() + groups.len() > MAX_LAYER_COUNT {
            self.notices.push("Color import exceeds 128 layers.".into());
            return;
        }
        self.record_layers();
        self.notices.extend(item.artwork.warnings);
        for (color, strokes) in groups {
            self.layers.push(Layer {
                name: if penplot::ink_rgb(&color).is_some() {
                    format!("{} {color}", item.name)
                } else {
                    item.name.clone()
                },
                strokes: strokes.into(),
                visible: true,
                raster: Some(raster.clone()),
                text: None,
            });
        }
        self.selected = Some(Selection::Layer(self.layers.len() - 1));
        self.status = "Image layer imported. Check placement before export.".into();
        self.invalidate();
    }

    fn poll_import(&mut self) {
        if let Some(task) = &self.import_task {
            match task.try_recv() {
                Ok(result) => {
                    self.import_task = None;
                    self.notices.extend(result.notices);
                    if result.items.is_empty() {
                        self.status = "No artwork imported. See the notes in Import.".into();
                        return;
                    }
                    self.record_layers();
                    let count = result.items.len();
                    for imported in result.items {
                        self.notices.extend(imported.artwork.warnings);
                        self.layers.push(Layer {
                            name: imported.name,
                            strokes: imported.artwork.strokes.into(),
                            visible: true,
                            raster: None,
                            text: None,
                        });
                    }
                    self.selected = Some(Selection::Layer(self.layers.len() - 1));
                    self.status =
                        format!("{count} layer(s) imported. Check placement before export.");
                    self.invalidate();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.import_task = None;
                    self.status = "The artwork importer stopped before returning a result.".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }

    fn take_file_selection(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &self.import_dialog else {
            return;
        };
        match dialog.try_recv() {
            Ok(selection) => {
                self.import_dialog = None;
                if let Some(paths) = selection {
                    self.begin_import(ctx, paths);
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => self.import_dialog = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            layers: self.layers.clone(),
            options: self.options.clone(),
            selected: self.selected,
        }
    }

    fn geometry_size(&self) -> (usize, usize) {
        self.layers
            .iter()
            .flat_map(|layer| layer.strokes.iter())
            .fold((0, 0), |(paths, vertices), stroke| {
                (paths + 1, vertices + stroke.points.len())
            })
    }

    fn record_layers(&mut self) {
        if self.undo.len() >= MAX_UNDO_STATES {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.redo.clear();
    }

    fn undo(&mut self) {
        if self.import_task.is_some() {
            return;
        }
        if let Some(previous) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.layers = previous.layers;
            self.options = previous.options;
            self.selected = previous.selected;
            self.invalidate();
        }
    }

    fn redo(&mut self) {
        if self.import_task.is_some() {
            return;
        }
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.layers = next.layers;
            self.options = next.options;
            self.selected = next.selected;
            self.invalidate();
        }
    }

    fn invalidate(&mut self) {
        self.placement_drag = None;
        self.preview_dirty = true;
        self.project_dirty = true;
        self.revision = self.revision.wrapping_add(1);
        self.output = None;
        self.planned.clear();
        self.route_targets.clear();
        self.object_context = None;
        self.shown = 0;
        self.error = None;
        self.raster_removed = (0, 0.0);
        self.play = false;
    }

    fn step_player(&mut self, ctx: &egui::Context) {
        if !self.play {
            return;
        }
        let now = ctx.input(|input| input.time);
        if now - self.last_tick >= 0.085 {
            self.shown = (self.shown + 1).min(self.planned.len());
            self.last_tick = now;
            self.play = self.shown < self.planned.len();
        }
        if self.play {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            let count = self.planned.len();
            if ui.add_enabled(count != 0, egui::Button::new(if self.play { "Pause" } else { "Play" })).clicked() {
                if self.shown >= count {
                    self.shown = 0;
                }
                self.play = !self.play;
                ui.ctx().request_repaint();
            }
            ui.add_enabled(count != 0, egui::Slider::new(&mut self.shown, 0..=count).text("strokes"));
            ui.checkbox(&mut self.show_travel, "Show travel")
                .on_hover_text("Pen-up moves between strokes. The initial carriage position is not simulated.");
            ui.separator();
            if let Some(output) = &self.output {
                ui.label(format!(
                    "{:.1} mm draw / {:.1} mm travel  ·  Est. plot ~{}  /  air ~{}",
                    output.draw_mm,
                    output.travel_mm,
                    format_duration(output.seconds),
                    format_duration(output.air_seconds),
                )).on_hover_text(
                    "Ideal motion + dwell only. Excludes acceleration, firmware overhead and the initial XY/Z moves.",
                );
            } else if self.plan_task.is_some() {
                ui.spinner();
                ui.label(RichText::new("Planning current route…").weak());
            } else {
                ui.label(RichText::new("No exportable route").weak());
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(WARN, error);
        } else {
            ui.label(RichText::new(&self.status).weak());
        }
        ui.add_space(4.0);
    }

    fn open_project_dialog(&mut self) {
        if self.import_dialog.is_some()
            || self.import_task.is_some()
            || self.raster_import.is_busy()
            || self.pending_open.is_some()
        {
            return;
        }
        let mut dialog = rfd::FileDialog::new().add_filter("Penplot project", &["penplot"]);
        if let Some(directory) = self.project_file.as_deref().and_then(Path::parent) {
            dialog = dialog.set_directory(directory);
        }
        if let Some(path) = dialog.pick_file() {
            match penplot::read_limited(&path).and_then(|bytes| Project::from_json(&bytes)) {
                Ok(project) if self.project_dirty => self.pending_open = Some((path, project)),
                Ok(project) => self.apply_project(path, project),
                Err(error) => self.status = format!("Unable to open project: {error}"),
            }
        }
    }

    fn save_project_dialog(&mut self) {
        if self.import_dialog.is_some()
            || self.import_task.is_some()
            || self.raster_import.is_busy()
        {
            return;
        }
        let mut dialog = rfd::FileDialog::new().add_filter("Penplot project", &["penplot"]);
        if let Some(existing) = self.project_file.as_deref() {
            if let Some(directory) = existing.parent() {
                dialog = dialog.set_directory(directory);
            }
            dialog =
                dialog.set_file_name(existing.file_name().unwrap_or_default().to_string_lossy());
        } else {
            dialog = dialog.set_file_name("drawing.penplot");
        }
        if let Some(mut path) = dialog.save_file() {
            let overwrite_confirmed = path.extension().is_some();
            if !overwrite_confirmed {
                path.set_extension("penplot");
            }
            let result = self
                .as_project()
                .to_json()
                .and_then(|bytes| penplot::save_file(&path, &bytes, overwrite_confirmed));
            self.status = match result {
                Ok(()) => {
                    self.project_dirty = false;
                    let status = format!("Saved project to {}.", path.display());
                    self.project_file = Some(path);
                    status
                }
                Err(error) => format!("Unable to save project: {error}"),
            };
        }
    }

    fn as_project(&self) -> Project {
        Project {
            layers: self
                .layers
                .iter()
                .map(|layer| ProjectLayer {
                    name: layer.name.clone(),
                    visible: layer.visible,
                    strokes: layer.strokes.to_vec(),
                    raster: layer.raster.clone(),
                    text: layer.text.clone(),
                })
                .collect(),
            options: self.options.clone(),
            profile: self.profile.clone(),
            import_options: self.import_options.clone(),
        }
    }

    fn apply_project(&mut self, path: PathBuf, project: Project) {
        let options = &project.options;
        self.layers = project
            .layers
            .into_iter()
            .map(|layer| {
                let raster = layer.raster_settings(options);
                Layer {
                    name: layer.name,
                    visible: layer.visible,
                    strokes: layer.strokes.into(),
                    raster,
                    text: layer.text,
                }
            })
            .collect();
        self.grayscale_preview = self
            .layers
            .iter()
            .any(|l| l.raster.as_ref().is_some_and(|r| r.pattern.is_some()));
        self.options = project.options;
        self.profile = project.profile;
        self.import_options = project.import_options;
        self.frame = Camera::default();
        self.isometric = false;
        self.tools = Tool::Pan;
        self.draft.clear();
        self.notices.clear();
        self.profile_notices.clear();
        self.undo.clear();
        self.redo.clear();
        self.selected = None;
        self.text_dialog = None;
        self.placement_drag = None;
        self.air_first = true;
        self.status = format!("Opened {}.", path.display());
        self.pending_export = None;
        self.project_file = Some(path);
        self.invalidate();
        self.project_dirty = false;
    }

    fn confirm_project_actions(&mut self, ctx: &egui::Context) {
        if let Some((path, _)) = &self.pending_open {
            let mut discard = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("project_replace")).show(ctx, |ui| {
                ui.heading("Replace this workspace?");
                ui.label(format!(
                    "Opening {} will discard unsaved changes.",
                    path.display()
                ));
                ui.horizontal(|ui| {
                    cancel = ui.button("Keep working").clicked();
                    discard = ui.button("Discard and open").clicked();
                });
            });
            if discard {
                if let Some((path, project)) = self.pending_open.take() {
                    self.apply_project(path, project);
                }
            } else if cancel || modal.should_close() {
                self.pending_open = None;
            }
        }
        if self.confirm_quit {
            let mut discard = false;
            let mut cancel = false;
            let modal = egui::Modal::new(egui::Id::new("project_quit")).show(ctx, |ui| {
                ui.heading("Close without saving?");
                ui.label("Save the current workspace first if you want to keep it.");
                ui.horizontal(|ui| {
                    cancel = ui.button("Keep working").clicked();
                    discard = ui.button("Discard and close").clicked();
                });
            });
            if discard {
                self.confirm_quit = false;
                self.allow_quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if cancel || modal.should_close() {
                self.confirm_quit = false;
            }
        }
    }

    fn help_window(&mut self, ctx: &egui::Context) {
        egui::Window::new(if self.japanese { "使い方 / Quick start" } else { "Quick start" })
            .open(&mut self.help_open)
            .resizable(false)
            .max_width(520.0)
            .show(ctx, |ui| {
                if self.japanese {
                    ui.label("1. Import で図面を読み込みます。Fit to usable bed で描画範囲に収めます。");
                    ui.label("2. Machine で原点から実測した描画 Z・退避 Z、オフセット、可動範囲を設定します。");
                    ui.label("3. 機械側でペンを逃がして原点復帰し、まず Air run（ペン上げのみ）で干渉を確認します。");
                    ui.label("4. 問題なければ Air run first を外して描画用 G-code を出力します。");
                } else {
                    ui.label("1. Import your drawing and use Fit to usable bed if needed.");
                    ui.label("2. Enter measured Draw Z, Clearance Z, offsets and travel limits in Machine before running on hardware.");
                    ui.label("3. Home manually with clearance; export and verify a pen-up air run.");
                    ui.label("4. Uncheck Air run first only when ready to export the drawing file.");
                }
                ui.separator();
                ui.small("This app exports files only. It does not connect to or control the printer.");
            });
    }

    fn export_commands(&self, air_run: bool) -> Result<Arc<Output>, String> {
        if !self.can_export() {
            return Err(self.export_unavailable_reason());
        }
        if air_run {
            penplot::generate_with_travel(
                &self.planned,
                &self.profile,
                true,
                self.options.smooth_pen_up,
            )
            .map(Arc::new)
        } else {
            self.output
                .clone()
                .ok_or_else(|| "No valid, current G-code route.".into())
        }
    }

    fn open_gcode_snapshot(&mut self, path: Option<PathBuf>, air_run: bool, output: Arc<Output>) {
        let inspection = ExportInspection::new(path, self.revision, air_run, output, &self.profile);
        self.export_inspection = Some(inspection);
        self.show_gcode = true;
        self.switch_workspace(Workspace::Preview);
    }

    fn inspect_gcode(&mut self, air_run: bool) {
        match self.export_commands(air_run) {
            Ok(output) => self.open_gcode_snapshot(None, air_run, output),
            Err(error) => self.status = error,
        }
    }

    fn save_colors(&mut self, air: bool) {
        if !self.can_export() {
            self.status = self.export_unavailable_reason();
            return;
        }
        let jobs = match penplot::color_jobs(
            &self.planned,
            &self.profile,
            air,
            self.options.smooth_pen_up,
        ) {
            Ok(jobs) => jobs,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let dialog = rfd::FileDialog::new()
            .set_title("Choose a NEW color-pass folder name")
            .set_file_name(if air { "Color-air-run" } else { "Color-plot" });
        if let Some(path) = dialog.save_file() {
            match penplot::save_color_jobs(&path, &jobs) {
                Ok(()) => {
                    self.last_export_dir = path.parent().map(Path::to_path_buf);
                    self.status = format!(
                        "Saved {} pen passes to {}. Follow PENS.txt; keep paper and XY origin fixed between pens.",
                        jobs.len(),
                        path.display()
                    );
                }
                Err(e) => self.status = e,
            }
        }
    }

    fn save_gcode(&mut self, air_run: bool) {
        let output = match self.export_commands(air_run) {
            Ok(output) => output,
            Err(error) => {
                self.status = error;
                return;
            }
        };
        let aries = self.profile.machine_preset == Some(MachinePreset::VoxelabAries);
        let (name, extension) = match (aries, air_run) {
            (true, true) => ("airrun.gcode", "gcode"),
            (true, false) => ("plot.gcode", "gcode"),
            (false, true) => ("AIRRUN.GCODE", "GCODE"),
            (false, false) => ("PLOT.GCODE", "GCODE"),
        };
        let mut dialog = rfd::FileDialog::new()
            .add_filter("G-code", &["GCODE", "gcode"])
            .set_file_name(name);
        if let Some(directory) = self.last_export_dir.as_deref() {
            dialog = dialog.set_directory(directory);
        }
        if let Some(mut path) = dialog.save_file() {
            let overwrite_confirmed_by_dialog = path.extension().is_some();
            if !overwrite_confirmed_by_dialog {
                path.set_extension(extension);
            }
            match penplot::save_file(
                &path,
                output.gcode.as_bytes(),
                overwrite_confirmed_by_dialog,
            ) {
                Ok(()) => {
                    self.last_export_dir = path.parent().map(Path::to_path_buf);
                    self.status = if air_run {
                        format!(
                            "Exported {} with the pen up only. Verify clearance at the machine.",
                            path.display()
                        )
                    } else {
                        format!(
                            "Exported {}. Run an air plot before making contact.",
                            path.display()
                        )
                    };
                    self.open_gcode_snapshot(Some(path), air_run, output);
                }
                Err(error) => self.status = format!("Unable to save {}: {error}", path.display()),
            }
        }
    }
}

enum LayerAction {
    Visibility(bool),
    Move(usize),
}

fn section(ui: &mut egui::Ui, label: &str) {
    ui.label(RichText::new(label).small().strong().color(ACCENT));
}

fn install_system_font(ctx: &egui::Context) -> bool {
    #[cfg(target_os = "windows")]
    let paths = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .map(|dir| dir.join("Fonts"))
        .filter(|dir| dir.is_absolute())
        .into_iter()
        .flat_map(|dir| {
            [
                dir.join("meiryo.ttc"),
                dir.join("YuGothM.ttc"),
                dir.join("msgothic.ttc"),
            ]
        })
        .collect::<Vec<_>>();
    #[cfg(target_os = "macos")]
    let paths = [
        "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        "/System/Library/Fonts/AppleSDGothicNeo.ttc",
    ]
    .map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let paths = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
    ]
    .map(PathBuf::from);
    for path in paths {
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let mut data = Vec::new();
        if file
            .take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut data)
            .is_err()
            || data.len() > 32 * 1024 * 1024
            || !matches!(data.get(..4), Some(b"ttcf" | b"OTTO" | b"\x00\x01\x00\x00"))
        {
            continue;
        }
        let mut fonts = egui::FontDefinitions::default();
        let name = "system-cjk".to_owned();
        fonts
            .font_data
            .insert(name.clone(), Arc::new(egui::FontData::from_owned(data)));
        for names in fonts.families.values_mut() {
            names.push(name.clone());
        }
        ctx.set_fonts(fonts);
        return true;
    }
    false
}

fn artwork_dialog() -> Option<Vec<PathBuf>> {
    rfd::FileDialog::new()
        .add_filter("Plot artwork", &["svg", "dxf", "png", "jpg", "jpeg"])
        .pick_files()
}

fn import_one(path: &Path, options: &ImportOptions) -> Result<Imported, String> {
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned();
    let bytes = penplot::read_limited(path).map_err(|error| format!("{name}: {error}"))?;
    let mut artwork = penplot::import_bytes(&name, &bytes, options)
        .map_err(|error| format!("{name}: {error}"))?;
    for warning in &mut artwork.warnings {
        *warning = format!("{name}: {warning}");
    }
    if artwork.strokes.is_empty() {
        return Err(format!("{name}: no plottable strokes were imported."));
    }
    Ok(Imported { name, artwork })
}

struct Camera {
    zoom: f32,
    shift: Vec2,
}

impl Studio {
    fn fit_artwork(&self, options: &mut JobOptions) -> Result<(), String> {
        self.combined()
            .and_then(|strokes| penplot::fit(&strokes, &self.profile, options, 5.0))
    }

    fn fit_and_reset_view(&mut self) {
        let mut draft = self.options.clone();
        match self.fit_artwork(&mut draft) {
            Ok(()) => {
                self.record_layers();
                self.options = draft;
                self.frame = Camera::default();
                self.status = "Artwork fitted to the usable bed.".into();
                self.invalidate();
            }
            Err(error) => self.status = error,
        }
    }

    fn placement_panel(&mut self, ui: &mut egui::Ui) {
        section(ui, "SELECTED LAYER · MM");
        if self.placement_drag.is_some() {
            ui.small("Finish or cancel the canvas drag to edit numeric placement.");
        } else if let (Some(layer), Some(bounds)) =
            (self.placement_layer(), self.placement_bounds())
        {
            let low = bounds[0];
            let (old_width, old_height) = (bounds[2].x - low.x, bounds[2].y - low.y);
            let (mut width, mut height, mut x, mut y) = (old_width, old_height, low.x, low.y);
            let width_changed = ui
                .add_enabled_ui(old_width > 1e-8, |ui| number(ui, "Width", &mut width, 0.1))
                .inner;
            let height_changed = ui
                .add_enabled_ui(old_height > 1e-8, |ui| {
                    number(ui, "Height", &mut height, 0.1)
                })
                .inner;
            let moved = number(ui, "Left X", &mut x, 0.1) | number(ui, "Bottom Y", &mut y, 0.1);
            let motion = if width_changed {
                Some(PlacementMotion::Scale(low, width / old_width))
            } else if height_changed {
                Some(PlacementMotion::Scale(low, height / old_height))
            } else if moved {
                Some(PlacementMotion::Move(Point::new(x - low.x, y - low.y)))
            } else {
                None
            };
            if let Some(motion) = motion {
                self.finish_layer_placement(layer, motion, ui.ctx());
            }
            ui.small("Width and height keep the aspect ratio.");
        } else {
            ui.small("Select a visible layer or click its path to transform it.");
        }
        ui.separator();
        section(ui, "WHOLE JOB");
        if ui
            .add_enabled(
                self.placement_drag.is_none() && self.layers.iter().any(|layer| layer.visible),
                egui::Button::new("Fit to usable bed"),
            )
            .on_hover_text("Fit the combined visible artwork with a 5 mm border.")
            .clicked()
        {
            self.fit_and_reset_view();
            ui.ctx().request_repaint();
        }
    }

    fn import_panel(&mut self, ui: &mut egui::Ui) {
        let before = self.import_options.clone();
        section(ui, "IMPORT SETTINGS");
        ui.small("Applies to files imported next.");
        number(
            ui,
            "Curve tolerance (mm)",
            &mut self.import_options.tolerance,
            0.01,
        );
        number(ui, "Raster DPI", &mut self.import_options.dpi, 1.0);
        ui.horizontal(|ui| {
            ui.label("Raster threshold");
            ui.add(egui::DragValue::new(&mut self.import_options.threshold).speed(1));
        });
        ui.checkbox(
            &mut self.import_options.invert,
            "Trace light instead of dark",
        );
        if before != self.import_options {
            self.project_dirty = true;
        }
    }

    fn machine_panel(&mut self, ui: &mut egui::Ui) {
        section(ui, "MACHINE PROFILE");
        ui.label(RichText::new(&self.profile.name).strong());
        ui.horizontal_wrapped(|ui| {
            if ui.button("Load JSON…").clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Profile", &["json"])
                    .pick_file()
            {
                self.load_profile(&path);
            }
            if ui.button("Save JSON…").clicked() {
                self.save_profile();
            }
        });
        let mut picked = None;
        egui::ComboBox::from_id_salt("machine_preset")
            .width(ui.available_width().min(240.0))
            .selected_text("Load a machine preset…")
            .show_ui(ui, |ui| {
                if ui.button("EasyThreed K9").clicked() {
                    picked = Some(None);
                    ui.close();
                }
                for (preset, suffix) in [
                    (MachinePreset::BambuP1s, "layout only"),
                    (MachinePreset::VoxelabAries, "experimental"),
                ] {
                    if ui
                        .button(format!("{} ({suffix})", preset.name()))
                        .on_hover_text(preset.notice())
                        .clicked()
                    {
                        picked = Some(Some(preset));
                        ui.close();
                    }
                }
            });
        if let Some(preset) = picked {
            self.apply_machine_preset(preset);
            ui.ctx().request_repaint();
        }
        if let Some(preset) = self.profile.machine_preset {
            ui.colored_label(WARN, preset.notice());
        }
        ui.add_space(4.0);
        let profile = &mut self.profile;
        let mut changed = ui
            .add(egui::TextEdit::singleline(&mut profile.name).desired_width(ui.available_width()))
            .changed();
        changed |= number(ui, "Bed X (mm)", &mut profile.bed_x, 0.5);
        changed |= number(ui, "Bed Y (mm)", &mut profile.bed_y, 0.5);
        changed |= number(ui, "Maximum Z (mm)", &mut profile.max_z, 0.1);
        egui::ComboBox::from_id_salt("origin")
            .width(ui.available_width().min(200.0))
            .selected_text(match profile.origin {
                Origin::LeftBottom => "Front left",
                Origin::Center => "Center",
            })
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(&mut profile.origin, Origin::LeftBottom, "Front left")
                    .changed();
                changed |= ui
                    .selectable_value(&mut profile.origin, Origin::Center, "Center")
                    .changed();
            });
        changed |= number(ui, "Command offset X (mm)", &mut profile.offset_x, 0.1);
        changed |= number(ui, "Command offset Y (mm)", &mut profile.offset_y, 0.1);
        ui.separator();
        section(ui, "PEN HEIGHTS • ABSOLUTE");
        ui.small(if self.japanese {
            "描画と退避は、どちらも機械の原点 Z=0 からの絶対位置です。差分ではありません。共通補正は両方に加算します。"
        } else {
            "Draw and clearance are absolute targets from the same active Z=0 after homing, not a lift distance. The common offset adds to both."
        });
        changed |= number(ui, "Draw Z (mm)", &mut profile.pen_down_z, 0.1);
        if profile.k9_start_gcode.is_some() {
            let mut relative = profile.k9_lift_mm.is_some();
            if ui
                .checkbox(
                    &mut relative,
                    if self.japanese {
                        "描画 Z からの垂直上昇量を指定"
                    } else {
                        "Set vertical lift above Draw Z"
                    },
                )
                .changed()
            {
                profile.k9_lift_mm = relative.then_some(2.0);
                changed = true;
            }
        }
        if let Some(lift) = &mut profile.k9_lift_mm {
            changed |= number(ui, "Vertical lift (mm)", lift, 0.1);
            ui.small(format!(
                "Clearance Z = {:.3} mm (absolute)",
                profile.clearance_z()
            ));
        } else {
            changed |= number(ui, "Clearance Z (mm)", &mut profile.pen_up_z, 0.1);
        }
        changed |= number(ui, "Common Z offset (mm)", &mut profile.offset_z, 0.1);
        ui.separator();
        section(ui, "PEN & SPEED");
        changed |= number(ui, "Pen up (mm/s)", &mut profile.pen_up_speed, 0.1);
        changed |= number(ui, "Pen down (mm/s)", &mut profile.pen_down_speed, 0.1);
        changed |= number(ui, "Draw (mm/s)", &mut profile.draw_speed, 0.5);
        changed |= number(ui, "Travel (mm/s)", &mut profile.travel_speed, 0.5);
        if profile.machine_preset == Some(MachinePreset::VoxelabAries) {
            ui.small("Aries: G4 dwell is not emitted.");
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.label("After pen down (ms)");
                changed |= ui
                    .add(egui::DragValue::new(&mut profile.touchdown_delay).speed(5))
                    .changed();
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Before pen up (ms)");
                changed |= ui
                    .add(egui::DragValue::new(&mut profile.liftup_delay).speed(5))
                    .changed();
            });
        }
        if let Some(start) = &mut profile.k9_start_gcode {
            ui.separator();
            section(
                ui,
                if self.japanese {
                    "開始 G-code"
                } else {
                    "START G-CODE"
                },
            );
            changed |= ui
                .checkbox(
                    &mut start.enabled,
                    if self.japanese {
                        "K9 の実験的な速度・加速度設定を出力"
                    } else {
                        "Emit experimental K9 planner settings"
                    },
                )
                .changed();
            ui.small(if self.japanese {
                "M203 X/Y、M201 X/Y、M204 P/T のみ。空走にも適用します。移動の F 値や EEPROM は変更しません。"
            } else {
                "Only M203 X/Y, M201 X/Y and M204 P/T. Also applies to air runs. Does not change commanded F or write EEPROM."
            });
            if start.enabled {
                ui.colored_label(WARN, if self.japanese {
                    "K9 実機・100 mm/s 未検証。最初はペンを外し、低速で監視してください。"
                } else {
                    "Not hardware-verified at 100 mm/s. First test slowly with the pen removed and supervise."
                });
            }
            changed |= ui
                .add(
                    egui::TextEdit::multiline(&mut start.code)
                        .code_editor()
                        .desired_rows(3)
                        .desired_width(ui.available_width())
                        .char_limit(1024),
                )
                .changed();
            if ui
                .button(if self.japanese {
                    "3 行の初期値に戻す（無効）"
                } else {
                    "Reset three lines and turn off"
                })
                .clicked()
            {
                *start = penplot::K9StartGcode::default();
                changed = true;
            }
        }
        if changed {
            self.pending_export = None;
            self.invalidate();
            ui.ctx().request_repaint();
        }
        if !self.profile_notices.is_empty() {
            ui.separator();
            section(ui, "PROFILE NOTES");
            for notice in &self.profile_notices {
                ui.colored_label(WARN, notice);
            }
        }
    }

    fn apply_machine_preset(&mut self, preset: Option<MachinePreset>) {
        self.pending_export = None;
        self.profile = preset.map(Profile::for_machine).unwrap_or_else(Profile::k9);
        self.profile_notices.clear();
        if let Some(preset) = preset {
            self.air_first = true;
            self.status = preset.notice().into();
        } else {
            self.air_first = true;
            self.options.smooth_pen_up = true;
            self.status = "Loaded K9 starting values. Measure your machine before export.".into();
        }
        self.invalidate();
    }

    fn load_profile(&mut self, path: &Path) {
        match penplot::read_limited(path)
            .and_then(|data| String::from_utf8(data).map_err(|error| error.to_string()))
            .and_then(|text| Profile::from_json(&text))
        {
            Ok((profile, notices)) => {
                self.pending_export = None;
                if profile.machine_preset.is_some() {
                    self.air_first = true;
                }
                self.profile = profile;
                self.profile_notices = notices;
                self.status =
                    "Profile loaded. Check the measured setup before running on hardware.".into();
                self.invalidate();
            }
            Err(error) => self.status = format!("Unable to load {}: {error}", path.display()),
        }
    }

    fn save_profile(&mut self) {
        if let Err(error) = self.profile.validate() {
            self.status = format!("Profile is invalid: {error}");
            return;
        }
        if let Some(mut path) = rfd::FileDialog::new()
            .add_filter("Profile", &["json"])
            .set_file_name("plotter.json")
            .save_file()
        {
            let overwrite_confirmed_by_dialog = path.extension().is_some();
            if !overwrite_confirmed_by_dialog {
                path.set_extension("json");
            }
            let result = serde_json::to_vec_pretty(&self.profile)
                .map_err(|error| error.to_string())
                .and_then(|data| penplot::save_file(&path, &data, overwrite_confirmed_by_dialog));
            self.status = match result {
                Ok(()) => format!("Saved profile to {}.", path.display()),
                Err(error) => format!("Unable to save {}: {error}", path.display()),
            };
        }
    }

    fn min_stroke_controls(&mut self, ui: &mut egui::Ui) {
        let mut value = self.options.min_image_stroke_mm;
        ui.horizontal_wrapped(|ui| {
            ui.label("Min image stroke");
            ui.add(
                egui::DragValue::new(&mut value)
                    .range(0.0..=10_000.0)
                    .speed(0.1)
                    .suffix(" mm")
                    .max_decimals(3),
            )
            .on_hover_text(
                "Click to type a minimum or drag in 0.1 mm steps. 0 keeps all image paths.",
            );
            if ui
                .add_enabled(value > 0.0, egui::Button::new("Off"))
                .on_hover_text("Keep all image paths (0 mm).")
                .clicked()
            {
                value = 0.0;
            }
        });
        if self.raster_removed.0 > 0 {
            ui.small(format!(
                "Excluded {} image paths ({:.2} mm).",
                self.raster_removed.0, self.raster_removed.1
            ));
        } else {
            ui.small("0 = off. Applies to image paths only.");
        }
        if value != self.options.min_image_stroke_mm {
            self.record_layers();
            self.options.min_image_stroke_mm = value;
            self.invalidate();
            ui.ctx().request_repaint();
        }
    }

    fn route_panel(&mut self, ui: &mut egui::Ui) {
        section(ui, "PATH PLANNING");
        let mut draft = self.options.clone();
        let mut changed = ui
            .checkbox(&mut draft.optimize, "Reduce pen-up travel")
            .changed();
        changed |= ui.checkbox(&mut draft.smooth_pen_up, "Smooth pen-up travel (quadratic)")
            .on_hover_text("Off by default. Curves eligible pen-up links, not drawn lines. Firmware still controls acceleration.")
            .changed();
        if draft.smooth_pen_up
            && let Some(output) = &self.output
        {
            ui.small(format!(
                "{} curved links in current route.",
                output.curved_travel.len()
            ));
        }
        let label = match draft.fill {
            Fill::None => "Outlines only",
            Fill::Lines => "Hatch lines",
            Fill::Cross => "Cross-hatch",
        };
        egui::ComboBox::from_id_salt("fill")
            .width(ui.available_width().min(200.0))
            .selected_text(label)
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(&mut draft.fill, Fill::None, "Outlines only")
                    .changed();
                changed |= ui
                    .selectable_value(&mut draft.fill, Fill::Lines, "Hatch lines")
                    .changed();
                changed |= ui
                    .selectable_value(&mut draft.fill, Fill::Cross, "Cross-hatch")
                    .changed();
            });
        if draft.fill != Fill::None {
            changed |= number(ui, "Spacing (mm)", &mut draft.hatch_spacing, 0.1);
            changed |= number(ui, "Angle (deg)", &mut draft.hatch_angle, 1.0);
            ui.small("Hatching applies to closed, filled shapes.");
        }
        if changed {
            self.record_layers();
            self.options = draft;
            self.invalidate();
            ui.ctx().request_repaint();
        }
    }

    fn combined(&self) -> Result<Vec<Stroke>, String> {
        combine(&self.layers)
    }

    fn can_export(&self) -> bool {
        self.profile.export_blocker().is_none()
            && self.output.is_some()
            && !self.planned.is_empty()
            && !self.preview_dirty
            && self.placement_drag.is_none()
            && self.plan_task.is_none()
            && self.import_dialog.is_none()
            && self.import_task.is_none()
            && !self.raster_import.is_busy()
            && self.text_dialog.is_none()
            && self.error.is_none()
    }

    fn poll_plan(&mut self, ctx: &egui::Context) {
        if let Some(task) = &self.plan_task {
            match task.try_recv() {
                Ok(report) => {
                    self.plan_task = None;
                    if report.revision == self.revision {
                        self.update_preview(report);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.plan_task = None;
                    self.error =
                        Some("The planner stopped unexpectedly. Edit the job to retry.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if !self.preview_dirty || self.plan_task.is_some() {
            return;
        }
        self.preview_dirty = false;
        if !self.layers.iter().any(|layer| layer.visible) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        let snapshot = self.snapshot();
        let profile = self.profile.clone();
        let revision = self.revision;
        std::thread::spawn(move || {
            let report = calculate_plan(revision, &snapshot, &profile);
            let _ = sender.send(report);
            ctx.request_repaint();
        });
        self.plan_task = Some(receiver);
    }

    fn update_preview(&mut self, report: PlanReport) {
        self.error = None;
        self.output = None;
        self.raster_removed = report.removed;
        match report.result {
            Ok((planned, output)) => {
                self.planned = planned;
                self.route_targets = self.build_route_targets();
                self.shown = self.planned.len();
                match output {
                    Ok(output) => self.output = Some(Arc::new(output)),
                    Err(error) => self.error = Some(error),
                }
            }
            Err(error) => self.error = Some(error),
        }
    }

    #[cfg(test)]
    fn rebuild(&mut self) {
        self.preview_dirty = false;
        let report = calculate_plan(self.revision, &self.snapshot(), &self.profile);
        self.update_preview(report);
    }
}

fn combine(layers: &[Layer]) -> Result<Vec<Stroke>, String> {
    let mut all = Vec::new();
    for (index, layer) in layers.iter().enumerate().filter(|(_, layer)| layer.visible) {
        penplot::append_layer(&mut all, &layer.strokes, index)?;
    }
    Ok(all)
}

fn calculate_plan(revision: u64, snapshot: &Snapshot, profile: &Profile) -> PlanReport {
    let raster = snapshot
        .layers
        .iter()
        .enumerate()
        .filter(|(_, layer)| layer.visible)
        .filter_map(|(index, layer)| Some((index, layer.raster.clone()?)))
        .collect();
    let mut removed = (0, 0.0);
    let result = combine(&snapshot.layers).and_then(|strokes| {
        let plan = penplot::plan_with_raster(&strokes, &snapshot.options, &raster)?;
        removed = (plan.removed_count, plan.removed_mm);
        let output = if plan.strokes.is_empty() {
            Err("No drawable paths remain after filtering".into())
        } else {
            penplot::generate_with_travel(
                &plan.strokes,
                profile,
                false,
                snapshot.options.smooth_pen_up,
            )
        };
        Ok((plan.strokes, output))
    });
    PlanReport {
        revision,
        result,
        removed,
    }
}

fn number(ui: &mut egui::Ui, label: &str, field: &mut f64, speed: f64) -> bool {
    if ui.available_width() < 210.0 {
        ui.label(label);
        return ui
            .add(egui::DragValue::new(field).speed(speed).max_decimals(4))
            .changed();
    }
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(egui::DragValue::new(field).speed(speed).max_decimals(4))
                .changed()
        })
        .inner
    })
    .inner
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            shift: Vec2::ZERO,
        }
    }
}

#[derive(Clone, Copy)]
struct CanvasMap {
    anchor: Pos2,
    scale: f32,
}

impl CanvasMap {
    fn screen(self, p: Point) -> Pos2 {
        self.anchor + egui::vec2(p.x as f32 * self.scale, -(p.y as f32) * self.scale)
    }

    fn paper(self, p: Pos2) -> Point {
        let delta = (p - self.anchor) / self.scale;
        Point::new(f64::from(delta.x), -f64::from(delta.y))
    }

    fn rect(self, area: (f64, f64, f64, f64)) -> Rect {
        Rect::from_two_pos(
            self.screen(Point::new(area.0, area.1)),
            self.screen(Point::new(area.2, area.3)),
        )
    }
}

#[derive(Clone, Copy)]
enum PlacementHandle {
    Move,
    Scale(Point),
    Rotate(Point),
}

#[derive(Clone, Copy)]
enum PlacementMotion {
    Move(Point),
    Scale(Point, f64),
    Rotate(Point, f64),
}

impl PlacementMotion {
    fn identity(self) -> bool {
        match self {
            Self::Move(shift) => shift.x.hypot(shift.y) < 1e-8,
            Self::Scale(_, factor) => (factor - 1.0).abs() < 1e-8,
            Self::Rotate(_, degrees) => degrees.abs() < 1e-8,
        }
    }

    fn apply(self, point: Point) -> Point {
        match self {
            Self::Move(shift) => Point::new(point.x + shift.x, point.y + shift.y),
            Self::Scale(pivot, factor) => Point::new(
                pivot.x + (point.x - pivot.x) * factor,
                pivot.y + (point.y - pivot.y) * factor,
            ),
            Self::Rotate(pivot, degrees) => {
                let (sin, cos) = degrees.to_radians().sin_cos();
                let (x, y) = (point.x - pivot.x, point.y - pivot.y);
                Point::new(pivot.x + x * cos - y * sin, pivot.y + x * sin + y * cos)
            }
        }
    }
}

struct PlacementDrag {
    bounds: [Point; 4],
    cursor_start: Point,
    handle: PlacementHandle,
    layer: usize,
    revision: u64,
    motion: PlacementMotion,
}

impl PlacementDrag {
    fn affects(&self, stroke: &Stroke) -> bool {
        stroke
            .layer
            .split_once(':')
            .and_then(|(prefix, _)| prefix.parse::<usize>().ok())
            == Some(self.layer)
    }

    fn update(&mut self, cursor: Point, snap: bool) {
        let delta = Point::new(
            cursor.x - self.cursor_start.x,
            cursor.y - self.cursor_start.y,
        );
        self.motion = match self.handle {
            PlacementHandle::Move => {
                let shift = match snap {
                    true if delta.x.abs() >= delta.y.abs() => Point::new(delta.x, 0.0),
                    true => Point::new(0.0, delta.y),
                    false => delta,
                };
                PlacementMotion::Move(shift)
            }
            PlacementHandle::Scale(pivot) => {
                let (x, y) = (self.cursor_start.x - pivot.x, self.cursor_start.y - pivot.y);
                let denominator = x * x + y * y;
                if denominator <= f64::EPSILON {
                    return;
                }
                let factor = 1.0 + (delta.x * x + delta.y * y) / denominator;
                PlacementMotion::Scale(pivot, factor.clamp(1e-8, 1e8))
            }
            PlacementHandle::Rotate(pivot) => {
                let angle = |p: Point| (p.y - pivot.y).atan2(p.x - pivot.x);
                let mut degrees = ((angle(cursor) - angle(self.cursor_start)).to_degrees() + 180.0)
                    .rem_euclid(360.0)
                    - 180.0;
                if snap {
                    degrees = (degrees / 15.0).round() * 15.0;
                }
                PlacementMotion::Rotate(pivot, degrees)
            }
        };
    }
}

fn rotate_handle(points: [Pos2; 4]) -> Pos2 {
    let top = points[2].lerp(points[3], 0.5);
    let center = points[0].lerp(points[2], 0.5);
    let direction = (top - center).normalized();
    top + if direction == Vec2::ZERO {
        egui::vec2(0.0, -24.0)
    } else {
        direction * 24.0
    }
}

fn scale_handle_radius(corners: [Pos2; 4]) -> f32 {
    // Keep the body draggable even when all four handles are close together.
    (corners[0].distance(corners[2]) * 0.28).min(11.0)
}

fn draw_placement_handles(painter: &egui::Painter, map: CanvasMap, bounds: [Point; 4], tool: Tool) {
    let points = bounds.map(|point| map.screen(point));
    let color = Color32::from_rgb(30, 125, 111);
    painter.add(egui::Shape::closed_line(
        points.to_vec(),
        Ink::new(1.4, color),
    ));
    match tool {
        Tool::Transform => {
            let size = (1.2 * scale_handle_radius(points)).clamp(2.0, 9.0);
            for point in points {
                painter.rect_filled(Rect::from_center_size(point, Vec2::splat(size)), 2.0, color);
            }
        }
        Tool::Rotate => {
            let handle = rotate_handle(points);
            painter.line_segment(
                [points[2].lerp(points[3], 0.5), handle],
                Ink::new(1.4, color),
            );
            painter.circle_filled(handle, 5.0, color);
        }
        _ => {}
    }
}

fn placement_handle(
    tool: Tool,
    press: Pos2,
    corners: [Pos2; 4],
    bounds: [Point; 4],
) -> Option<PlacementHandle> {
    let near = |point: Pos2| press.distance(point) <= 11.0;
    match tool {
        Tool::Transform => corners
            .iter()
            .enumerate()
            .filter(|(i, corner)| {
                press.distance(**corner) <= scale_handle_radius(corners)
                    && corner.distance(corners[(*i + 2) % 4]) > 0.2
            })
            .min_by(|(_, a), (_, b)| press.distance(**a).total_cmp(&press.distance(**b)))
            .map(|(i, _)| PlacementHandle::Scale(bounds[(i + 2) % 4]))
            .or_else(|| {
                Rect::from_two_pos(corners[0], corners[2])
                    .expand(9.0)
                    .contains(press)
                    .then_some(PlacementHandle::Move)
            }),
        Tool::Rotate if near(rotate_handle(corners)) => Some(PlacementHandle::Rotate(Point::new(
            (bounds[0].x + bounds[2].x) * 0.5,
            (bounds[0].y + bounds[2].y) * 0.5,
        ))),
        _ => None,
    }
}

impl Studio {
    fn placement_layer(&self) -> Option<usize> {
        let index = self.selected?.layer();
        self.layers
            .get(index)
            .filter(|layer| layer.visible && !layer.strokes.is_empty())
            .map(|_| index)
    }

    fn placement_bounds(&mut self) -> Option<[Point; 4]> {
        let index = self.placement_layer()?;
        if let Some((revision, cached_layer, result)) = self.placement_bounds_cache
            && revision == self.revision
            && cached_layer == index
        {
            return result;
        }
        let opts = &self.options;
        let (sin, cos) = opts.rotation_deg.rem_euclid(360.0).to_radians().sin_cos();
        let result = if opts.scale > 0.0
            && [opts.scale, opts.x, opts.y, sin, cos]
                .into_iter()
                .all(|value| value.is_finite() && value.abs() <= 1e8)
        {
            projected_min(&self.layers, opts.scale, sin, cos).and_then(|all| {
                let (lo, hi) = projected_bounds(&self.layers[index..=index], opts.scale, sin, cos)?;
                let (dx, dy) = (opts.x - all.x, opts.y - all.y);
                let (left, bottom, right, top) = (lo.x + dx, lo.y + dy, hi.x + dx, hi.y + dy);
                (right.is_finite() && top.is_finite()).then_some([
                    Point::new(left, bottom),
                    Point::new(right, bottom),
                    Point::new(right, top),
                    Point::new(left, top),
                ])
            })
        } else {
            None
        };
        self.placement_bounds_cache = Some((self.revision, index, result));
        result
    }

    fn transform_layer(&mut self, index: usize, motion: PlacementMotion) -> Result<(), String> {
        let error = || "Invalid layer transform or placement.".to_owned();
        if motion.identity() {
            return Ok(());
        }
        if self.placement_layer() != Some(index) {
            return Err(error());
        }
        let valid = |p: Point| [p.x, p.y].into_iter().all(|v| (-1e8..=1e8).contains(&v));
        let scale = self.options.scale;
        if !(0.0..=1e8).contains(&scale) || scale == 0.0 {
            return Err(error());
        }
        let (sin, cos) = self
            .options
            .rotation_deg
            .rem_euclid(360.0)
            .to_radians()
            .sin_cos();
        let old_min = projected_min(&self.layers, scale, sin, cos).ok_or_else(error)?;
        let shift = Point::new(self.options.x - old_min.x, self.options.y - old_min.y);
        let origin = motion.apply(shift);
        let (x, y) = ((origin.x - shift.x) / scale, (origin.y - shift.y) / scale);
        let offset = Point::new(x * cos + y * sin, -x * sin + y * cos);
        // Uniform scales and rotations commute in 2D. Only the shift needs conjugation.
        let linear = match motion {
            PlacementMotion::Move(_) => (1.0, 0.0),
            PlacementMotion::Scale(_, factor) if (1e-8..=1e8).contains(&factor) => (factor, 0.0),
            PlacementMotion::Scale(..) => return Err(error()),
            PlacementMotion::Rotate(_, degrees) => {
                let (sin, cos) = degrees.to_radians().sin_cos();
                (cos, sin)
            }
        };
        let direction = |p: Point| {
            let (c, s) = linear;
            Point::new(c * p.x - s * p.y, s * p.x + c * p.y)
        };
        let map = |p| {
            let p = direction(p);
            Point::new(p.x + offset.x, p.y + offset.y)
        };
        let mut edited = self.layers[index].clone();
        let mut strokes = edited.strokes.to_vec();
        for point in strokes.iter_mut().flat_map(|stroke| &mut stroke.points) {
            *point = map(*point);
            if !valid(*point) {
                return Err(error());
            }
        }
        edited.strokes = strokes.into();
        if let Some(text) = &mut edited.text {
            let [a, b, c, d, x, y] = text.transform;
            let (u, v, p) = (
                direction(Point::new(a, b)),
                direction(Point::new(c, d)),
                map(Point::new(x, y)),
            );
            text.transform = [u.x, u.y, v.x, v.y, p.x, p.y];
            text.validate()?;
        }
        let mut candidate = self.layers.clone();
        candidate[index] = edited;
        let new_min = projected_min(&candidate, scale, sin, cos).ok_or_else(error)?;
        // The planner normalizes to the visible artwork's min before placing.
        // Rebase that anchor so the unedited layers stay on the same paper coordinates.
        let anchor = Point::new(shift.x + new_min.x, shift.y + new_min.y);
        if !valid(anchor) {
            return Err(error());
        }
        self.record_layers();
        self.layers = candidate;
        self.options.x = anchor.x;
        self.options.y = anchor.y;
        self.invalidate();
        Ok(())
    }

    fn finish_layer_placement(
        &mut self,
        index: usize,
        motion: PlacementMotion,
        ctx: &egui::Context,
    ) {
        if let Err(error) = self.transform_layer(index, motion) {
            self.status = error;
        }
        ctx.request_repaint();
    }

    fn handle_placement(&mut self, ui: &egui::Ui, response: &egui::Response, map: CanvasMap) {
        let primary = egui::PointerButton::Primary;
        if self.placement_drag.is_none()
            && response.drag_started_by(primary)
            && self.import_task.is_none()
            && self.import_dialog.is_none()
            && !self.preview_dirty
            && self.plan_task.is_none()
            && let Some(press) = ui.input(|input| input.pointer.press_origin())
            && let Some(layer) = self.placement_layer()
            && let Some(bounds) = self.placement_bounds()
        {
            let handle = placement_handle(self.tools, press, bounds.map(|p| map.screen(p)), bounds);
            if let Some(handle) = handle {
                self.play = false;
                self.shown = self.planned.len();
                self.placement_drag = Some(PlacementDrag {
                    bounds,
                    cursor_start: map.paper(press),
                    handle,
                    layer,
                    revision: self.revision,
                    motion: PlacementMotion::Move(Point::new(0.0, 0.0)),
                });
            }
        }
        if let Some(drag) = &mut self.placement_drag
            && let Some(cursor) = ui.input(|input| input.pointer.interact_pos())
        {
            drag.update(map.paper(cursor), ui.input(|input| input.modifiers.shift));
        }
        if response.drag_stopped_by(primary)
            && let Some(drag) = self.placement_drag.take()
            && self.revision == drag.revision
        {
            self.finish_layer_placement(drag.layer, drag.motion, ui.ctx());
        }
        if !ui.input(|input| input.pointer.button_down(primary)) {
            self.placement_drag = None;
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.isometric, false, "2D");
            ui.selectable_value(&mut self.isometric, true, "3D lifts");
            ui.separator();
            if self.isometric {
                ui.label("Z exaggeration");
                ui.add(
                    egui::DragValue::new(&mut self.z_exaggeration)
                        .range(1.0..=12.0)
                        .speed(0.1)
                        .suffix("×"),
                );
                ui.label(RichText::new("Drawing simulation · not collision checked").weak());
                return;
            }
            if ui.button("Reset view").clicked() {
                self.frame = Camera::default();
            }
            if ui
                .add_enabled(
                    self.placement_drag.is_none() && self.layers.iter().any(|layer| layer.visible),
                    egui::Button::new(if self.japanese {
                        "ベッドに合わせる"
                    } else {
                        "Fit to bed"
                    }),
                )
                .on_hover_text("Fit the visible artwork with a 5 mm margin and reset the camera.")
                .clicked()
            {
                self.fit_and_reset_view();
                ui.ctx().request_repaint();
            }
            ui.checkbox(&mut self.grayscale_preview, if self.japanese {"塗りプレビュー（グレー）"} else {"Grayscale fill preview"});
            let hint = match self.tools {
                Tool::Bucket => "Click inside a closed region to apply the selected pattern",
                Tool::Transform if self.japanese => {
                    "選択レイヤー：内側で移動 · 角で拡大縮小 · Shiftで軸固定 · Escで取消"
                }
                Tool::Transform => {
                    "Selected layer: drag inside to move · corners resize · Shift: one axis · Esc: cancel"
                }
                Tool::Rotate => "Drag the circular handle · Shift: 15° steps · Esc: cancel",
                _ => "Wheel: zoom · Middle drag: pan",
            };
            ui.label(RichText::new(hint).weak());
        });
        if self.isometric {
            self.draft.clear();
            self.placement_drag = None;
            self.isometric_canvas(ui);
            return;
        }
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let base = response.rect;
        let bed_x = if (0.001..=10_000.0).contains(&self.profile.bed_x) {
            self.profile.bed_x
        } else {
            100.0
        };
        let bed_y = if (0.001..=10_000.0).contains(&self.profile.bed_y) {
            self.profile.bed_y
        } else {
            100.0
        };
        let fit = ((base.width() - 68.0).max(20.0) / bed_x as f32)
            .min((base.height() - 64.0).max(20.0) / bed_y as f32);
        let camera_map = |camera: &Camera| {
            let scale = fit * camera.zoom;
            let offset = egui::vec2(-bed_x as f32, bed_y as f32) * (scale * 0.5);
            CanvasMap {
                anchor: base.center() + offset + camera.shift,
                scale,
            }
        };
        if self.placement_drag.is_none()
            && let Some(pointer) = response.hover_pos()
        {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let before = camera_map(&self.frame).paper(pointer);
                self.frame.zoom = (self.frame.zoom * (scroll / 450.0).exp()).clamp(0.25, 32.0);
                let after = camera_map(&self.frame).screen(before);
                self.frame.shift += pointer - after;
            }
        }
        let panning = response.dragged_by(egui::PointerButton::Middle)
            || (self.tools == Tool::Pan && response.dragged_by(egui::PointerButton::Primary));
        if panning && self.placement_drag.is_none() {
            self.frame.shift += response.drag_delta();
        }
        let map = camera_map(&self.frame);
        let painter = painter.with_clip_rect(base);
        let paper_rect = map.rect((0.0, 0.0, bed_x, bed_y));
        painter.rect_filled(base, 7.0, Color32::from_rgb(15, 21, 27));
        painter.rect_filled(
            paper_rect,
            0.0,
            if self.grayscale_preview {
                Color32::WHITE
            } else {
                PAPER
            },
        );
        if !self.grayscale_preview {
            self.grid(&painter, map, bed_x, bed_y);
        }
        painter.rect_stroke(
            paper_rect,
            0.0,
            Ink::new(1.5, BED),
            egui::StrokeKind::Outside,
        );
        let area = self.profile.effective_area();
        if [area.0, area.1, area.2, area.3]
            .into_iter()
            .all(f64::is_finite)
            && area.0 < area.2
            && area.1 < area.3
        {
            painter.rect_stroke(
                map.rect(area),
                0.0,
                Ink::new(1.5, Color32::from_rgb(66, 143, 118)),
                egui::StrokeKind::Inside,
            );
        }
        // This is the machine origin expressed in physical bed coordinates, not the artwork origin.
        let origin = match self.profile.origin {
            Origin::LeftBottom => Point::new(0.0, 0.0),
            Origin::Center => Point::new(bed_x / 2.0, bed_y / 2.0),
        };
        let center = map.screen(origin);
        painter.line_segment(
            [center - egui::vec2(6.0, 0.0), center + egui::vec2(6.0, 0.0)],
            Ink::new(1.5, Color32::DARK_RED),
        );
        painter.line_segment(
            [center - egui::vec2(0.0, 6.0), center + egui::vec2(0.0, 6.0)],
            Ink::new(1.5, Color32::DARK_RED),
        );
        if !self.layers.iter().any(|layer| layer.visible)
            && self.error.is_none()
            && self.import_task.is_none()
            && self.plan_task.is_none()
        {
            painter.text(
                paper_rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop artwork to start",
                FontId::proportional(17.0),
                Color32::from_rgb(78, 93, 91),
            );
        }
        self.object_canvas_ui(ui, &response, map);
        if self.tools.placement() {
            self.handle_placement(ui, &response, map);
            if let Some(drag) = &self.placement_drag {
                self.paint_route_transformed(&painter, map, Some(drag));
                draw_placement_handles(
                    &painter,
                    map,
                    drag.bounds.map(|point| drag.motion.apply(point)),
                    self.tools,
                );
            } else {
                self.paint_route(&painter, map);
                if let Some(bounds) = self.placement_bounds() {
                    draw_placement_handles(&painter, map, bounds, self.tools);
                }
            }
        } else {
            self.paint_route(&painter, map);
            if self.tools == Tool::Bucket {
                self.bucket_canvas(&response, map);
            } else if self.tools != Tool::Pan {
                self.draw_on_bed(ui, &response, &painter, map);
            }
        }
        if let Some(at) = response.hover_pos() {
            let point = map.paper(at);
            painter.text(
                base.right_bottom() - egui::vec2(12.0, 8.0),
                egui::Align2::RIGHT_BOTTOM,
                format!("Paper X {:.1}  Y {:.1} mm", point.x, point.y),
                FontId::monospace(11.0),
                Color32::from_rgb(148, 166, 176),
            );
        }
        painter.text(
            base.left_top() + egui::vec2(11.0, 10.0),
            egui::Align2::LEFT_TOP,
            format!("BED  {} × {} mm", format_mm(bed_x), format_mm(bed_y)),
            FontId::monospace(11.0),
            BED,
        );
    }

    fn isometric_canvas(&self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::hover());
        let base = response.rect;
        let painter = painter.with_clip_rect(base);
        painter.rect_filled(base, 7.0, Color32::from_rgb(15, 21, 27));
        let bed_x = self.profile.bed_x;
        let bed_y = self.profile.bed_y;
        let up = self.profile.clearance_z();
        let down = self.profile.pen_down_z + self.profile.offset_z;
        let Some(map) = IsoMap::fit(
            base.shrink2(egui::vec2(32.0, 55.0)),
            bed_x,
            bed_y,
            up,
            down,
            self.z_exaggeration,
        ) else {
            painter.text(
                base.center(),
                egui::Align2::CENTER_CENTER,
                "Check the machine dimensions and Z settings.",
                FontId::proportional(15.0),
                WARN,
            );
            return;
        };
        let corners = [
            Point::new(0.0, 0.0),
            Point::new(bed_x, 0.0),
            Point::new(bed_x, bed_y),
            Point::new(0.0, bed_y),
        ];
        let screen: Vec<_> = corners
            .into_iter()
            .map(|point| map.screen(point, 0.0))
            .collect();
        painter.add(egui::Shape::convex_polygon(
            screen,
            Color32::from_rgb(38, 53, 59),
            Ink::new(1.3, Color32::from_rgb(83, 113, 118)),
        ));
        let grid = Ink::new(1.0, Color32::from_rgb(59, 78, 82));
        for i in 1..10 {
            let x = bed_x * f64::from(i) / 10.0;
            let y = bed_y * f64::from(i) / 10.0;
            painter.line_segment(
                [
                    map.screen(Point::new(x, 0.0), 0.0),
                    map.screen(Point::new(x, bed_y), 0.0),
                ],
                grid,
            );
            painter.line_segment(
                [
                    map.screen(Point::new(0.0, y), 0.0),
                    map.screen(Point::new(bed_x, y), 0.0),
                ],
                grid,
            );
        }
        for (point, axis) in [
            (Point::new(bed_x, 0.0), "+X"),
            (Point::new(0.0, bed_y), "+Y"),
        ] {
            painter.text(
                map.screen(point, 0.0),
                egui::Align2::CENTER_TOP,
                axis,
                FontId::monospace(11.0),
                BED,
            );
        }
        let canvas = painter.clip_rect().expand(2.0);
        trace_segments_with_curves(
            &self.planned[..self.shown.min(self.planned.len())],
            self.output
                .as_deref()
                .map_or(&[], |output| &output.curved_travel),
            up,
            down,
            |segment| {
                let a = map.screen(segment.from.0, segment.from.1);
                let b = map.screen(segment.to.0, segment.to.1);
                if !Rect::from_two_pos(a, b).intersects(canvas) {
                    return;
                }
                match segment.kind {
                    MoveKind::Travel if self.show_travel => {
                        paint_dashes(&painter, a, b, Ink::new(1.1, WARN))
                    }
                    MoveKind::Travel => (),
                    _ => {
                        painter.line_segment([a, b], Ink::new(1.6, segment.kind.color()));
                    }
                }
            },
        );
        let mut legend = base.left_top() + egui::vec2(15.0, 13.0);
        for (kind, name) in [
            (MoveKind::Draw, "Draw"),
            (MoveKind::Travel, "Travel"),
            (MoveKind::Lift, "Lift"),
            (MoveKind::Lower, "Lower"),
        ] {
            painter.line_segment(
                [
                    legend + egui::vec2(0.0, 7.0),
                    legend + egui::vec2(15.0, 7.0),
                ],
                Ink::new(2.0, kind.color()),
            );
            painter.text(
                legend + egui::vec2(20.0, 0.0),
                egui::Align2::LEFT_TOP,
                name,
                FontId::proportional(13.0),
                BED,
            );
            legend.x += 72.0;
        }
        painter.text(
            base.left_bottom() + egui::vec2(15.0, -13.0),
            egui::Align2::LEFT_BOTTOM,
            format!("Commanded Z: up {up:.3} mm   /   down {down:.3} mm"),
            FontId::monospace(11.0),
            BED,
        );
    }

    fn grid(&self, painter: &egui::Painter, map: CanvasMap, bed_x: f64, bed_y: f64) {
        let mut step = 5.0_f64;
        while (step as f32 * map.scale) < 13.0 || bed_x.max(bed_y) / step > 80.0 {
            step *= 2.0;
        }
        let ink = Ink::new(1.0, GRID);
        for i in 1..=(bed_x / step) as u32 {
            let x = f64::from(i) * step;
            painter.line_segment(
                [
                    map.screen(Point::new(x, 0.0)),
                    map.screen(Point::new(x, bed_y)),
                ],
                ink,
            );
        }
        for i in 1..=(bed_y / step) as u32 {
            let y = f64::from(i) * step;
            painter.line_segment(
                [
                    map.screen(Point::new(0.0, y)),
                    map.screen(Point::new(bed_x, y)),
                ],
                ink,
            );
        }
    }

    fn paint_route(&self, painter: &egui::Painter, map: CanvasMap) {
        self.paint_route_transformed(painter, map, None);
    }

    fn paint_route_transformed(
        &self,
        painter: &egui::Painter,
        map: CanvasMap,
        drag: Option<&PlacementDrag>,
    ) {
        let mut previous = None;
        let clipped = painter.clip_rect().expand(2.0);
        let mut curves = self
            .output
            .iter()
            .flat_map(|output| &output.curved_travel)
            .peekable();
        for (index, stroke) in self.planned.iter().enumerate() {
            let Some(&first) = stroke.points.first() else {
                continue;
            };
            let motion = drag
                .filter(|drag| drag.affects(stroke))
                .map(|drag| drag.motion);
            let transform = |point| motion.map_or(point, |motion| motion.apply(point));
            let start = map.screen(transform(first));
            let active = index < self.shown;
            if active
                && self.show_travel
                && drag.is_none()
                && let Some(last) = previous
            {
                let ink = Ink::new(1.2, Color32::from_rgb(209, 114, 59));
                if let Some(curve) = curves.next_if(|curve| curve.destination_stroke == index) {
                    for points in curve.paper_points.windows(2) {
                        paint_dashes(
                            painter,
                            map.screen(transform(points[0])),
                            map.screen(transform(points[1])),
                            ink,
                        );
                    }
                } else {
                    paint_dashes(painter, last, start, ink);
                }
            }
            let color = if active {
                route_color(&stroke.layer)
            } else {
                Color32::from_rgb(193, 200, 198)
            };
            let selected = self
                .selected
                .zip(self.route_targets.get(index).copied().flatten())
                .is_some_and(|(selected, target)| selected.contains(target));
            let ink = if let Some(pattern) = self.pattern_for_route(stroke) {
                Ink::new(
                    (pattern.pen_width_mm as f32 * map.scale).max(0.15),
                    if active {
                        Color32::BLACK
                    } else {
                        Color32::from_gray(190)
                    },
                )
            } else if self.grayscale_preview {
                Ink::new(
                    1.25,
                    if active {
                        Color32::from_gray(65)
                    } else {
                        Color32::from_gray(190)
                    },
                )
            } else if selected {
                Ink::new(
                    2.4,
                    if penplot::ink_rgb(&stroke.layer).is_some() {
                        color
                    } else {
                        Color32::from_rgb(20, 150, 115)
                    },
                )
            } else {
                Ink::new(if active { 1.7 } else { 1.25 }, color)
            };
            let mut last = start;
            for &point in &stroke.points[1..] {
                let next = map.screen(transform(point));
                if Rect::from_two_pos(last, next).intersects(clipped) {
                    painter.line_segment([last, next], ink);
                }
                last = next;
            }
            if stroke.closed {
                painter.line_segment([last, start], ink);
                last = start;
            }
            previous = Some(last);
        }
    }

    fn draw_on_bed(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        painter: &egui::Painter,
        map: CanvasMap,
    ) {
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.draft.clear();
            if let Some(position) = ui.input(|input| input.pointer.press_origin()) {
                self.draft
                    .push(map.paper(position.clamp(response.rect.min, response.rect.max)));
            }
        }
        if !self.draft.is_empty()
            && (response.dragged_by(egui::PointerButton::Primary)
                || response.drag_stopped_by(egui::PointerButton::Primary))
            && let Some(position) = ui.input(|input| input.pointer.interact_pos())
        {
            let point = map.paper(position.clamp(response.rect.min, response.rect.max));
            if self.tools == Tool::Freehand {
                let last = self.draft.last().copied().unwrap_or(point);
                if self.draft.len() < 8192
                    && ((last.x - point.x).hypot(last.y - point.y) >= 0.08
                        || response.drag_stopped_by(egui::PointerButton::Primary))
                {
                    self.draft.push(point);
                }
            } else {
                self.draft.truncate(1);
                self.draft.push(point);
            }
        }
        let Some((points, closed)) = draft_geometry(self.tools, &self.draft) else {
            return;
        };
        if response.drag_stopped_by(egui::PointerButton::Primary) {
            self.draft.clear();
            if let Err(error) = self.add_mark(points, closed) {
                self.status = error;
            }
            ui.ctx().request_repaint();
        } else {
            let points: Vec<_> = points.into_iter().map(|point| map.screen(point)).collect();
            let ink = Ink::new(1.8, Color32::from_rgb(37, 139, 121));
            if closed {
                painter.add(egui::Shape::closed_line(points, ink));
            } else {
                painter.add(egui::Shape::line(points, ink));
            }
        }
    }

    fn add_mark(&mut self, mut points: Vec<Point>, closed: bool) -> Result<(), String> {
        if self.import_task.is_some() {
            return Err("Wait for the current import before drawing.".into());
        }
        let (paths_used, points_used) = self.geometry_size();
        if paths_used >= penplot::MAX_STROKES || points_used + points.len() > penplot::MAX_POINTS {
            return Err(
                "Workspace geometry limit reached. Remove other layers before drawing.".into(),
            );
        }
        let scale = self.options.scale;
        let (sin, cos) = self
            .options
            .rotation_deg
            .rem_euclid(360.0)
            .to_radians()
            .sin_cos();
        if !scale.is_finite()
            || scale <= 0.0
            || !cos.is_finite()
            || !self.options.x.is_finite()
            || !self.options.y.is_finite()
        {
            return Err("Correct the artwork placement before adding a mark.".into());
        }
        let raw_min = projected_min(&self.layers, scale, sin, cos);
        let shift = match raw_min {
            Some(min) => Point::new(self.options.x - min.x, self.options.y - min.y),
            None => Point::new(0.0, 0.0),
        };
        let mut left = self.options.x;
        let mut bottom = self.options.y;
        if raw_min.is_none() {
            left = f64::INFINITY;
            bottom = f64::INFINITY;
        }
        for point in &mut points {
            left = left.min(point.x);
            bottom = bottom.min(point.y);
            let x = (point.x - shift.x) / scale;
            let y = (point.y - shift.y) / scale;
            *point = Point::new(x * cos + y * sin, -x * sin + y * cos);
        }
        let existing = self.layers.last().is_some_and(|layer| {
            layer.visible
                && layer.name == "Canvas marks"
                && matches!(layer.strokes.first(), Some(stroke) if stroke.layer == "Canvas marks")
        });
        if !existing && self.layers.len() >= MAX_LAYER_COUNT {
            return Err("Layer limit reached. Remove a layer before adding canvas marks.".into());
        }
        let group = if existing {
            self.layers
                .last()
                .unwrap()
                .strokes
                .iter()
                .map(|stroke| stroke.group)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("Canvas group limit reached")?
        } else {
            0
        };
        self.record_layers();
        self.options.x = left;
        self.options.y = bottom;
        let mut strokes = if existing {
            self.layers
                .pop()
                .expect("Canvas marks layer was checked above")
                .strokes
                .to_vec()
        } else {
            Vec::new()
        };
        let stroke_index = strokes.len();
        strokes.push(Stroke {
            points,
            layer: "Canvas marks".into(),
            group,
            closed,
            filled: false,
        });
        self.layers.push(Layer {
            name: "Canvas marks".into(),
            strokes: strokes.into(),
            visible: true,
            raster: None,
            text: None,
        });
        self.selected = Some(Selection::Mark {
            layer: self.layers.len() - 1,
            stroke: stroke_index,
        });
        self.status = format!("Added {} mark. Undo removes it.", self.tools.label());
        self.invalidate();
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct IsoMap {
    anchor: Pos2,
    scale: f32,
    z_exaggeration: f64,
}

impl IsoMap {
    fn fit(
        rect: Rect,
        bed_x: f64,
        bed_y: f64,
        up: f64,
        down: f64,
        z_exaggeration: f64,
    ) -> Option<Self> {
        if !(0.001..=10_000.0).contains(&bed_x)
            || !(0.001..=10_000.0).contains(&bed_y)
            || !up.is_finite()
            || !down.is_finite()
            || up.abs() > 20_000.0
            || down.abs() > 20_000.0
        {
            return None;
        }
        let z_span = up.max(down).max(0.0) - up.min(down).min(0.0);
        let z_top = up.max(down).max(0.0);
        let x_lo = -0.866_025_4 * bed_y;
        let x_hi = 0.866_025_4 * bed_x;
        let y_lo = -z_top * z_exaggeration;
        let y_hi = y_lo + (bed_x + bed_y) * 0.5 + z_span * z_exaggeration;
        let scale = (rect.width().max(20.0) / (x_hi - x_lo) as f32)
            .min(rect.height().max(20.0) / (y_hi - y_lo) as f32);
        Some(Self {
            anchor: rect.center()
                - egui::vec2((x_lo + x_hi) as f32 * 0.5, (y_lo + y_hi) as f32 * 0.5) * scale,
            scale,
            z_exaggeration,
        })
    }

    fn screen(self, point: Point, z: f64) -> Pos2 {
        self.anchor
            + egui::vec2(
                ((point.x - point.y) * 0.866_025_4) as f32,
                ((point.x + point.y) * 0.5 - z * self.z_exaggeration) as f32,
            ) * self.scale
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MoveKind {
    Draw,
    Travel,
    Lift,
    Lower,
}

impl MoveKind {
    fn color(self) -> Color32 {
        match self {
            Self::Draw => Color32::from_rgb(116, 187, 252),
            Self::Travel => WARN,
            Self::Lift => ACCENT,
            Self::Lower => Color32::from_rgb(239, 153, 222),
        }
    }
}

struct TraceSegment {
    from: (Point, f64),
    to: (Point, f64),
    kind: MoveKind,
}

#[cfg(test)]
fn trace_segments(strokes: &[Stroke], up: f64, down: f64, emit: impl FnMut(TraceSegment)) {
    trace_segments_with_curves(strokes, &[], up, down, emit);
}

fn trace_segments_with_curves(
    strokes: &[Stroke],
    curves: &[penplot::CurvedTravel],
    up: f64,
    down: f64,
    mut emit: impl FnMut(TraceSegment),
) {
    let mut previous = None;
    let mut curves = curves.iter().peekable();
    let mut line = |from, to, kind| emit(TraceSegment { from, to, kind });
    for (index, stroke) in strokes.iter().enumerate() {
        let Some(&first) = stroke.points.first() else {
            continue;
        };
        if let Some(last) = previous {
            if let Some(curve) = curves.next_if(|curve| curve.destination_stroke == index) {
                for points in curve.paper_points.windows(2) {
                    line((points[0], up), (points[1], up), MoveKind::Travel);
                }
            } else {
                line((last, up), (first, up), MoveKind::Travel);
            }
        }
        line((first, up), (first, down), MoveKind::Lower);
        let mut last = first;
        for &next in &stroke.points[1..] {
            line((last, down), (next, down), MoveKind::Draw);
            last = next;
        }
        if stroke.closed && last != first {
            line((last, down), (first, down), MoveKind::Draw);
            last = first;
        }
        line((last, down), (last, up), MoveKind::Lift);
        previous = Some(last);
    }
}

fn projected_min(layers: &[Layer], scale: f64, sin: f64, cos: f64) -> Option<Point> {
    projected_bounds(layers, scale, sin, cos)
        .map(|(lo, _)| lo)
        .or_else(|| {
            // Keep an anchor while ALL layers are hidden, including save/reopen.
            // Visibility transitions rebase to/from this deterministic reference.
            let all: Vec<_> = layers
                .iter()
                .cloned()
                .map(|mut layer| {
                    layer.visible = true;
                    layer
                })
                .collect();
            projected_bounds(&all, scale, sin, cos).map(|(lo, _)| lo)
        })
}

fn projected_bounds(layers: &[Layer], scale: f64, sin: f64, cos: f64) -> Option<(Point, Point)> {
    let mut lo = Point::new(f64::INFINITY, f64::INFINITY);
    let mut hi = Point::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for point in layers
        .iter()
        .filter(|layer| layer.visible)
        .flat_map(|layer| layer.strokes.iter())
        .flat_map(|stroke| stroke.points.iter())
    {
        let x = (point.x * cos - point.y * sin) * scale;
        let y = (point.x * sin + point.y * cos) * scale;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        lo.x = lo.x.min(x);
        lo.y = lo.y.min(y);
        hi.x = hi.x.max(x);
        hi.y = hi.y.max(y);
    }
    lo.x.is_finite().then_some((lo, hi))
}

fn format_mm(value: f64) -> String {
    if value.fract().abs() < 0.001 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn route_color(layer: &str) -> Color32 {
    penplot::ink_rgb(layer).map_or(Color32::from_rgb(37, 75, 124), |[r, g, b]| {
        Color32::from_rgb(r, g, b)
    })
}

fn paint_dashes(painter: &egui::Painter, from: Pos2, to: Pos2, ink: Ink) {
    // Limit work at high zoom and clip before tessellating very long trips.
    let rect = painter.clip_rect();
    let distance = (to - from).length();
    if !(0.5..=80_000.0).contains(&distance) || !Rect::from_two_pos(from, to).intersects(rect) {
        return;
    }
    let unit = (to - from) / distance;
    let mut along = 0.0;
    let period = 8.0_f32.max(distance / 500.0);
    while along < distance {
        let start = from + unit * along;
        let end = from + unit * (along + period * 0.5).min(distance);
        if Rect::from_two_pos(start, end).intersects(rect) {
            painter.line_segment([start, end], ink);
        }
        along += period;
    }
}

fn draft_geometry(tool: Tool, points: &[Point]) -> Option<(Vec<Point>, bool)> {
    let (first, last) = (*points.first()?, *points.last()?);
    let dx = last.x - first.x;
    let dy = last.y - first.y;
    let line = match tool {
        Tool::Bucket | Tool::Pan | Tool::Transform | Tool::Rotate => return None,
        Tool::Freehand if points.len() >= 2 => {
            let distinct = points
                .iter()
                .any(|p| (p.x - first.x).hypot(p.y - first.y) >= 0.04);
            distinct.then(|| (points.to_vec(), false))?
        }
        Tool::Line if dx.hypot(dy) >= 0.04 => (vec![first, last], false),
        Tool::Rectangle if dx.abs() >= 0.04 && dy.abs() >= 0.04 => (
            vec![
                first,
                Point::new(last.x, first.y),
                last,
                Point::new(first.x, last.y),
            ],
            true,
        ),
        Tool::Ellipse if dx.abs() >= 0.04 && dy.abs() >= 0.04 => {
            let rx = dx.abs() * 0.5;
            let ry = dy.abs() * 0.5;
            let (cx, cy) = ((last.x + first.x) * 0.5, (last.y + first.y) * 0.5);
            let max_r = rx.max(ry);
            let count = (std::f64::consts::PI / (1.0 - 0.05 / max_r).clamp(0.0, 1.0).acos())
                .ceil()
                .clamp(16.0, 4096.0) as usize;
            let path = (0..count)
                .map(|i| {
                    let t = i as f64 / count as f64 * std::f64::consts::TAU;
                    Point::new(cx + rx * t.cos(), cy + ry * t.sin())
                })
                .collect();
            (path, true)
        }
        _ => return None,
    };
    Some(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egui_draws_empty_and_populated_frames_headlessly() {
        let context = egui::Context::default();
        let mut app = Studio::default();
        let draw_frame = |app: &mut Studio| {
            let frame = context.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1240.0, 780.0))),
                    ..Default::default()
                },
                |context| app.update_ui(context),
            );
            let primitives = context.tessellate(frame.shapes, frame.pixels_per_point);
            assert!(!primitives.is_empty());
        };
        draw_frame(&mut app);
        app.add_mark(vec![Point::new(10.0, 12.0), Point::new(42.0, 55.0)], false)
            .unwrap();
        app.rebuild();
        assert!(app.output.is_some());
        for workspace in [Workspace::Edit, Workspace::Import, Workspace::Preview] {
            app.switch_workspace(workspace);
            draw_frame(&mut app);
        }
        app.switch_workspace(Workspace::Edit);
        app.isometric = true;
        draw_frame(&mut app);
    }

    #[test]
    fn isometric_projection_separates_bed_axes_and_raises_z_on_screen() {
        let map = IsoMap::fit(
            Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0)),
            100.0,
            100.0,
            5.0,
            2.0,
            3.0,
        )
        .unwrap();
        let origin = Point::new(0.0, 0.0);
        assert!(map.screen(Point::new(10.0, 0.0), 0.0).x > map.screen(origin, 0.0).x);
        assert!(map.screen(Point::new(0.0, 10.0), 0.0).x < map.screen(origin, 0.0).x);
        assert!(map.screen(origin, 5.0).y < map.screen(origin, 2.0).y);
    }

    #[test]
    fn isometric_trace_visualizes_lift_before_travel_and_at_end() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(10.0, 10.0), Point::new(20.0, 10.0)], false)
            .unwrap();
        app.add_mark(vec![Point::new(45.0, 50.0), Point::new(60.0, 50.0)], false)
            .unwrap();
        app.rebuild();
        let mut moves = Vec::new();
        trace_segments(&app.planned, 6.25, 2.75, |segment| moves.push(segment));
        assert_eq!(
            moves.iter().map(|s| s.kind).collect::<Vec<_>>(),
            [
                MoveKind::Lower,
                MoveKind::Draw,
                MoveKind::Lift,
                MoveKind::Travel,
                MoveKind::Lower,
                MoveKind::Draw,
                MoveKind::Lift,
            ]
        );
        assert_eq!((moves[2].from.1, moves[2].to.1), (2.75, 6.25));
        assert_eq!((moves[3].from.1, moves[3].to.1), (6.25, 6.25));
        assert_eq!(moves.last().unwrap().to.1, 6.25);
    }

    #[test]
    fn k9_start_edit_invalidates_old_exports_and_round_trips_in_projects() {
        let mut app = Studio::default();
        app.apply_machine_preset(None);
        app.add_mark(vec![Point::new(10.0, 10.0), Point::new(25.0, 10.0)], false)
            .unwrap();
        app.rebuild();
        let off = app.export_commands(true).unwrap();
        assert!(!off.gcode.contains("M203"));
        app.inspect_gcode(true);
        let old_preview_revision = app.export_inspection.as_ref().unwrap().revision;

        app.profile.k9_start_gcode.as_mut().unwrap().enabled = true;
        app.invalidate();
        assert!(app.export_commands(true).is_err());
        assert_ne!(app.revision, old_preview_revision);
        app.rebuild();
        let on = app.export_commands(true).unwrap();
        assert!(on.gcode.contains("M203 X100 Y100"));
        assert_eq!(
            off.gcode
                .lines()
                .filter(|l| l.starts_with("G1"))
                .collect::<Vec<_>>(),
            on.gcode
                .lines()
                .filter(|l| l.starts_with("G1"))
                .collect::<Vec<_>>()
        );
        let encoded = app.as_project().to_json().unwrap();
        let decoded = Project::from_json(&encoded).unwrap();
        assert_eq!(decoded.profile.k9_start_gcode, app.profile.k9_start_gcode);

        app.profile.k9_start_gcode.as_mut().unwrap().code = "G91".into();
        app.invalidate();
        app.rebuild();
        assert!(app.export_commands(false).is_err() && app.export_commands(true).is_err());
        assert!(app.error.as_ref().unwrap().contains("K9 start G-code"));
    }

    #[test]
    fn export_requires_current_valid_route_without_a_manual_confirmation() {
        let mut app = Studio::default();
        assert!(app.air_first);
        assert!(!app.can_export());
        app.add_mark(vec![Point::new(10.0, 10.0), Point::new(25.0, 10.0)], false)
            .unwrap();
        app.rebuild();
        assert!(app.can_export());
        assert!(penplot::generate_air_run(&app.planned, &app.profile).is_ok());
        let old_report = calculate_plan(app.revision, &app.snapshot(), &app.profile);
        let (sender, receiver) = mpsc::channel();
        sender.send(old_report).unwrap();
        app.plan_task = Some(receiver);
        app.invalidate();
        app.poll_plan(&egui::Context::default());
        assert!(!app.can_export());
        assert!(
            app.planned.is_empty(),
            "stale route must not be shown or exported"
        );
    }

    #[test]
    fn loading_project_discards_history_and_waits_for_current_geometry() {
        let mut original = Studio::default();
        original
            .add_mark(vec![Point::new(10.0, 10.0), Point::new(42.0, 10.0)], false)
            .unwrap();
        original.rebuild();
        let project = Project::from_json(&original.as_project().to_json().unwrap()).unwrap();
        original.air_first = false;
        assert!(original.can_export());
        original.apply_project(PathBuf::from("example.penplot"), project);
        assert!(!original.project_dirty);
        assert!(original.preview_dirty);
        assert!(!original.can_export());
        assert!(original.air_first);
        assert!(original.undo.is_empty());
        assert_eq!(original.layers.len(), 1);
        original.rebuild();
        assert!(original.can_export());
    }

    #[test]
    fn placement_rotation_preserves_visible_artwork_and_uses_shift_snap() {
        let mut app = Studio::default();
        app.add_mark(
            vec![
                Point::new(10.0, 10.0),
                Point::new(35.0, 10.0),
                Point::new(10.0, 18.0),
            ],
            true,
        )
        .unwrap();
        let hidden_stroke = Stroke {
            points: vec![Point::new(-900.0, -900.0), Point::new(500.0, 500.0)],
            layer: "Hidden".into(),
            group: 0,
            closed: false,
            filled: false,
        };
        app.layers.push(Layer {
            name: "Hidden".into(),
            strokes: vec![hidden_stroke].into(),
            raster: None,
            text: None,
            visible: false,
        });
        app.invalidate();
        app.rebuild();
        let before = app.planned[0].points.clone();
        let bounds = app.placement_bounds().unwrap();
        assert_eq!(bounds[0], Point::new(10.0, 10.0));
        assert_eq!(bounds[2], Point::new(35.0, 18.0));
        let pivot = Point::new(22.5, 14.0);
        let mut drag = PlacementDrag {
            bounds,
            cursor_start: Point::new(22.5, 25.0),
            handle: PlacementHandle::Rotate(pivot),
            layer: 0,
            revision: app.revision,
            motion: PlacementMotion::Rotate(pivot, 0.0),
        };
        drag.update(Point::new(14.5, 22.0), true);
        let intended: Vec<_> = before.into_iter().map(|p| drag.motion.apply(p)).collect();
        assert!(matches!(drag.motion, PlacementMotion::Rotate(_, 45.0)));
        app.transform_layer(drag.layer, drag.motion).unwrap();
        assert_eq!(app.options.rotation_deg, 0.0);
        app.rebuild();
        for (point, expected) in app.planned[0].points.iter().zip(intended) {
            assert!((point.x - expected.x).hypot(point.y - expected.y) < 1e-8);
        }
    }

    #[test]
    fn transform_prioritizes_resize_handles_and_moves_from_inside() {
        let bounds = [
            Point::new(10., 20.),
            Point::new(60., 20.),
            Point::new(60., 70.),
            Point::new(10., 70.),
        ];
        let corners = bounds.map(|p| Pos2::new(p.x as f32, p.y as f32));
        assert!(matches!(
            placement_handle(Tool::Transform, Pos2::new(14., 23.), corners, bounds),
            Some(PlacementHandle::Scale(p)) if p == bounds[2]
        ));
        assert!(matches!(
            placement_handle(Tool::Transform, Pos2::new(35., 40.), corners, bounds),
            Some(PlacementHandle::Move)
        ));
        assert!(placement_handle(Tool::Transform, Pos2::new(0., 0.), corners, bounds).is_none());
        assert!(placement_handle(Tool::Pan, Pos2::new(35., 40.), corners, bounds).is_none());
        let tiny = corners.map(|p| Pos2::new(p.x * 0.1, p.y * 0.1));
        assert!(matches!(
            placement_handle(Tool::Transform, tiny[0].lerp(tiny[2], 0.5), tiny, bounds),
            Some(PlacementHandle::Move)
        ));
    }

    #[test]
    fn placement_scale_fixes_opposite_corner_and_keeps_positive_scale() {
        let pivot = Point::new(20.0, 20.0);
        let mut drag = PlacementDrag {
            bounds: [pivot; 4],
            cursor_start: Point::new(30.0, 30.0),
            handle: PlacementHandle::Scale(pivot),
            layer: 0,
            revision: 0,
            motion: PlacementMotion::Scale(pivot, 1.0),
        };
        drag.update(Point::new(40.0, 40.0), false);
        assert_eq!(drag.motion.apply(pivot), pivot);
        assert_eq!(drag.motion.apply(drag.cursor_start), Point::new(40.0, 40.0));
        drag.update(Point::new(5.0, 5.0), false);
        assert!(matches!(drag.motion, PlacementMotion::Scale(_, f) if f > 0.0));
        assert_eq!(drag.motion.apply(pivot), pivot);
    }

    #[test]
    fn active_placement_blocks_export_but_escape_does_not_dirty_job() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(10.0, 10.0), Point::new(22.0, 20.0)], false)
            .unwrap();
        app.rebuild();
        app.project_dirty = false;
        let revision = app.revision;
        let history = app.undo.len();
        let original = app.options.clone();
        let bounds = app.placement_bounds().unwrap();
        let mut drag = PlacementDrag {
            bounds,
            cursor_start: bounds[0],
            handle: PlacementHandle::Move,
            layer: 0,
            revision,
            motion: PlacementMotion::Move(Point::new(0.0, 0.0)),
        };
        drag.update(Point::new(bounds[0].x + 3.0, bounds[0].y + 5.0), true);
        assert_eq!(
            drag.motion.apply(bounds[0]),
            Point::new(bounds[0].x, bounds[0].y + 5.0)
        );
        app.placement_drag = Some(drag);
        assert!(!app.can_export());
        let ctx = egui::Context::default();
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: Key::Escape,
                    physical_key: Some(Key::Escape),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| app.update_ui(ctx),
        );
        assert!(app.placement_drag.is_none());
        assert!(app.can_export());
        assert_eq!(app.revision, revision);
        assert_eq!(app.options.x, original.x);
        assert_eq!(app.options.y, original.y);
        assert_eq!(app.undo.len(), history);
        assert!(!app.project_dirty);
    }

    #[test]
    fn one_pointer_move_drag_commits_one_undo_step() {
        let ctx = egui::Context::default();
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(30.0, 30.0), Point::new(65.0, 60.0)], false)
            .unwrap();
        app.rebuild();
        app.tools = Tool::Transform;
        let history = app.undo.len();
        let draw = |app: &mut Studio, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 820.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.canvas(ui));
                },
            )
        };
        let output = draw(&mut app, vec![]);
        let paper = output
            .shapes
            .iter()
            .find_map(|item| match &item.shape {
                egui::Shape::Rect(shape) if shape.fill == PAPER => Some(shape.rect),
                _ => None,
            })
            .unwrap();
        let from = paper.center();
        let to = from + egui::vec2(35.0, -26.0);
        let pointer = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        draw(
            &mut app,
            vec![egui::Event::PointerMoved(from), pointer(from, true)],
        );
        draw(
            &mut app,
            vec![egui::Event::PointerMoved(from.lerp(to, 0.5))],
        );
        assert!(app.placement_drag.is_some());
        assert_eq!(app.undo.len(), history);
        assert!(!app.can_export());
        draw(
            &mut app,
            vec![egui::Event::PointerMoved(to), pointer(to, false)],
        );
        assert!(app.placement_drag.is_none());
        assert_eq!(app.undo.len(), history + 1);
        assert!((app.options.x - (30.0 + 3500.0 / f64::from(paper.width()))).abs() < 0.001);
        assert!((app.options.y - (30.0 + 2600.0 / f64::from(paper.height()))).abs() < 0.001);
        assert!(!app.can_export());
        let moved_x = app.options.x;
        app.undo();
        assert_eq!((app.options.x, app.options.y), (30.0, 30.0));
        app.redo();
        assert_eq!(app.options.x, moved_x);
    }

    #[test]
    fn resize_one_layer_by_drag_leaves_other_layer_and_its_gcode_fixed() {
        let ctx = egui::Context::default();
        let mut app = Studio {
            profile: Profile::k9(),
            ..Studio::default()
        };
        app.options.optimize = false;
        app.add_mark(vec![Point::new(30., 30.), Point::new(50., 50.)], false)
            .unwrap();
        let mut other = app.layers[0].clone();
        other.name = "Other layer".into();
        let mut strokes = other.strokes.to_vec();
        strokes[0].points = vec![Point::new(70., 65.), Point::new(86., 72.)];
        other.strokes = strokes.into();
        app.layers.push(other);
        app.selected = Some(Selection::Layer(0));
        app.invalidate();
        app.rebuild();
        app.tools = Tool::Transform;
        let bounds_a = app.placement_bounds().unwrap();
        app.selected = Some(Selection::Layer(1));
        assert_eq!(app.placement_bounds().unwrap()[0], Point::new(70., 65.));
        app.selected = Some(Selection::Layer(0));
        assert_eq!(app.placement_bounds().unwrap(), bounds_a);
        let original = app.export_commands(false).unwrap().gcode.clone();
        let fixed = app.planned[1].points.clone();
        let fixed_command = "G1 X86.000 Y72.000 F1800.000";
        assert!(original.contains(fixed_command));
        let draw = |app: &mut Studio, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1100.0, 820.0))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.canvas(ui));
                },
            )
        };
        let frame = draw(&mut app, vec![]);
        let paper = frame
            .shapes
            .iter()
            .find_map(|item| match &item.shape {
                egui::Shape::Rect(shape) if shape.fill == PAPER => Some(shape.rect),
                _ => None,
            })
            .unwrap();
        let map = CanvasMap {
            anchor: paper.left_bottom(),
            scale: paper.width() / 100.,
        };
        let from = map.screen(bounds_a[2]);
        let to = map.screen(Point::new(60., 60.));
        let pointer = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        draw(
            &mut app,
            vec![egui::Event::PointerMoved(from), pointer(from, true)],
        );
        let dragging = draw(&mut app, vec![egui::Event::PointerMoved(to)]);
        let unchanged_line = |shapes: &[egui::epaint::ClippedShape]| {
            shapes.iter().any(|item| match &item.shape {
                egui::Shape::LineSegment { points, .. } => {
                    points[0].distance(map.screen(fixed[0])) < 0.01
                        && points[1].distance(map.screen(fixed[1])) < 0.01
                }
                _ => false,
            })
        };
        assert!(
            app.placement_drag
                .as_ref()
                .unwrap()
                .affects(&app.planned[0])
        );
        assert!(unchanged_line(&frame.shapes));
        assert!(unchanged_line(&dragging.shapes));
        draw(
            &mut app,
            vec![egui::Event::PointerMoved(to), pointer(to, false)],
        );
        app.rebuild();
        assert_eq!(app.planned[1].points, fixed);
        assert!(
            app.export_commands(false)
                .unwrap()
                .gcode
                .contains(fixed_command)
        );
        assert!(
            app.export_commands(false)
                .unwrap()
                .gcode
                .contains("G1 X60.000 Y60.000 F1800.000")
        );
        let project = Project::from_json(&app.as_project().to_json().unwrap()).unwrap();
        let mut reopened = Studio::default();
        reopened.apply_project(PathBuf::from("example.penplot"), project);
        reopened.rebuild();
        assert_eq!(
            reopened.export_commands(false).unwrap().gcode,
            app.export_commands(false).unwrap().gcode,
        );
        app.undo();
        app.rebuild();
        assert_eq!(app.export_commands(false).unwrap().gcode, original);
    }

    #[test]
    fn moving_selected_layer_past_visible_min_does_not_move_other_layer() {
        let mut app = Studio::default();
        app.options.scale = 1.25;
        app.options.rotation_deg = 22.5;
        app.options.optimize = false;
        app.add_mark(vec![Point::new(30., 30.), Point::new(40., 40.)], false)
            .unwrap();
        let mut other = app.layers[0].clone();
        other.name = "Other layer".into();
        let mut strokes = other.strokes.to_vec();
        for p in &mut strokes[0].points {
            p.x += 20.0;
        }
        other.strokes = strokes.into();
        app.layers.push(other);
        app.invalidate();
        app.rebuild();
        let fixed = app.planned[1].points.clone();
        let first = app.planned[0].points[0];
        let movement = PlacementMotion::Move(Point::new(-18., -12.));
        app.transform_layer(0, movement).unwrap();
        app.rebuild();
        assert!(app.planned[0].points[0].distance(movement.apply(first)) < 1e-8);
        for (actual, expected) in app.planned[1].points.iter().zip(fixed) {
            assert!(actual.distance(expected) < 1e-8);
        }
    }

    #[test]
    fn fit_uses_five_mm_margin_and_resets_camera() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(10.0, 10.0), Point::new(30.0, 20.0)], false)
            .unwrap();
        app.frame.zoom = 3.0;
        app.frame.shift = egui::vec2(20.0, -10.0);
        app.fit_and_reset_view();
        let bounds = app.placement_bounds().unwrap();
        assert_eq!(bounds[0], Point::new(5.0, 27.5));
        assert_eq!(bounds[2], Point::new(95.0, 72.5));
        assert_eq!(app.frame.zoom, 1.0);
        assert_eq!(app.frame.shift, Vec2::ZERO);
        assert!(!app.show_travel);
    }

    #[test]
    fn export_inspection_stays_on_the_saved_snapshot_after_edits() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(10.0, 10.0), Point::new(40.0, 30.0)], false)
            .unwrap();
        app.rebuild();
        let path = PathBuf::from("PLOT.GCODE");
        let output = app.export_commands(false).unwrap();
        assert!(Arc::ptr_eq(&output, app.output.as_ref().unwrap()));
        app.open_gcode_snapshot(Some(path.clone()), false, output);
        let exported = app.revision;
        app.options.x += 15.0;
        app.invalidate();
        assert!(app.export_commands(false).is_err());
        let current = app.export_inspection.as_ref().unwrap();
        assert_eq!(current.revision, exported);
        assert_ne!(current.revision, app.revision);
        assert_eq!(current.path.as_ref(), Some(&path));
        assert_eq!(current.title(), "Exported plot G-code");
        assert!(app.show_gcode);
        app.rebuild();
        app.inspect_gcode(true);
        let inspection = app.export_inspection.unwrap();
        assert_eq!(inspection.revision, app.revision);
        assert!(inspection.path.is_none());
        assert_eq!(inspection.title(), "Inspect air-run G-code");
    }

    #[test]
    fn missing_artwork_error_does_not_create_layer() {
        let path = Path::new("surely-not-a-real-penplot-artwork-274036.svg");
        let error = match import_one(path, &ImportOptions::default()) {
            Ok(_) => panic!("test path unexpectedly exists"),
            Err(error) => error,
        };
        assert!(error.contains("surely-not-a-real"));
    }

    #[test]
    fn canvas_coordinates_round_trip_and_preserve_y_up() {
        let map = CanvasMap {
            anchor: Pos2::new(100.0, 200.0),
            scale: 3.0,
        };
        let point = Point::new(22.0, -5.0);
        assert_eq!(map.screen(point), Pos2::new(166.0, 215.0));
        let actual = map.paper(map.screen(point));
        assert_eq!((actual.x, actual.y), (point.x, point.y));
    }

    #[test]
    fn tab_preview_and_air_export_share_current_curve_setting() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(5., 5.), Point::new(10., 10.)], false)
            .unwrap();
        app.add_mark(vec![Point::new(30., 10.), Point::new(35., 5.)], false)
            .unwrap();
        app.rebuild();
        assert!(app.export_commands(false).unwrap().curved_travel.is_empty());
        app.record_layers();
        app.options.smooth_pen_up = true;
        app.invalidate();
        assert!(app.export_commands(false).is_err());
        app.rebuild();
        let plot = app.export_commands(false).unwrap();
        let air = app.export_commands(true).unwrap();
        let curve = &plot.curved_travel[0];
        assert_eq!(curve.paper_points, air.curved_travel[0].paper_points);
        let mut points = Vec::new();
        trace_segments_with_curves(&app.planned, &plot.curved_travel, 5.0, 2.0, |segment| {
            if segment.kind == MoveKind::Travel {
                assert_eq!((segment.from.1, segment.to.1), (5.0, 5.0));
                points.push((segment.from.0, segment.to.0));
            }
        });
        assert_eq!(
            points,
            curve
                .paper_points
                .windows(2)
                .map(|p| (p[0], p[1]))
                .collect::<Vec<_>>()
        );
        app.undo();
        assert!(!app.options.smooth_pen_up);
    }

    #[test]
    fn editing_invalidates_a_plotted_result_and_can_undo_redo() {
        let mut app = Studio::default();
        app.add_mark(vec![Point::new(12.0, 10.0), Point::new(42.0, 10.0)], false)
            .unwrap();
        assert_eq!(app.layers.len(), 1);
        assert_eq!(app.options.x, 12.0);
        app.rebuild();
        assert_eq!(app.planned.len(), 1);
        app.undo();
        assert!(app.layers.is_empty());
        assert!(app.output.is_none());
        assert!(app.preview_dirty);
        app.redo();
        assert_eq!(app.layers.len(), 1);
        assert_eq!(app.options.x, 12.0);
    }

    #[test]
    fn drawing_outside_existing_bounds_does_not_shift_existing_stroke() {
        let mut app = Studio::default();
        app.options.rotation_deg = 45.0;
        app.options.scale = 1.75;
        app.add_mark(vec![Point::new(20.0, 20.0), Point::new(45.0, 20.0)], false)
            .unwrap();
        app.add_mark(vec![Point::new(5.0, 10.0), Point::new(10.0, 15.0)], false)
            .unwrap();
        app.options.optimize = false;
        app.rebuild();
        assert_eq!(app.planned.len(), 2);
        assert!((app.planned[0].points[0].x - 20.0).abs() < 1e-9);
        assert!((app.planned[0].points[0].y - 20.0).abs() < 1e-9);
    }

    #[test]
    fn shape_tools_generate_closed_usable_contours() {
        let points = [Point::new(25.0, 37.0), Point::new(9.0, 7.0)];
        let (outline, closed) = draft_geometry(Tool::Rectangle, &points).unwrap();
        assert!(closed);
        assert_eq!(outline.len(), 4);
        let (outline, closed) = draft_geometry(Tool::Ellipse, &points).unwrap();
        assert!(closed);
        assert!(outline.len() >= 16);
        assert!((outline[0].x - 25.0).abs() < 0.001);
        assert!((outline[0].y - 22.0).abs() < 0.001);
        assert!(draft_geometry(Tool::Ellipse, &points[..1]).is_none());
    }
}
