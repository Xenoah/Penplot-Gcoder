"""2D preview widget using QPainter — with interactive move/scale and cursor coords."""
from __future__ import annotations
import math
from typing import List, Optional, Tuple

from PyQt6.QtCore import Qt, QPointF, QRectF, QTimer, pyqtSignal
from PyQt6.QtGui import (QPainter, QPen, QColor, QFont, QWheelEvent, QCursor)
from PyQt6.QtWidgets import QWidget, QSizePolicy

from src.models.pen_path import PenPath, PathGroup
from src.models.settings import AppSettings

# ── constants ─────────────────────────────────────────────────────────────────
_HANDLE_PX   = 9      # handle square half-size in screen pixels
_HANDLE_HALF = _HANDLE_PX / 2

# Interaction state names
_IDLE  = "idle"
_PAN   = "pan"
_MOVE  = "move"
_SCALE = "scale"


class Preview2D(QWidget):
    """2D preview: zoom/pan, interactive move and scale of loaded paths."""

    # Emitted on mouse move: (x_mm, y_mm) in world (bed) coordinates
    cursor_moved = pyqtSignal(float, float)
    # Emitted when the user finishes dragging (move or scale):
    # (offset_x_mm, offset_y_mm, scale_pct)
    placement_changed = pyqtSignal(float, float, float)

    def __init__(self, settings: AppSettings, parent=None):
        super().__init__(parent)
        self.settings = settings
        self.groups: List[PathGroup] = []
        self._overflow = False

        # Bounding box of current display groups in world (mm) coords
        self._bbox: Optional[Tuple[float, float, float, float]] = None  # x0,y0,x1,y1

        # Animation
        self._anim_timer = QTimer(self)
        self._anim_timer.timeout.connect(self._anim_step)
        self._anim_total_paths: List[PenPath] = []
        self._anim_index = 0
        self._anim_drawn: List[PenPath] = []

        # View transform (zoom/pan)
        self._vscale = 1.0
        self._voffset = QPointF(0, 0)

        # Interaction state
        self._interact = _IDLE
        self._drag_screen_start = QPointF()
        self._drag_voffset_start = QPointF()   # for pan
        self._drag_world_start   = (0.0, 0.0)  # for move
        self._drag_offset_start  = (0.0, 0.0)  # settings.path.offset at drag start
        self._drag_scale_start   = 100.0        # settings.path.scale at drag start
        self._drag_anchor_world  = (0.0, 0.0)  # fixed corner for scale
        self._drag_anchor_dist   = 1.0          # initial dist (pixels) for scale
        # live delta applied during drag before emitting (for visual feedback)
        self._live_dx = 0.0   # world mm
        self._live_dy = 0.0
        self._live_ds = 1.0   # scale multiplier

        self.setMinimumSize(300, 300)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Expanding)
        self.setMouseTracking(True)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)

    # ═══════════════════════════════════════════════════ public API

    def set_groups(self, groups: List[PathGroup]):
        self.groups = groups
        self._bbox = self._compute_bbox()
        self._overflow = self._check_overflow()
        self._live_dx = self._live_dy = 0.0
        self._live_ds = 1.0
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
        """Fit the entire bed into the widget with a small margin."""
        bx = self.settings.machine.bed_x
        by = self.settings.machine.bed_y
        if bx <= 0 or by <= 0:
            return
        margin = 20
        sx = (self.width()  - 2 * margin) / bx
        sy = (self.height() - 2 * margin) / by
        self._vscale = min(sx, sy)
        self._voffset = QPointF(
            margin + (self.width()  - 2 * margin - bx * self._vscale) / 2,
            margin + (self.height() - 2 * margin - by * self._vscale) / 2,
        )
        self.update()

    # ═══════════════════════════════════════════════════ coordinate helpers

    def _world_to_screen(self, x: float, y: float) -> QPointF:
        by = self.settings.machine.bed_y
        sx = x * self._vscale + self._voffset.x()
        sy = (by - y) * self._vscale + self._voffset.y()
        return QPointF(sx, sy)

    def _screen_to_world(self, sx: float, sy: float) -> Tuple[float, float]:
        by = self.settings.machine.bed_y
        x = (sx - self._voffset.x()) / self._vscale
        y = by - (sy - self._voffset.y()) / self._vscale
        return x, y

    # ═══════════════════════════════════════════════════ bbox helpers

    def _compute_bbox(self) -> Optional[Tuple[float, float, float, float]]:
        xs, ys = [], []
        for g in self.groups:
            for p in g.paths:
                for x, y in p.points:
                    xs.append(x); ys.append(y)
        if not xs:
            return None
        return min(xs), min(ys), max(xs), max(ys)

    def _check_overflow(self) -> bool:
        x_min, y_min, x_max, y_max = self.settings.effective_area()
        for g in self.groups:
            for p in g.paths:
                for x, y in p.points:
                    if x < x_min - 1e-4 or x > x_max + 1e-4 or \
                       y < y_min - 1e-4 or y > y_max + 1e-4:
                        return True
        return False

    def _handle_rects_screen(self) -> List[Tuple[QRectF, int, int]]:
        """
        Returns [(screen_rect, corner_wx_sign, corner_wy_sign), ...]
        for each of the 4 corner handles.
        corner_wx_sign: -1 = left, +1 = right
        corner_wy_sign: -1 = bottom, +1 = top
        """
        if not self._bbox:
            return []
        x0, y0, x1, y1 = self._bbox
        corners = [
            (x0, y0, -1, -1), (x1, y0,  1, -1),
            (x0, y1, -1,  1), (x1, y1,  1,  1),
        ]
        result = []
        for wx, wy, sx, sy in corners:
            sc = self._world_to_screen(wx, wy)
            r = QRectF(sc.x() - _HANDLE_HALF, sc.y() - _HANDLE_HALF,
                       _HANDLE_PX, _HANDLE_PX)
            result.append((r, sx, sy))
        return result

    def _hit_handle(self, pos: QPointF) -> Optional[Tuple[int, int]]:
        for rect, sx, sy in self._handle_rects_screen():
            if rect.contains(pos):
                return sx, sy
        return None

    def _inside_bbox(self, pos: QPointF) -> bool:
        if not self._bbox:
            return False
        x0, y0, x1, y1 = self._bbox
        tl = self._world_to_screen(x0, y1)
        br = self._world_to_screen(x1, y0)
        return QRectF(tl, br).contains(pos)

    # ═══════════════════════════════════════════════════ animation

    def _anim_step(self):
        if self._anim_index < len(self._anim_total_paths):
            self._anim_drawn.append(self._anim_total_paths[self._anim_index])
            self._anim_index += 1
        else:
            self._anim_timer.stop()
        self.update()

    # ═══════════════════════════════════════════════════ Qt events

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

        # ── Bed rectangle (grey) ─────────────────────────────────────────────
        painter.setPen(QPen(QColor("#888888"), 1.5))
        painter.setBrush(QColor("#1e1e1e"))
        painter.drawRect(QRectF(self._world_to_screen(0, by),
                                self._world_to_screen(bx, 0)))

        # ── Effective area (green dashed) ────────────────────────────────────
        painter.setPen(QPen(QColor("#44aa44"), 1.5, Qt.PenStyle.DashLine))
        painter.setBrush(Qt.BrushStyle.NoBrush)
        painter.drawRect(QRectF(self._world_to_screen(x_min, y_max),
                                self._world_to_screen(x_max, y_min)))

        # ── Grid ─────────────────────────────────────────────────────────────
        self._draw_grid(painter, bx, by)

        # ── Paths ─────────────────────────────────────────────────────────────
        anim_mode = self._anim_timer.isActive() or bool(self._anim_drawn)
        draw_list = self._anim_drawn if anim_mode else [p for g in self.groups for p in g.paths]
        prev_end: Optional[tuple] = None
        for path in draw_list:
            if not path.points:
                continue
            # Travel line
            if prev_end is not None:
                painter.setPen(QPen(QColor("#ff4444"), 1.0, Qt.PenStyle.DotLine))
                painter.drawLine(self._world_to_screen(*prev_end),
                                 self._world_to_screen(*path.points[0]))
            # Draw stroke
            painter.setPen(QPen(QColor("#4488ff"), 1.5))
            pts = [self._world_to_screen(*pt) for pt in path.points]
            for i in range(1, len(pts)):
                painter.drawLine(pts[i - 1], pts[i])
            prev_end = path.points[-1]

        # ── Selection bbox + handles ─────────────────────────────────────────
        if self._bbox and self.groups:
            self._draw_selection(painter)

        # ── Overflow warning ─────────────────────────────────────────────────
        if self._overflow:
            painter.setFont(QFont("Arial", 12, QFont.Weight.Bold))
            painter.fillRect(0, 0, self.width(), 32, QColor(200, 50, 50, 200))
            painter.setPen(Qt.GlobalColor.white)
            painter.drawText(8, 22, "WARNING: Path overflows effective drawing area!")

        painter.end()

    def _draw_selection(self, painter: QPainter):
        x0, y0, x1, y1 = self._bbox
        tl = self._world_to_screen(x0, y1)
        br = self._world_to_screen(x1, y0)
        # Dashed white selection rectangle
        pen = QPen(QColor("#ffffff"), 1, Qt.PenStyle.DashLine)
        pen.setDashPattern([4, 4])
        painter.setPen(pen)
        painter.setBrush(Qt.BrushStyle.NoBrush)
        painter.drawRect(QRectF(tl, br))
        # Corner handles
        painter.setPen(QPen(QColor("#ffffff"), 1))
        painter.setBrush(QColor("#4488ff"))
        for rect, _, _ in self._handle_rects_screen():
            painter.drawRect(rect)

    def _draw_grid(self, painter: QPainter, bx: float, by: float):
        painter.setPen(QPen(QColor("#333333"), 0.5))
        step = 10.0
        x = 0.0
        while x <= bx:
            painter.drawLine(self._world_to_screen(x, 0),
                             self._world_to_screen(x, by))
            x += step
        y = 0.0
        while y <= by:
            painter.drawLine(self._world_to_screen(0, y),
                             self._world_to_screen(bx, y))
            y += step

    # ── Mouse / keyboard ─────────────────────────────────────────────────────

    def wheelEvent(self, event: QWheelEvent):
        delta = event.angleDelta().y()
        factor = 1.15 if delta > 0 else 1.0 / 1.15
        mouse = QPointF(event.position())
        self._voffset = mouse - (mouse - self._voffset) * factor
        self._vscale *= factor
        self.update()

    def mousePressEvent(self, event):
        pos = QPointF(event.position())
        if event.button() == Qt.MouseButton.LeftButton:
            if self._bbox and self.groups:
                # Check corner handles first
                hit = self._hit_handle(pos)
                if hit is not None:
                    self._begin_scale(pos, hit)
                    return
                # Check inside bbox → move
                if self._inside_bbox(pos):
                    self._begin_move(pos)
                    return
            # Fall back to pan
            self._begin_pan(pos)
        elif event.button() == Qt.MouseButton.RightButton:
            self._begin_pan(pos)

    def mouseMoveEvent(self, event):
        pos = QPointF(event.position())
        wx, wy = self._screen_to_world(pos.x(), pos.y())
        self.cursor_moved.emit(wx, wy)

        if self._interact == _PAN:
            delta = pos - self._drag_screen_start
            self._voffset = self._drag_voffset_start + delta
            self.update()

        elif self._interact == _MOVE:
            cwx, cwy = self._screen_to_world(pos.x(), pos.y())
            self._live_dx = cwx - self._drag_world_start[0]
            self._live_dy = cwy - self._drag_world_start[1]
            new_ox = self._drag_offset_start[0] + self._live_dx
            new_oy = self._drag_offset_start[1] + self._live_dy
            self.placement_changed.emit(new_ox, new_oy, self._drag_scale_start)

        elif self._interact == _SCALE:
            ax, ay = self._drag_anchor_world
            asc = self._world_to_screen(ax, ay)
            dist = math.hypot(pos.x() - asc.x(), pos.y() - asc.y())
            if self._drag_anchor_dist > 0:
                ratio = dist / self._drag_anchor_dist
                new_scale = self._drag_scale_start * ratio
                # Compute new offsets so the anchor stays at the same world pos
                # anchor_out = src_anchor * new_scale/100 + new_offset
                # src_anchor = (anchor_out - old_offset) / (old_scale/100)
                old_s = self._drag_scale_start / 100.0
                old_ox, old_oy = self._drag_offset_start
                src_ax = (ax - old_ox) / old_s
                src_ay = (ay - old_oy) / old_s
                new_s = new_scale / 100.0
                new_ox = ax - src_ax * new_s
                new_oy = ay - src_ay * new_s
                self.placement_changed.emit(new_ox, new_oy, new_scale)

        # Update cursor shape
        if self._interact == _IDLE and self._bbox and self.groups:
            if self._hit_handle(pos) is not None:
                self.setCursor(QCursor(Qt.CursorShape.SizeFDiagCursor))
            elif self._inside_bbox(pos):
                self.setCursor(QCursor(Qt.CursorShape.SizeAllCursor))
            else:
                self.setCursor(QCursor(Qt.CursorShape.ArrowCursor))
        elif self._interact == _PAN:
            self.setCursor(QCursor(Qt.CursorShape.ClosedHandCursor))

    def mouseReleaseEvent(self, event):
        self._interact = _IDLE
        self.setCursor(QCursor(Qt.CursorShape.ArrowCursor))

    # ── Interaction helpers ───────────────────────────────────────────────────

    def _begin_pan(self, pos: QPointF):
        self._interact = _PAN
        self._drag_screen_start = pos
        self._drag_voffset_start = QPointF(self._voffset)
        self.setCursor(QCursor(Qt.CursorShape.ClosedHandCursor))

    def _begin_move(self, pos: QPointF):
        self._interact = _MOVE
        self._drag_world_start = self._screen_to_world(pos.x(), pos.y())
        pa = self.settings.path
        self._drag_offset_start = (pa.offset_x, pa.offset_y)
        self._drag_scale_start  = pa.scale
        self.setCursor(QCursor(Qt.CursorShape.SizeAllCursor))

    def _begin_scale(self, pos: QPointF, sign: Tuple[int, int]):
        self._interact = _SCALE
        if not self._bbox:
            return
        x0, y0, x1, y1 = self._bbox
        sx, sy = sign
        # Anchor = opposite corner
        ax = x0 if sx > 0 else x1
        ay = y0 if sy > 0 else y1
        self._drag_anchor_world = (ax, ay)
        asc = self._world_to_screen(ax, ay)
        self._drag_anchor_dist = max(
            1.0, math.hypot(pos.x() - asc.x(), pos.y() - asc.y()))
        pa = self.settings.path
        self._drag_offset_start = (pa.offset_x, pa.offset_y)
        self._drag_scale_start  = pa.scale
        self.setCursor(QCursor(Qt.CursorShape.SizeFDiagCursor))
