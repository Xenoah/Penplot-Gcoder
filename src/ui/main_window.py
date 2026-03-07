"""Main application window."""
from __future__ import annotations
import os
from typing import List, Optional

import numpy as np

from PyQt6.QtCore import Qt
from PyQt6.QtGui import QAction, QKeySequence, QUndoStack, QUndoCommand
from PyQt6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QPushButton, QLabel, QStatusBar, QFileDialog,
    QMessageBox, QSplitter, QDoubleSpinBox, QFrame,
)

from src.models.settings import AppSettings
from src.models.pen_path import PathGroup
from src.ui.settings_panel import SettingsPanel
from src.ui.preview_2d import Preview2D
from src.ui.preview_3d import Preview3D
from src.ui.path_list import PathListPanel
from src.core.bed_calculator import paths_overflow
from src.core.path_optimizer import optimize
from src.core.fill_generator import generate_fills_for_paths
from src.core.gcode_generator import generate_gcode


# ---------------------------------------------------------------------------
# Undo Commands
# ---------------------------------------------------------------------------
class PlacementCommand(QUndoCommand):
    """Undo/Redo for path placement (scale, offset, rotation)."""

    def __init__(self, window: "MainWindow",
                 old_scale, old_ox, old_oy, old_rot,
                 new_scale, new_ox, new_oy, new_rot):
        super().__init__("Change Placement")
        self._win = window
        self._old = (old_scale, old_ox, old_oy, old_rot)
        self._new = (new_scale, new_ox, new_oy, new_rot)

    def _apply(self, values):
        s, ox, oy, rot = values
        self._win.settings.path.scale = s
        self._win.settings.path.offset_x = ox
        self._win.settings.path.offset_y = oy
        self._win.settings.path.rotation = rot
        self._win._prev_placement = values
        self._win._settings_panel._refresh_from_settings()
        self._win._refresh_display()

    def undo(self):
        self._apply(self._old)

    def redo(self):
        self._apply(self._new)


