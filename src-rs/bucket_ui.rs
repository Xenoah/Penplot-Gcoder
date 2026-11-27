//! Paint-bucket editing. Stored region boundaries and physical pattern settings
//! feed the same planner for the monochrome preview, project reload and export.
use super::*;
use penplot::{Pattern, PatternSettings};

pub(super) fn controls(ui: &mut egui::Ui, value: &mut PatternSettings, japanese: bool) {
    let title = |en, ja| if japanese { ja } else { en };
    for (pattern, en, ja) in [
        (Pattern::Dots, "Polka dots", "水玉"),
        (Pattern::Diagonal, "Diagonal lines", "斜線"),
        (Pattern::Cross, "Cross-hatching", "クロスハッチング"),
    ] {
        ui.selectable_value(&mut value.pattern, pattern, title(en, ja));
    }
    ui.label(title("Density (%)", "濃度・密度 (%)"));
    ui.add(egui::Slider::new(&mut value.density, 1.0..=100.));
    if value.pattern != Pattern::Dots {
        ui.horizontal(|ui| {
            ui.label(title("Angle", "角度"));
            ui.add(
                egui::DragValue::new(&mut value.angle_deg)
                    .range(-180.0..=180.)
                    .suffix("°"),
            );
        });
    }
    ui.checkbox(
        &mut value.clip_to_pen,
        title("Ignore parts thinner than pen", "ペン幅より細い部分を省く"),
    );
    ui.horizontal(|ui| {
        ui.label(title("Pen width", "ペン幅"));
        ui.add(
            egui::DragValue::new(&mut value.pen_width_mm)
                .speed(0.01)
                .range(0.05..=3.)
                .suffix(" mm"),
        );
    });
    value.dot_diameter_mm = value.dot_diameter_mm.max(value.pen_width_mm).max(0.1);
    if value.pattern == Pattern::Dots {
        ui.horizontal(|ui| {
            ui.label(title("Dot diameter", "水玉の直径"));
            ui.add(
                egui::DragValue::new(&mut value.dot_diameter_mm)
                    .speed(0.05)
                    .range(value.pen_width_mm.max(0.1)..=10.)
                    .suffix(" mm"),
            );
        });
    }
    ui.small(title("Higher density makes the actual pattern darker. Pen width controls the ink estimate; paper and pen change the real tone.", "密度を上げるほど実際の模様が濃くなります。ペン幅を合わせて確認してください。実物の濃さはペンと紙でも変わります。"));
}

impl Studio {
    pub(super) fn bucket_panel(&mut self, ui: &mut egui::Ui) {
        section(
            ui,
            if self.japanese {
                "次に塗る領域"
            } else {
                "NEXT BUCKET FILL"
            },
        );
        controls(ui, &mut self.bucket_defaults, self.japanese);
        ui.small(if self.japanese {
            "閉じた領域の内側をクリック。塗りは独立レイヤーになり、右側で再編集できます。"
        } else {
            "Click inside a closed region. Each fill is a separate layer, editable on the right."
        });
    }

    pub(super) fn bucket_at(&mut self, paper: Point) -> Result<(), String> {
        self.bucket_defaults.validate()?;
        let (sin, cos) = self
            .options
            .rotation_deg
            .rem_euclid(360.)
            .to_radians()
            .sin_cos();
        let scale = self.options.scale;
        if !scale.is_finite() || scale <= 0. {
            return Err("Correct the artwork scale first".into());
        }
        let min = projected_min(&self.layers, scale, sin, cos)
            .ok_or("Import linework or draw a closed shape first")?;
        let x = (paper.x - self.options.x + min.x) / scale;
        let y = (paper.y - self.options.y + min.y) / scale;
        let raw = Point::new(x * cos + y * sin, -x * sin + y * cos);
        // Repainting an existing region updates it instead of stacking duplicates.
        if let Some(index) = self.layers.iter().rposition(|l| {
            l.visible
                && l.raster.as_ref().is_some_and(|r| r.pattern.is_some())
                && penplot::region_contains(&l.strokes, raw)
        }) {
            self.selected = Some(Selection::Layer(index));
            if self.layers[index].raster.as_ref().unwrap().pattern.as_ref()
                != Some(&self.bucket_defaults)
            {
                self.record_layers();
                self.layers[index].raster.as_mut().unwrap().pattern =
                    Some(self.bucket_defaults.clone());
                self.invalidate();
            }
            self.grayscale_preview = true;
            self.status = "Updated this region’s pattern. Change its settings on the right.".into();
            return Ok(());
        }
        if self.layers.len() >= MAX_LAYER_COUNT {
            return Err("Layer limit reached; remove a layer first".into());
        }
        let source: Vec<_> = self
            .layers
            .iter()
            .filter(|l| l.visible && l.raster.as_ref().is_none_or(|r| r.pattern.is_none()))
            .flat_map(|l| l.strokes.iter().cloned())
            .collect();
        let region = penplot::bucket_region(&source, raw)?;
        let (paths, points) = self.geometry_size();
        if paths + region.len() > penplot::MAX_STROKES
            || points + region.iter().map(|s| s.points.len()).sum::<usize>() > penplot::MAX_POINTS
        {
            return Err("Workspace geometry limit reached".into());
        }
        let settings = penplot::RasterSettings {
            pattern: Some(self.bucket_defaults.clone()),
            outline: false,
            fill: Fill::Lines,
            ..Default::default()
        };
        // Validate density/geometry before committing this undoable edit.
        let mut isolated = Vec::new();
        penplot::append_layer(&mut isolated, &region, 0)?;
        let checked = penplot::plan_with_raster(
            &isolated,
            &JobOptions {
                scale,
                rotation_deg: self.options.rotation_deg,
                ..Default::default()
            },
            &[(0, settings.clone())].into_iter().collect(),
        )?;
        if checked.strokes.is_empty() {
            return Err(
                "No pattern fits here. Increase density or reduce dot diameter / pen width.".into(),
            );
        }
        self.record_layers();
        self.layers.push(Layer {
            name: format!(
                "Bucket fill {}",
                self.layers
                    .iter()
                    .filter(|l| l.raster.as_ref().is_some_and(|r| r.pattern.is_some()))
                    .count()
                    + 1
            ),
            strokes: region.into(),
            visible: true,
            raster: Some(settings),
            text: None,
        });
        if let Some(after) = projected_min(&self.layers, scale, sin, cos) {
            self.options.x += after.x - min.x;
            self.options.y += after.y - min.y;
        }
        self.selected = Some(Selection::Layer(self.layers.len() - 1));
        self.grayscale_preview = true;
        self.invalidate();
        self.status =
            "Bucket fill added. Preview shows the same pattern that will be pen-plotted.".into();
        Ok(())
    }

