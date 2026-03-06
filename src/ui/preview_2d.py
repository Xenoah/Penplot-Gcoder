"""2D preview widget using QPainter."""
from __future__ import annotations
import math
from typing import List, Optional

from PyQt6.QtCore import Qt, QPointF, QRectF, QTimer
from PyQt6.QtGui import (QPainter, QPen, QColor, QBrush, QFont,
                          QPainterPath, QTransform, QWheelEvent)
from PyQt6.QtWidgets import QWidget, QSizePolicy

from src.models.pen_path import PenPath, PathGroup
from src.models.settings import AppSettings


class Preview2D(QWidget):
    """2D preview of pen paths with zoom / pan and overflow detection."""

    def __init__(self, settings: AppSettings, parent=None):
        super().__init__(parent)
        self.settings = settings
        self.groups: List[PathGroup] = []
        self._overflow = False

        # Animation state
        self._anim_timer = QTimer(self)
        self._anim_timer.timeout.connect(self._anim_step)
        self._anim_total_paths: List[PenPath] = []
        self._anim_index = 0
        self._anim_drawn: List[PenPath] = []

        # View transform
        self._scale = 1.0
        self._offset = QPointF(0, 0)
        self._drag_start: Optional[QPointF] = None
        self._drag_offset_start: Optional[QPointF] = None

        self.setMinimumSize(300, 300)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Expanding)
        self.setMouseTracking(True)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)

    # ------------------------------------------------------------------ public
    def set_groups(self, groups: List[PathGroup]):
        self.groups = groups
        self._overflow = self._check_overflow()
        self.update()

    def set_overflow(self, overflow: bool):
        self._overflow = overflow
        self.update()

    def start_animation(self):
        self._anim_drawn = []
        self._anim_total_paths = [p for g in self.groups for p in g.paths]
        self._anim_index = 0
        self._anim_timer.start(30)

    def stop_animation(self):
        self._anim_timer.stop()
        self._anim_drawn = []
        self.update()

    def fit_view(self):
        """Fit the bed into the widget."""
        bx = self.settings.machine.bed_x
        by = self.settings.machine.bed_y
        if bx <= 0 or by <= 0:
            return
        margin = 20
        sx = (self.width() - 2 * margin) / bx
        sy = (self.height() - 2 * margin) / by
        self._scale = min(sx, sy)
        self._offset = QPointF(
            margin + (self.width() - 2 * margin - bx * self._scale) / 2,
            margin + (self.height() - 2 * margin - by * self._scale) / 2,
        )
        self.update()

    # ------------------------------------------------------------------ internal
    def _check_overflow(self) -> bool:
        x_min, y_min, x_max, y_max = self.settings.effective_area()
        for g in self.groups:
            for p in g.paths:
                for x, y in p.points:
                    if x < x_min - 1e-4 or x > x_max + 1e-4 or \
                       y < y_min - 1e-4 or y > y_max + 1e-4:
                        return True
        return False

    def _anim_step(self):
        if self._anim_index < len(self._anim_total_paths):
            self._anim_drawn.append(self._anim_total_paths[self._anim_index])
            self._anim_index += 1
        else:
            self._anim_timer.stop()
        self.update()

    def _world_to_screen(self, x: float, y: float) -> QPointF:
        """Convert world (mm) coords to screen pixels. Y is flipped."""
        bx = self.settings.machine.bed_x
        by = self.settings.machine.bed_y
        sx = x * self._scale + self._offset.x()
        sy = (by - y) * self._scale + self._offset.y()
        return QPointF(sx, sy)

    # ------------------------------------------------------------------ Qt events
    def showEvent(self, _):
        self.fit_view()

    def resizeEvent(self, _):
        self.fit_view()

    def paintEvent(self, _):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.fillRect(self.rect(), QColor("#2b2b2b"))

        bx = self.settings.machine.bed_x
        by = self.settings.machine.bed_y
        x_min, y_min, x_max, y_max = self.settings.effective_area()

        # Bed rectangle (grey)
        pen = QPen(QColor("#888888"), 1.5)
        painter.setPen(pen)
        painter.setBrush(QColor("#1e1e1e"))
        tl = self._world_to_screen(0, by)
        br = self._world_to_screen(bx, 0)
        painter.drawRect(QRectF(tl, br))

        # Effective area (green)
        pen = QPen(QColor("#44aa44"), 1.5, Qt.PenStyle.DashLine)
        painter.setPen(pen)
        painter.setBrush(Qt.BrushStyle.NoBrush)
        tl2 = self._world_to_screen(x_min, y_max)
        br2 = self._world_to_screen(x_max, y_min)
        painter.drawRect(QRectF(tl2, br2))

        # Grid (subtle)
        self._draw_grid(painter, bx, by)

        # Paths
        anim_mode = self._anim_timer.isActive() or bool(self._anim_drawn)
        draw_list = self._anim_drawn if anim_mode else [p for g in self.groups for p in g.paths]
        prev_end: Optional[tuple] = None
        for path in draw_list:
            if not path.points:
                continue
            # Travel line to path start
            if prev_end is not None:
                travel_pen = QPen(QColor("#ff4444"), 1.0, Qt.PenStyle.DotLine)
                painter.setPen(travel_pen)
                p1 = self._world_to_screen(*prev_end)
                p2 = self._world_to_screen(*path.points[0])
                painter.drawLine(p1, p2)

            # Draw path
            draw_pen = QPen(QColor("#4488ff"), 1.5)
            painter.setPen(draw_pen)
            pts = [self._world_to_screen(*pt) for pt in path.points]
            for i in range(1, len(pts)):
                painter.drawLine(pts[i - 1], pts[i])
            prev_end = path.points[-1]

        # Overflow warning banner
        if self._overflow:
            painter.setFont(QFont("Arial", 12, QFont.Weight.Bold))
            painter.fillRect(0, 0, self.width(), 32, QColor(200, 50, 50, 200))
            painter.setPen(Qt.GlobalColor.white)
            painter.drawText(8, 22, "WARNING: Path overflows effective drawing area!")

        painter.end()

    def _draw_grid(self, painter: QPainter, bx: float, by: float):
        grid_pen = QPen(QColor("#333333"), 0.5)
        painter.setPen(grid_pen)
        step = 10.0
        x = 0.0
        while x <= bx:
            p1 = self._world_to_screen(x, 0)
            p2 = self._world_to_screen(x, by)
            painter.drawLine(p1, p2)
            x += step
        y = 0.0
        while y <= by:
            p1 = self._world_to_screen(0, y)
            p2 = self._world_to_screen(bx, y)
            painter.drawLine(p1, p2)
            y += step

    def wheelEvent(self, event: QWheelEvent):
        delta = event.angleDelta().y()
        factor = 1.15 if delta > 0 else 1.0 / 1.15
        mouse = QPointF(event.position())
        # Zoom around mouse position
        self._offset = mouse - (mouse - self._offset) * factor
        self._scale *= factor
        self.update()

    def mousePressEvent(self, event):
        if event.button() == Qt.MouseButton.LeftButton:
            self._drag_start = QPointF(event.position())
            self._drag_offset_start = QPointF(self._offset)

    def mouseMoveEvent(self, event):
        if self._drag_start is not None:
            delta = QPointF(event.position()) - self._drag_start
            self._offset = self._drag_offset_start + delta
            self.update()

    def mouseReleaseEvent(self, event):
        if event.button() == Qt.MouseButton.LeftButton:
            self._drag_start = None
