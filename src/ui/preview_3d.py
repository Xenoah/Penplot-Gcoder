"""3D preview widget using pyqtgraph GLViewWidget."""
from __future__ import annotations
from typing import List, Optional

import numpy as np
from PyQt6.QtCore import Qt, QTimer
from PyQt6.QtWidgets import QWidget, QVBoxLayout, QHBoxLayout, QLabel, QSlider

try:
    import pyqtgraph.opengl as gl
    from pyqtgraph.opengl import GLViewWidget
    PYQTGRAPH_GL_OK = True
except ImportError:
    PYQTGRAPH_GL_OK = False

from src.models.pen_path import PenPath, PathGroup
from src.models.settings import AppSettings


class Preview3D(QWidget):
    """3D preview of pen paths using pyqtgraph OpenGL."""

    def __init__(self, settings: AppSettings, parent=None):
        super().__init__(parent)
        self.settings = settings
        self.groups: List[PathGroup] = []
        self._z_exag = 10.0
        self._items: List = []  # GL items added to view

        # Animation
        self._anim_timer = QTimer(self)
        self._anim_timer.timeout.connect(self._anim_step)
        self._anim_all_paths: List[PenPath] = []
        self._anim_index = 0

        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)

        if not PYQTGRAPH_GL_OK:
            layout.addWidget(QLabel("pyqtgraph / PyOpenGL not available.\n"
                                    "Install: pip install pyqtgraph PyOpenGL"))
            return

        self._view = GLViewWidget()
        self._view.setBackgroundColor("#1e1e1e")
        self._view.setCameraPosition(distance=400, elevation=30, azimuth=-60)
        layout.addWidget(self._view)

        # Z exaggeration slider
        slider_row = QHBoxLayout()
        slider_row.addWidget(QLabel("Z exag:"))
        self._z_slider = QSlider(Qt.Orientation.Horizontal)
        self._z_slider.setRange(1, 50)
        self._z_slider.setValue(10)
        self._z_slider.valueChanged.connect(self._on_z_exag_changed)
        self._z_label = QLabel("10×")
        slider_row.addWidget(self._z_slider)
        slider_row.addWidget(self._z_label)
        layout.addLayout(slider_row)

        self._draw_bed()

    # ------------------------------------------------------------------ public
    def set_groups(self, groups: List[PathGroup]):
        self.groups = groups
        self._rebuild()

    def set_draw_limit(self, n: int):
        """Show only the first n paths (−1 = all). Used by seekbar."""
        all_paths = [p for g in self.groups for p in g.paths]
        limit = n if n >= 0 else len(all_paths)
        self._clear_path_items()
        self._add_path_items(all_paths[:limit])

    def start_animation(self):
        self._clear_path_items()
        self._anim_all_paths = [p for g in self.groups for p in g.paths]
        self._anim_index = 0
        self._anim_timer.start(40)

    def stop_animation(self):
        self._anim_timer.stop()
        self._rebuild()

    def reset_camera(self):
        if PYQTGRAPH_GL_OK and hasattr(self, "_view"):
            self._view.setCameraPosition(distance=400, elevation=30, azimuth=-60)

    # ------------------------------------------------------------------ keyboard
    def keyPressEvent(self, event):
        if not PYQTGRAPH_GL_OK or not hasattr(self, "_view"):
            return
        key = event.key()
        if key == Qt.Key.Key_R:
            self.reset_camera()
        elif key == Qt.Key.Key_7:  # Numpad 7 — top
            self._view.setCameraPosition(elevation=90, azimuth=0)
        elif key == Qt.Key.Key_1:  # Numpad 1 — front
            self._view.setCameraPosition(elevation=0, azimuth=0)
        elif key == Qt.Key.Key_3:  # Numpad 3 — side
            self._view.setCameraPosition(elevation=0, azimuth=-90)
        else:
            super().keyPressEvent(event)

    # ------------------------------------------------------------------ internal
    def _on_z_exag_changed(self, val):
        self._z_exag = float(val)
        self._z_label.setText(f"{val}×")
        self._rebuild()

    def _draw_bed(self):
        if not PYQTGRAPH_GL_OK or not hasattr(self, "_view"):
            return
        bx = self.settings.machine.bed_x
        by = self.settings.machine.bed_y
        step = 20.0
        lines = []
        x = 0.0
        while x <= bx:
            lines.append(np.array([[x, 0, 0], [x, by, 0]]))
            x += step
        y = 0.0
        while y <= by:
            lines.append(np.array([[0, y, 0], [bx, y, 0]]))
            y += step
        for seg in lines:
            item = gl.GLLinePlotItem(
                pos=seg.astype(np.float32),
                color=(0.4, 0.4, 0.4, 0.4),
                width=1,
                mode="lines",
            )
            self._view.addItem(item)

        # Effective area outline
        x_min, y_min, x_max, y_max = self.settings.effective_area()
        ea_pts = np.array([
            [x_min, y_min, 0], [x_max, y_min, 0],
            [x_max, y_max, 0], [x_min, y_max, 0],
            [x_min, y_min, 0],
        ], dtype=np.float32)
        ea_item = gl.GLLinePlotItem(pos=ea_pts, color=(0.2, 0.8, 0.2, 1.0), width=2)
        self._view.addItem(ea_item)

    def _clear_path_items(self):
        if not PYQTGRAPH_GL_OK or not hasattr(self, "_view"):
            return
        for item in self._items:
            self._view.removeItem(item)
        self._items = []

    def _rebuild(self):
        self._clear_path_items()
        if not PYQTGRAPH_GL_OK or not hasattr(self, "_view"):
            return
        all_paths = [p for g in self.groups for p in g.paths]
        self._add_path_items(all_paths)

    def _add_path_items(self, paths: List[PenPath]):
        if not PYQTGRAPH_GL_OK or not hasattr(self, "_view"):
            return
        puz = self.settings.pen.pen_up_z * self._z_exag
        pdz = self.settings.pen.pen_down_z * self._z_exag
        ox = self.settings.pen.offset_x
        oy = self.settings.pen.offset_y

        prev_end = None
        for path in paths:
            if not path.points:
                continue
            # Travel line
            start = path.points[0]
            if prev_end is not None:
                travel_pts = np.array([
                    [prev_end[0] + ox, prev_end[1] + oy, puz],
                    [start[0] + ox, start[1] + oy, puz],
                ], dtype=np.float32)
                item = gl.GLLinePlotItem(
                    pos=travel_pts, color=(1.0, 0.3, 0.3, 0.8),
                    width=1, mode="lines",
                )
                self._view.addItem(item)
                self._items.append(item)

            # Pen down line (green vertical)
            sx, sy = start[0] + ox, start[1] + oy
            pen_dn = np.array([[sx, sy, puz], [sx, sy, pdz]], dtype=np.float32)
            dn_item = gl.GLLinePlotItem(pos=pen_dn, color=(0.2, 0.9, 0.2, 1.0),
                                         width=2, mode="lines")
            self._view.addItem(dn_item)
            self._items.append(dn_item)

            # Draw path
            pts3d = np.array(
                [[x + ox, y + oy, pdz] for x, y in path.points],
                dtype=np.float32
            )
            draw_item = gl.GLLinePlotItem(
                pos=pts3d, color=(0.3, 0.5, 1.0, 1.0),
                width=2, mode="line_strip",
            )
            self._view.addItem(draw_item)
            self._items.append(draw_item)

            # Pen up line (orange vertical)
            ex, ey = path.points[-1][0] + ox, path.points[-1][1] + oy
            pen_up = np.array([[ex, ey, pdz], [ex, ey, puz]], dtype=np.float32)
            up_item = gl.GLLinePlotItem(pos=pen_up, color=(1.0, 0.6, 0.0, 1.0),
                                         width=2, mode="lines")
            self._view.addItem(up_item)
            self._items.append(up_item)

            prev_end = path.points[-1]

    def _anim_step(self):
        if not PYQTGRAPH_GL_OK or not hasattr(self, "_view"):
            self._anim_timer.stop()
            return
        if self._anim_index < len(self._anim_all_paths):
            path = self._anim_all_paths[self._anim_index]
            self._add_path_items([path])
            self._anim_index += 1
        else:
            self._anim_timer.stop()
