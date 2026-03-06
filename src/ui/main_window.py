"""Main application window."""
from __future__ import annotations
import os
from typing import List, Optional

from PyQt6.QtCore import Qt
from PyQt6.QtGui import QAction, QKeySequence, QUndoStack, QUndoCommand
from PyQt6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QPushButton, QLabel, QStatusBar, QFileDialog,
    QMessageBox, QSplitter,
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
        self._source_groups: List[PathGroup] = []   # raw imported groups
        self._display_groups: List[PathGroup] = []  # after transform / fill / optimize
        self._source_file: str = ""
        self._overflow = False
        self._preview_mode = "2D"

        # Undo stack
        self._undo_stack = QUndoStack(self)

        # Track previous placement for undo
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

        # Centre + Right: preview + path list
        splitter = QSplitter(Qt.Orientation.Horizontal)
        root.addWidget(splitter)

        # Centre: preview stack
        preview_container = QWidget()
        pcl = QVBoxLayout(preview_container)
        pcl.setContentsMargins(0, 0, 0, 0)

        # Toolbar above preview
        tb = QHBoxLayout()
        self._btn_2d = QPushButton("2D")
        self._btn_3d = QPushButton("3D")
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
        pcl.addLayout(tb)

        # 2D / 3D previews
        self._preview_2d = Preview2D(self.settings)
        self._preview_3d = Preview3D(self.settings)
        self._preview_3d.setVisible(False)

        pcl.addWidget(self._preview_2d)
        pcl.addWidget(self._preview_3d)

        # Status bar under preview (overflow + stats)
        self._info_bar = QLabel()
        self._info_bar.setStyleSheet("padding: 2px 6px;")
        pcl.addWidget(self._info_bar)

        # Bottom toolbar
        btn_row = QHBoxLayout()
        self._open_btn = QPushButton("Open File...")
        self._gcode_btn = QPushButton("Generate G-code")
        self._save_btn = QPushButton("Save G-code...")
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

    def _build_menu(self):
        mb = self.menuBar()

        # File
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

        # Edit
        edit_menu = mb.addMenu("Edit")
        act_undo = self._undo_stack.createUndoAction(self, "Undo")
        act_undo.setShortcut(QKeySequence.StandardKey.Undo)
        act_redo = self._undo_stack.createRedoAction(self, "Redo")
        act_redo.setShortcut(QKeySequence.StandardKey.Redo)
        edit_menu.addAction(act_undo)
        edit_menu.addAction(act_redo)

        # View
        view_menu = mb.addMenu("View")
        act_2d = QAction("2D View", self)
        act_2d.triggered.connect(lambda: self._switch_preview("2D"))
        act_3d = QAction("3D View", self)
        act_3d.triggered.connect(lambda: self._switch_preview("3D"))
        view_menu.addAction(act_2d)
        view_menu.addAction(act_3d)

        # Help
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
        # Populate path list with source groups so user can reorder them
        self._path_list.set_groups(list(groups))
        self.setWindowTitle(f"Penplot-Gcoder — {os.path.basename(path)}")
        self._refresh_display()

    def _refresh_display(self):
        """Apply transforms, fill, optimize to source groups and update previews."""
        if not self._source_groups:
            self._display_groups = []
            self._overflow = False
            self._update_preview()
            self._update_status()
            return

        pa = self.settings.path
        fi = self.settings.fill

        # Use path list order (which tracks source groups in user-reordered order)
        ordered_source = self._path_list.current_groups() if self._path_list.groups else self._source_groups

        # Transform all source paths
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

            # Fill generation
            fill_paths = generate_fills_for_paths(new_paths, fi) if fi.enabled else []

            # Determine layering
            if fi.layer_order == "outline_first":
                combined = new_paths + fill_paths
            elif fi.layer_order == "fill_first":
                combined = fill_paths + new_paths
            else:
                combined = fill_paths

            # Optimization
            if pa.optimize and combined:
                combined = optimize(combined, pa.optimize_algorithm, pa.join_distance)

            from src.models.pen_path import PathGroup as PG
            new_group = PG(
                color=group.color,
                label=group.label,
                paths=combined,
                pen_change_before=group.pen_change_before,
            )
            all_transformed.append(new_group)

        self._display_groups = all_transformed

        # Check overflow
        all_paths = [p for g in self._display_groups for p in g.paths]
        self._overflow = paths_overflow(all_paths, self.settings)

        self._update_preview()
        self._update_status()
        self._gcode_btn.setEnabled(not self._overflow)

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
            time_str = f"{m}m{s:02d}s"
            total_str = f"{total_mm:.0f}mm"
        else:
            time_str = "0m00s"
            total_str = "0mm"

        eff = self.settings.effective_area()
        w = eff[2] - eff[0]
        h = eff[3] - eff[1]
        info = f"Effective area: {w:.1f}×{h:.1f}mm  |  Paths: {n}  |  Est. time: {time_str}  |  Draw: {total_str}"
        if self._overflow:
            info += "  ⚠ OVERFLOW"
        self._info_bar.setText(info)
        self._status.showMessage(info)

    def _on_settings_changed(self):
        """Called whenever settings panel emits settings_changed."""
        new_placement = self._snapshot_placement()
        if new_placement != self._prev_placement and not self._undo_stack.isActive():
            old = self._prev_placement
            self._prev_placement = new_placement
            cmd = PlacementCommand(
                self,
                old[0], old[1], old[2], old[3],
                new_placement[0], new_placement[1], new_placement[2], new_placement[3],
            )
            # Push without triggering redo (which would recurse)
            self._undo_stack.blockSignals(True)
            self._undo_stack.push(cmd)
            self._undo_stack.blockSignals(False)
        self._refresh_display()

    def _snapshot_placement(self):
        pa = self.settings.path
        return (pa.scale, pa.offset_x, pa.offset_y, pa.rotation)

    def _on_center_clicked(self):
        """Center the drawing on the effective drawing area."""
        if not self._display_groups:
            return
        all_paths = [p for g in self._display_groups for p in g.paths]
        if not all_paths:
            return

        import numpy as np
        pts_list = [p.np_points for p in all_paths if len(p.np_points) > 0]
        if not pts_list:
            return
        all_pts = np.concatenate(pts_list)
        bmin = all_pts.min(axis=0)
        bmax = all_pts.max(axis=0)
        draw_cx = (bmin[0] + bmax[0]) / 2
        draw_cy = (bmin[1] + bmax[1]) / 2

        x_min, y_min, x_max, y_max = self.settings.effective_area()
        area_cx = (x_min + x_max) / 2
        area_cy = (y_min + y_max) / 2

        pa = self.settings.path
        old_placement = self._snapshot_placement()
        pa.offset_x += area_cx - draw_cx
        pa.offset_y += area_cy - draw_cy

        new_placement = self._snapshot_placement()
        self._prev_placement = new_placement
        cmd = PlacementCommand(self, *old_placement, *new_placement)
        self._undo_stack.blockSignals(True)
        self._undo_stack.push(cmd)
        self._undo_stack.blockSignals(False)
        self._settings_panel._refresh_from_settings()
        self._refresh_display()

    def _on_path_order_changed(self):
        self._refresh_display()

    def _on_generate_gcode(self):
        if self._overflow:
            QMessageBox.warning(self, "Overflow", "Path overflows effective area. Cannot generate G-code.")
            return
        src = os.path.basename(self._source_file) if self._source_file else ""
        try:
            self._gcode_text = generate_gcode(self._display_groups, self.settings, src)
        except Exception as e:
            QMessageBox.critical(self, "Error", f"G-code generation failed:\n{e}")
            return
        self._save_btn.setEnabled(True)
        QMessageBox.information(self, "G-code ready",
                                f"G-code generated successfully ({len(self._gcode_text)} chars).\n"
                                "Click 'Save G-code...' to save.")

    def _on_save_gcode(self):
        if not self._gcode_text:
            self._on_generate_gcode()
        if not self._gcode_text:
            return
        default = os.path.splitext(self._source_file)[0] + ".gcode" if self._source_file else "output.gcode"
        path, _ = QFileDialog.getSaveFileName(
            self, "Save G-code", default, "G-code Files (*.gcode *.nc *.txt);;All Files (*)"
        )
        if not path:
            return
        with open(path, "w", encoding="utf-8") as f:
            f.write(self._gcode_text)
        self._status.showMessage(f"Saved: {path}")

    def _on_save_settings(self):
        path, _ = QFileDialog.getSaveFileName(
            self, "Save Settings", "", "JSON Files (*.json);;All Files (*)"
        )
        if path:
            self.settings.to_json(path)
            self._status.showMessage(f"Settings saved: {path}")

    def _on_load_settings(self):
        path, _ = QFileDialog.getOpenFileName(
            self, "Load Settings", "", "JSON Files (*.json);;All Files (*)"
        )
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