    pub(super) fn bucket_canvas(&mut self, response: &egui::Response, map: CanvasMap) {
        if response.clicked()
            && self.text_dialog.is_none()
            && self.import_task.is_none()
            && !self.raster_import.is_busy()
            && self.pending_open.is_none()
            && let Some(at) = response.interact_pointer_pos()
            && let Err(message) = self.bucket_at(map.paper(at))
        {
            self.status = message;
        }
    }

    pub(super) fn pattern_for_route(&self, stroke: &Stroke) -> Option<&PatternSettings> {
        let index = stroke.layer.split_once(':')?.0.parse::<usize>().ok()?;
        self.layers.get(index)?.raster.as_ref()?.pattern.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn box_points(x: f64, y: f64, w: f64, h: f64) -> Vec<Point> {
        vec![
            Point::new(x, y),
            Point::new(x + w, y),
            Point::new(x + w, y + h),
            Point::new(x, y + h),
            Point::new(x, y),
        ]
    }
    #[test]
    fn svg_buckets_at_paper_origin_remain_exportable_after_each_click() {
        let mut app = Studio::default();
        let art = penplot::import_bytes(
            "bucket-card.svg",
            include_bytes!("../examples/bucket-card.svg"),
            &ImportOptions::default(),
        )
        .unwrap();
        app.layers.push(Layer {
            name: "bucket-card.svg".into(),
            strokes: art.strokes.into(),
            visible: true,
            raster: None,
            text: None,
        });
        for (pattern, density, x) in [
            (Pattern::Dots, 35., 12.),
            (Pattern::Diagonal, 50., 42.),
            (Pattern::Cross, 70., 72.),
        ] {
            app.bucket_defaults.pattern = pattern;
            app.bucket_defaults.density = density;
            app.bucket_at(Point::new(x, 20.)).unwrap();
            app.rebuild();
            assert!(app.output.is_some(), "{:?}; {:?}", app.error, app.options);
        }
        let before = app.planned.clone();
        let p = Project::from_json(&app.as_project().to_json().unwrap()).unwrap();
        app.apply_project(PathBuf::from("roundtrip.penplot"), p);
        app.rebuild();
        assert_eq!(app.planned, before);
        assert!(app.output.is_some());
    }
    #[test]
    fn bucket_edit_repaint_undo_save_and_move_keep_other_layers_fixed() {
        let mut app = Studio::default();
        app.add_mark(box_points(10., 10., 40., 30.), true).unwrap();
        app.add_mark(box_points(20., 20., 10., 10.), true).unwrap();
        app.rebuild();
        let original = app.layers[0].strokes.clone();
        app.bucket_defaults.pattern = Pattern::Dots;
        app.bucket_at(Point::new(15., 15.)).unwrap();
        app.rebuild();
        assert!(app.grayscale_preview);
        assert_eq!(app.layers.len(), 2);
        assert_eq!(app.layers[0].strokes.as_ref(), original.as_ref());
        assert_eq!(app.layers[1].strokes.len(), 2);
        let initial = app.as_project();
        let before = app.planned.clone();
        app.bucket_defaults.pattern = Pattern::Cross;
        app.bucket_defaults.density = 70.;
        app.bucket_at(Point::new(15., 15.)).unwrap();
        app.rebuild();
        assert_eq!(app.layers.len(), 2);
        assert_ne!(app.planned, before);
        app.undo();
        app.rebuild();
        assert_eq!(app.as_project(), initial);
        assert_eq!(app.planned, before);
        app.redo();
        app.rebuild();
        let project = Project::from_json(&app.as_project().to_json().unwrap()).unwrap();
        let routes = app.planned.clone();
        app.apply_project(PathBuf::from("bucket.penplot"), project);
        app.rebuild();
        assert_eq!(app.planned, routes);
        assert!(app.grayscale_preview);
        let other = app.layers[0].strokes.clone();
        app.finish_layer_placement(
            1,
            PlacementMotion::Move(Point::new(2., 3.)),
            &egui::Context::default(),
        );
        app.rebuild();
        assert_eq!(app.layers[0].strokes.as_ref(), other.as_ref());
        app.delete_object(Selection::Layer(1));
        app.rebuild();
        assert_eq!(app.layers.len(), 1);
        assert_eq!(app.layers[0].strokes.as_ref(), original.as_ref());
    }
}