# ---------------------------------------------------------------------------
# Main Window
# ---------------------------------------------------------------------------
class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.settings = AppSettings()
        self._source_groups: List[PathGroup] = []
        self._display_groups: List[PathGroup] = []
        self._source_file: str = ""
        self._overflow = False
        self._preview_mode = "2D"
        self._applying_placement = False   # guard against signal loops

        self._undo_stack = QUndoStack(self)
        self._prev_placement = self._snapshot_placement()

        self.setWindowTitle("Penplot-Gcoder")
        self.resize(1280, 780)

        self._build_ui()
        self._build_menu()
        self._update_status()

    # ------------------------------------------------------------------ UI build
    def _build_ui(self):
        central = QWidget()
        self.setCentralWidget(central)
        root = QHBoxLayout(central)
        root.setContentsMargins(4, 4, 4, 4)
        root.setSpacing(4)

        # Left: settings panel
        self._settings_panel = SettingsPanel(self.settings)
        self._settings_panel.settings_changed.connect(self._on_settings_changed)
        self._settings_panel.center_clicked.connect(self._on_center_clicked)
        root.addWidget(self._settings_panel)

        # Centre + Right splitter
        splitter = QSplitter(Qt.Orientation.Horizontal)
        root.addWidget(splitter)

        preview_container = QWidget()
        pcl = QVBoxLayout(preview_container)
        pcl.setContentsMargins(0, 0, 0, 0)
        pcl.setSpacing(2)

        # ── Top toolbar: 2D/3D + Animate ─────────────────────────────────────
        tb = QHBoxLayout()
        self._btn_2d   = QPushButton("2D")
        self._btn_3d   = QPushButton("3D")
        self._btn_anim = QPushButton("▶ Animate")
        self._btn_2d.setCheckable(True)
        self._btn_3d.setCheckable(True)
        self._btn_2d.setChecked(True)
        self._btn_2d.clicked.connect(lambda: self._switch_preview("2D"))
        self._btn_3d.clicked.connect(lambda: self._switch_preview("3D"))
        self._btn_anim.clicked.connect(self._toggle_animation)
        tb.addWidget(self._btn_2d)
        tb.addWidget(self._btn_3d)
        tb.addWidget(self._btn_anim)
        tb.addStretch()

        # Cursor coordinate display (top-right of toolbar)
        self._coord_label = QLabel("X: --.-  Y: --.-")
        self._coord_label.setStyleSheet("color: #aaaaaa; font-family: monospace;")
        tb.addWidget(self._coord_label)
        pcl.addLayout(tb)

        # ── Absolute-coordinate / transform input bar ─────────────────────────
        abs_bar = QHBoxLayout()
        abs_bar.setSpacing(6)

        def _dspin(lo, hi, dec=2, suffix=""):
            w = QDoubleSpinBox()
            w.setRange(lo, hi)
            w.setDecimals(dec)
            w.setSuffix(suffix)
            w.setFixedWidth(90)
            return w

        abs_bar.addWidget(QLabel("X:"))
        self._abs_x = _dspin(-9999, 9999, 2, " mm")
        abs_bar.addWidget(self._abs_x)

        abs_bar.addWidget(QLabel("Y:"))
        self._abs_y = _dspin(-9999, 9999, 2, " mm")
        abs_bar.addWidget(self._abs_y)

        sep = QFrame(); sep.setFrameShape(QFrame.Shape.VLine)
        abs_bar.addWidget(sep)

        abs_bar.addWidget(QLabel("W:"))
        self._abs_w = QLabel("--")
        self._abs_w.setFixedWidth(70)
        abs_bar.addWidget(self._abs_w)

        abs_bar.addWidget(QLabel("H:"))
        self._abs_h = QLabel("--")
        self._abs_h.setFixedWidth(70)
        abs_bar.addWidget(self._abs_h)

        sep2 = QFrame(); sep2.setFrameShape(QFrame.Shape.VLine)
        abs_bar.addWidget(sep2)

        abs_bar.addWidget(QLabel("Scale:"))
        self._abs_scale = _dspin(0.1, 10000, 1, " %")
        abs_bar.addWidget(self._abs_scale)

        abs_bar.addStretch()
        pcl.addLayout(abs_bar)

        # Connect abs bar → settings (block reverse update during init)
        self._abs_x.editingFinished.connect(self._on_abs_bar_changed)
        self._abs_y.editingFinished.connect(self._on_abs_bar_changed)
        self._abs_scale.editingFinished.connect(self._on_abs_bar_changed)

        # ── Previews ─────────────────────────────────────────────────────────
        self._preview_2d = Preview2D(self.settings)
        self._preview_3d = Preview3D(self.settings)
        self._preview_3d.setVisible(False)

        # Connect 2D preview signals
        self._preview_2d.cursor_moved.connect(self._on_cursor_moved)
        self._preview_2d.placement_changed.connect(self._on_preview_placement_changed)

        pcl.addWidget(self._preview_2d)
        pcl.addWidget(self._preview_3d)

        # Info bar
        self._info_bar = QLabel()
        self._info_bar.setStyleSheet("padding: 2px 6px;")
        pcl.addWidget(self._info_bar)

        # Bottom toolbar
        btn_row = QHBoxLayout()
        self._open_btn  = QPushButton("Open File...")
        self._gcode_btn = QPushButton("Generate G-code")
        self._save_btn  = QPushButton("Save G-code...")
        self._gcode_text: Optional[str] = None

        self._open_btn.clicked.connect(self._on_open)
        self._gcode_btn.clicked.connect(self._on_generate_gcode)
        self._save_btn.clicked.connect(self._on_save_gcode)
        self._save_btn.setEnabled(False)

        btn_row.addWidget(self._open_btn)
        btn_row.addWidget(self._gcode_btn)
        btn_row.addWidget(self._save_btn)
        pcl.addLayout(btn_row)

        splitter.addWidget(preview_container)

        # Right: path list
        self._path_list = PathListPanel()
        self._path_list.order_changed.connect(self._on_path_order_changed)
        self._path_list.pen_change_changed.connect(self._refresh_display)
        splitter.addWidget(self._path_list)

        splitter.setStretchFactor(0, 4)
        splitter.setStretchFactor(1, 1)

        # Status bar
        self._status = QStatusBar()
        self.setStatusBar(self._status)

        # Init abs bar
        self._sync_abs_bar()

    def _build_menu(self):
        mb = self.menuBar()

        file_menu = mb.addMenu("File")
        act_open = QAction("Open...", self)
        act_open.setShortcut(QKeySequence.StandardKey.Open)
        act_open.triggered.connect(self._on_open)
        file_menu.addAction(act_open)

        act_save = QAction("Save G-code...", self)
        act_save.setShortcut(QKeySequence.StandardKey.Save)
        act_save.triggered.connect(self._on_save_gcode)
        file_menu.addAction(act_save)

        act_save_settings = QAction("Save Settings...", self)
        act_save_settings.triggered.connect(self._on_save_settings)
        file_menu.addAction(act_save_settings)

        act_load_settings = QAction("Load Settings...", self)
        act_load_settings.triggered.connect(self._on_load_settings)
        file_menu.addAction(act_load_settings)

        file_menu.addSeparator()
        act_quit = QAction("Quit", self)
        act_quit.setShortcut("Ctrl+Q")
        act_quit.triggered.connect(self.close)
        file_menu.addAction(act_quit)

        edit_menu = mb.addMenu("Edit")
        act_undo = self._undo_stack.createUndoAction(self, "Undo")
        act_undo.setShortcut(QKeySequence.StandardKey.Undo)
        act_redo = self._undo_stack.createRedoAction(self, "Redo")
        act_redo.setShortcut(QKeySequence.StandardKey.Redo)
        edit_menu.addAction(act_undo)
        edit_menu.addAction(act_redo)

        view_menu = mb.addMenu("View")
        act_2d = QAction("2D View", self)
        act_2d.triggered.connect(lambda: self._switch_preview("2D"))
        act_3d = QAction("3D View", self)
        act_3d.triggered.connect(lambda: self._switch_preview("3D"))
        view_menu.addAction(act_2d)
        view_menu.addAction(act_3d)

        help_menu = mb.addMenu("Help")
        act_about = QAction("About", self)
        act_about.triggered.connect(self._on_about)
        help_menu.addAction(act_about)

    # ------------------------------------------------------------------ slots

    def _switch_preview(self, mode: str):
        self._preview_mode = mode
        self._btn_2d.setChecked(mode == "2D")
        self._btn_3d.setChecked(mode == "3D")
        self._preview_2d.setVisible(mode == "2D")
        self._preview_3d.setVisible(mode == "3D")

    def _toggle_animation(self):
        if self._preview_mode == "2D":
            self._preview_2d.start_animation()
        else:
            self._preview_3d.start_animation()

    # ── File loading ─────────────────────────────────────────────────────────

    def _on_open(self):
        path, _ = QFileDialog.getOpenFileName(
            self, "Open File", "",
            "Vector/Image Files (*.svg *.dxf *.png *.jpg *.jpeg);;All Files (*)"
        )
        if not path:
            return
        self._load_file(path)

    def _load_file(self, path: str):
        ext = os.path.splitext(path)[1].lower()
        precision = self.settings.path.curve_precision
        try:
            if ext == ".svg":
                from src.core.importer.svg_importer import import_svg
                groups = import_svg(path, precision)
            elif ext == ".dxf":
                from src.core.importer.dxf_importer import import_dxf
                groups = import_dxf(path, precision)
            elif ext in (".png", ".jpg", ".jpeg"):
                from src.core.importer.image_importer import import_image
                groups = import_image(path)
            else:
                QMessageBox.warning(self, "Unsupported", f"Unsupported file type: {ext}")
                return
        except Exception as e:
            QMessageBox.critical(self, "Error", f"Failed to load file:\n{e}")
            return

        self._source_file = path
        self._source_groups = groups
        self._path_list.set_groups(list(groups))
        self.setWindowTitle(f"Penplot-Gcoder — {os.path.basename(path)}")

        # Auto-fit: scale and center to effective drawing area
        self._auto_fit_to_bed(groups)
        self._refresh_display()

    def _auto_fit_to_bed(self, groups: List[PathGroup]):
        """Scale + center raw paths to 90 % of effective drawing area."""
        all_pts = []
        for g in groups:
            for p in g.paths:
                all_pts.extend(p.points)
        if not all_pts:
            return

        pts = np.array(all_pts)
        src_w = float(pts[:, 0].max() - pts[:, 0].min())
        src_h = float(pts[:, 1].max() - pts[:, 1].min())
        if src_w <= 0 and src_h <= 0:
            return

        x_min, y_min, x_max, y_max = self.settings.effective_area()
        eff_w = x_max - x_min
        eff_h = y_max - y_min

        # Compute scale to fit within 90 % of effective area
        scale_x = (eff_w / src_w * 0.9) if src_w > 0 else 1.0
        scale_y = (eff_h / src_h * 0.9) if src_h > 0 else 1.0
        fit_scale = min(scale_x, scale_y) * 100.0   # as percent

        # Compute offset so the path center lands on the area center
        src_cx = float((pts[:, 0].min() + pts[:, 0].max()) / 2)
        src_cy = float((pts[:, 1].min() + pts[:, 1].max()) / 2)
        tgt_cx = (x_min + x_max) / 2
        tgt_cy = (y_min + y_max) / 2

        self.settings.path.scale    = fit_scale
        self.settings.path.offset_x = tgt_cx - src_cx * fit_scale / 100.0
        self.settings.path.offset_y = tgt_cy - src_cy * fit_scale / 100.0
        self.settings.path.rotation = 0.0
        self._prev_placement = self._snapshot_placement()
        self._settings_panel._refresh_from_settings()
        self._sync_abs_bar()

    # ── Display refresh ──────────────────────────────────────────────────────

    def _refresh_display(self):
        if not self._source_groups:
            self._display_groups = []
            self._overflow = False
            self._update_preview()
            self._update_status()
            return

        pa = self.settings.path
        fi = self.settings.fill
        ordered_source = (self._path_list.current_groups()
                          if self._path_list.groups else self._source_groups)

        all_transformed: List[PathGroup] = []
        for group in ordered_source:
            new_paths = []
            for p in group.paths:
                tp = p.transformed(
                    scale=pa.scale / 100.0,
                    offset_x=pa.offset_x,
                    offset_y=pa.offset_y,
                    angle_deg=pa.rotation,
                )
                new_paths.append(tp)

            fill_paths = generate_fills_for_paths(new_paths, fi) if fi.enabled else []

            if fi.layer_order == "outline_first":
                combined = new_paths + fill_paths
            elif fi.layer_order == "fill_first":
                combined = fill_paths + new_paths
            else:
                combined = fill_paths

            if pa.optimize and combined:
                combined = optimize(combined, pa.optimize_algorithm, pa.join_distance)

            from src.models.pen_path import PathGroup as PG
            all_transformed.append(PG(
                color=group.color,
                label=group.label,
                paths=combined,
                pen_change_before=group.pen_change_before,
            ))

        self._display_groups = all_transformed
        all_paths = [p for g in self._display_groups for p in g.paths]
        self._overflow = paths_overflow(all_paths, self.settings)

        self._update_preview()
        self._update_status()
        self._gcode_btn.setEnabled(not self._overflow)
        self._sync_abs_bar()

    def _update_preview(self):
        self._preview_2d.set_groups(self._display_groups)
        self._preview_2d.set_overflow(self._overflow)
        self._preview_3d.set_groups(self._display_groups)

    def _update_status(self):
        from src.core.gcode_generator import _estimate_time
        all_paths = [p for g in self._display_groups for p in g.paths]
        n = len(all_paths)
        if all_paths:
            dummy_groups = [PathGroup(paths=all_paths)]
            total_mm, est_sec = _estimate_time(dummy_groups, self.settings)
            m, s = divmod(int(est_sec), 60)
            time_str  = f"{m}m{s:02d}s"
            total_str = f"{total_mm:.0f}mm"
        else:
            time_str = total_str = "—"

        eff = self.settings.effective_area()
        w = eff[2] - eff[0]
        h = eff[3] - eff[1]
        info = (f"Effective area: {w:.1f}×{h:.1f}mm  |  "
                f"Paths: {n}  |  Est. time: {time_str}  |  Draw: {total_str}")
        if self._overflow:
            info += "  ⚠ OVERFLOW"
        self._info_bar.setText(info)
        self._status.showMessage(info)

    # ── Absolute-coordinate bar ───────────────────────────────────────────────

    def _sync_abs_bar(self):
        """Update the absolute-coordinate bar from current settings + bbox."""
        if self._applying_placement:
            return
        pa = self.settings.path
        self._abs_x.blockSignals(True)
        self._abs_y.blockSignals(True)
        self._abs_scale.blockSignals(True)

        self._abs_x.setValue(pa.offset_x)
        self._abs_y.setValue(pa.offset_y)
        self._abs_scale.setValue(pa.scale)

        # Compute W / H from display bbox
        bbox = self._preview_2d._compute_bbox()
        if bbox:
            x0, y0, x1, y1 = bbox
            self._abs_w.setText(f"{x1 - x0:.2f} mm")
            self._abs_h.setText(f"{y1 - y0:.2f} mm")
        else:
            self._abs_w.setText("--")
            self._abs_h.setText("--")

        self._abs_x.blockSignals(False)
        self._abs_y.blockSignals(False)
        self._abs_scale.blockSignals(False)

    def _on_abs_bar_changed(self):
        """User typed an exact value in the abs bar."""
        if self._applying_placement:
            return
        old = self._snapshot_placement()
        self.settings.path.offset_x = self._abs_x.value()
        self.settings.path.offset_y = self._abs_y.value()
        self.settings.path.scale    = self._abs_scale.value()
        new = self._snapshot_placement()
        if new != old:
            self._push_placement_undo(old, new)
        self._settings_panel._refresh_from_settings()
        self._refresh_display()

    # ── Preview signals ───────────────────────────────────────────────────────

    def _on_cursor_moved(self, x: float, y: float):
        self._coord_label.setText(f"X: {x:7.2f}  Y: {y:7.2f} mm")

    def _on_preview_placement_changed(self, ox: float, oy: float, scale: float):
        """Live drag update from the 2D preview."""
        if self._applying_placement:
            return
        self._applying_placement = True
        old = self._snapshot_placement()
        self.settings.path.offset_x = ox
        self.settings.path.offset_y = oy
        self.settings.path.scale    = scale
        self._applying_placement = False

        self._settings_panel._refresh_from_settings()
        self._refresh_display()
        new = self._snapshot_placement()
        if new != old:
            self._push_placement_undo(old, new)

    # ── Settings-panel change ─────────────────────────────────────────────────

    def _on_settings_changed(self):
        if self._applying_placement:
            return
        new_placement = self._snapshot_placement()
        if new_placement != self._prev_placement:
            old = self._prev_placement
            self._prev_placement = new_placement
            self._push_placement_undo(old, new_placement)
        self._refresh_display()

    def _push_placement_undo(self, old, new):
        self._prev_placement = new
        cmd = PlacementCommand(self, old[0], old[1], old[2], old[3],
                               new[0], new[1], new[2], new[3])
        self._undo_stack.blockSignals(True)
        self._undo_stack.push(cmd)
        self._undo_stack.blockSignals(False)

    def _snapshot_placement(self):
        pa = self.settings.path
        return (pa.scale, pa.offset_x, pa.offset_y, pa.rotation)

    def _on_center_clicked(self):
        if not self._display_groups:
            return
        all_paths = [p for g in self._display_groups for p in g.paths]
        if not all_paths:
            return

        pts_list = [p.np_points for p in all_paths if len(p.np_points) > 0]
        if not pts_list:
            return
        all_pts  = np.concatenate(pts_list)
        draw_cx  = float((all_pts[:, 0].min() + all_pts[:, 0].max()) / 2)
        draw_cy  = float((all_pts[:, 1].min() + all_pts[:, 1].max()) / 2)

        x_min, y_min, x_max, y_max = self.settings.effective_area()
        area_cx = (x_min + x_max) / 2
        area_cy = (y_min + y_max) / 2

        old = self._snapshot_placement()
        self.settings.path.offset_x += area_cx - draw_cx
        self.settings.path.offset_y += area_cy - draw_cy
        new = self._snapshot_placement()
        self._push_placement_undo(old, new)
        self._settings_panel._refresh_from_settings()
        self._refresh_display()

    def _on_path_order_changed(self):
        self._refresh_display()

    # ── G-code ───────────────────────────────────────────────────────────────

    def _on_generate_gcode(self):
        if self._overflow:
            QMessageBox.warning(self, "Overflow",
                                "Path overflows effective area. Cannot generate G-code.")
            return
        src = os.path.basename(self._source_file) if self._source_file else ""
        try:
            self._gcode_text = generate_gcode(self._display_groups, self.settings, src)
        except Exception as e:
            QMessageBox.critical(self, "Error", f"G-code generation failed:\n{e}")
            return
        self._save_btn.setEnabled(True)
        QMessageBox.information(
            self, "G-code ready",
            f"G-code generated ({len(self._gcode_text)} chars).\n"
            "Click 'Save G-code...' to save."
        )

    def _on_save_gcode(self):
        if not self._gcode_text:
            self._on_generate_gcode()
        if not self._gcode_text:
            return
        default = (os.path.splitext(self._source_file)[0] + ".gcode"
                   if self._source_file else "output.gcode")
        path, _ = QFileDialog.getSaveFileName(
            self, "Save G-code", default,
            "G-code Files (*.gcode *.nc *.txt);;All Files (*)"
        )
        if not path:
            return
        with open(path, "w", encoding="utf-8") as f:
            f.write(self._gcode_text)
        self._status.showMessage(f"Saved: {path}")

    def _on_save_settings(self):
        path, _ = QFileDialog.getSaveFileName(
            self, "Save Settings", "", "JSON Files (*.json);;All Files (*)")
        if path:
            self.settings.to_json(path)
            self._status.showMessage(f"Settings saved: {path}")

    def _on_load_settings(self):
        path, _ = QFileDialog.getOpenFileName(
            self, "Load Settings", "", "JSON Files (*.json);;All Files (*)")
        if path:
            try:
                self.settings = AppSettings.from_json(path)
                self._settings_panel.settings = self.settings
                self._settings_panel._refresh_from_settings()
                self._preview_2d.settings = self.settings
                self._preview_3d.settings = self.settings
                self._on_settings_changed()
                self._status.showMessage(f"Settings loaded: {path}")
            except Exception as e:
                QMessageBox.critical(self, "Error", f"Failed to load settings:\n{e}")

    def _on_about(self):
        QMessageBox.about(
            self, "About Penplot-Gcoder",
            "Penplot-Gcoder\n\n"
            "Convert SVG/DXF/PNG to G-code for pen plotters\n"
            "based on 3D printers.\n\n"
            "Built with Python, PyQt6, svgpathtools, ezdxf,\n"
            "OpenCV, Shapely and pyqtgraph."
        )
