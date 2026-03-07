"""2D preview — QPainter, interactive move/scale, draw-limit for seekbar."""
from __future__ import annotations
import math
from typing import List, Optional, Tuple

from PyQt6.QtCore import Qt, QPointF, QRectF, pyqtSignal
from PyQt6.QtGui import QPainter, QPen, QColor, QFont, QWheelEvent, QCursor
from PyQt6.QtWidgets import QWidget, QSizePolicy

from src.models.pen_path import PenPath, PathGroup
from src.models.settings import AppSettings

_HANDLE_PX   = 9
_HANDLE_HALF = _HANDLE_PX / 2
_IDLE = "idle"; _PAN = "pan"; _MOVE = "move"; _SCALE = "scale"


class Preview2D(QWidget):
    cursor_moved      = pyqtSignal(float, float)    # x_mm, y_mm
    placement_changed = pyqtSignal(float, float, float)  # ox, oy, scale_pct

    def __init__(self, settings: AppSettings, parent=None):
        super().__init__(parent)
        self.settings = settings
        self.groups: List[PathGroup] = []
        self._overflow   = False
        self._draw_limit = -1   # -1 = all; ≥0 = show first N paths

        self._vscale  = 1.0
        self._voffset = QPointF(0, 0)

        self._interact            = _IDLE
        self._drag_screen_start   = QPointF()
        self._drag_voffset_start  = QPointF()
        self._drag_world_start    = (0.0, 0.0)
        self._drag_offset_start   = (0.0, 0.0)
        self._drag_scale_start    = 100.0
        self._drag_anchor_world   = (0.0, 0.0)
        self._drag_anchor_dist    = 1.0

        self._bbox: Optional[Tuple[float, float, float, float]] = None

        self.setMinimumSize(300, 300)
        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Expanding)
        self.setMouseTracking(True)
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)

    # ── public ──────────────────────────────────────────────────────────────

    def set_groups(self, groups: List[PathGroup]):
        self.groups  = groups
        self._bbox   = self._compute_bbox()
        self._overflow = self._check_overflow()
        self.update()

    def set_overflow(self, overflow: bool):
        self._overflow = overflow
        self.update()

    def set_draw_limit(self, n: int):
        """Show only the first n paths (−1 = all)."""
        self._draw_limit = n
        self.update()

    def fit_view(self):
        bx = self.settings.machine.bed_x
        by = self.settings.machine.bed_y
        if bx <= 0 or by <= 0:
            return
        margin = 20
        sx = (self.width()  - 2 * margin) / bx
        sy = (self.height() - 2 * margin) / by
        self._vscale  = min(sx, sy)
        self._voffset = QPointF(
            margin + (self.width()  - 2 * margin - bx * self._vscale) / 2,
            margin + (self.height() - 2 * margin - by * self._vscale) / 2,
        )
        self.update()

    # ── coord helpers ────────────────────────────────────────────────────────

    def _w2s(self, x, y) -> QPointF:
        by = self.settings.machine.bed_y
        return QPointF(x * self._vscale + self._voffset.x(),
                       (by - y) * self._vscale + self._voffset.y())

    def _s2w(self, sx, sy) -> Tuple[float, float]:
        by = self.settings.machine.bed_y
        return ((sx - self._voffset.x()) / self._vscale,
                by - (sy - self._voffset.y()) / self._vscale)

    # ── bbox ─────────────────────────────────────────────────────────────────

    def _compute_bbox(self) -> Optional[Tuple[float, float, float, float]]:
        xs, ys = [], []
        for g in self.groups:
            for p in g.paths:
                for x, y in p.points:
                    xs.append(x); ys.append(y)
        return (min(xs), min(ys), max(xs), max(ys)) if xs else None

    def _check_overflow(self) -> bool:
        x_min, y_min, x_max, y_max = self.settings.effective_area()
        for g in self.groups:
            for p in g.paths:
                for x, y in p.points:
                    if x < x_min-1e-4 or x > x_max+1e-4 or y < y_min-1e-4 or y > y_max+1e-4:
                        return True
        return False

    def _handle_rects(self) -> List[Tuple[QRectF, int, int]]:
        if not self._bbox:
            return []
        x0, y0, x1, y1 = self._bbox
        result = []
        for wx, wy, sx, sy in [(x0,y0,-1,-1),(x1,y0,1,-1),(x0,y1,-1,1),(x1,y1,1,1)]:
            sc = self._w2s(wx, wy)
            r  = QRectF(sc.x()-_HANDLE_HALF, sc.y()-_HANDLE_HALF, _HANDLE_PX, _HANDLE_PX)
            result.append((r, sx, sy))
        return result

    def _hit_handle(self, pos):
        for rect, sx, sy in self._handle_rects():
            if rect.contains(pos):
                return sx, sy
        return None

    def _inside_bbox(self, pos) -> bool:
        if not self._bbox:
            return False
        x0, y0, x1, y1 = self._bbox
        return QRectF(self._w2s(x0, y1), self._w2s(x1, y0)).contains(pos)

    # ── Qt events ────────────────────────────────────────────────────────────

    def showEvent(self, _):   self.fit_view()
    def resizeEvent(self, _): self.fit_view()

    def paintEvent(self, _):
        painter = QPainter(self)
        painter.setRenderHint(QPainter.RenderHint.Antialiasing)
        painter.fillRect(self.rect(), QColor("#2b2b2b"))

        bx = self.settings.machine.bed_x
        by = self.settings.machine.bed_y
        x_min, y_min, x_max, y_max = self.settings.effective_area()

        # Bed
        painter.setPen(QPen(QColor("#888888"), 1.5))
        painter.setBrush(QColor("#1e1e1e"))
        painter.drawRect(QRectF(self._w2s(0, by), self._w2s(bx, 0)))

        # Effective area
        painter.setPen(QPen(QColor("#44aa44"), 1.5, Qt.PenStyle.DashLine))
        painter.setBrush(Qt.BrushStyle.NoBrush)
        painter.drawRect(QRectF(self._w2s(x_min, y_max), self._w2s(x_max, y_min)))

        # Grid
        painter.setPen(QPen(QColor("#333333"), 0.5))
        x = 0.0
        while x <= bx:
            painter.drawLine(self._w2s(x, 0), self._w2s(x, by)); x += 10
        y = 0.0
        while y <= by:
            painter.drawLine(self._w2s(0, y), self._w2s(bx, y)); y += 10

        # Paths (respecting draw limit)
        all_paths = [p for g in self.groups for p in g.paths]
        limit     = self._draw_limit if self._draw_limit >= 0 else len(all_paths)
        draw_list = all_paths[:limit]

        prev_end = None
        for path in draw_list:
            if not path.points:
                continue
            if prev_end is not None:
                painter.setPen(QPen(QColor("#ff4444"), 1.0, Qt.PenStyle.DotLine))
                painter.drawLine(self._w2s(*prev_end), self._w2s(*path.points[0]))
            painter.setPen(QPen(QColor("#4488ff"), 1.5))
            pts = [self._w2s(*pt) for pt in path.points]
            for i in range(1, len(pts)):
                painter.drawLine(pts[i-1], pts[i])
            prev_end = path.points[-1]

        # Selection bbox + handles
        if self._bbox and self.groups:
            x0, y0, x1, y1 = self._bbox
            pen = QPen(QColor("#ffffff"), 1, Qt.PenStyle.DashLine)
            pen.setDashPattern([4, 4])
            painter.setPen(pen)
            painter.setBrush(Qt.BrushStyle.NoBrush)
            painter.drawRect(QRectF(self._w2s(x0, y1), self._w2s(x1, y0)))
            painter.setPen(QPen(QColor("#ffffff"), 1))
            painter.setBrush(QColor("#4488ff"))
            for rect, _, _ in self._handle_rects():
                painter.drawRect(rect)

        # Overflow banner
        if self._overflow:
            painter.setFont(QFont("Arial", 12, QFont.Weight.Bold))
            painter.fillRect(0, 0, self.width(), 32, QColor(200, 50, 50, 200))
            painter.setPen(Qt.GlobalColor.white)
            painter.drawText(8, 22, "⚠ パスが有効描画エリアをはみ出しています")

        painter.end()

    def wheelEvent(self, event: QWheelEvent):
        factor = 1.15 if event.angleDelta().y() > 0 else 1.0/1.15
        mouse  = QPointF(event.position())
        self._voffset = mouse - (mouse - self._voffset) * factor
        self._vscale *= factor
        self.update()

    def mousePressEvent(self, event):
        pos = QPointF(event.position())
        if event.button() == Qt.MouseButton.LeftButton:
            if self._bbox and self.groups:
                hit = self._hit_handle(pos)
                if hit:
                    self._begin_scale(pos, hit); return
                if self._inside_bbox(pos):
                    self._begin_move(pos); return
            self._begin_pan(pos)
        elif event.button() == Qt.MouseButton.RightButton:
            self._begin_pan(pos)

    def mouseMoveEvent(self, event):
        pos = QPointF(event.position())
        wx, wy = self._s2w(pos.x(), pos.y())
        self.cursor_moved.emit(wx, wy)

        if self._interact == _PAN:
            self._voffset = self._drag_voffset_start + (pos - self._drag_screen_start)
            self.update()
        elif self._interact == _MOVE:
            cwx, cwy = self._s2w(pos.x(), pos.y())
            dx = cwx - self._drag_world_start[0]
            dy = cwy - self._drag_world_start[1]
            self.placement_changed.emit(self._drag_offset_start[0]+dx,
                                        self._drag_offset_start[1]+dy,
                                        self._drag_scale_start)
        elif self._interact == _SCALE:
            ax, ay = self._drag_anchor_world
            asc  = self._w2s(ax, ay)
            dist = math.hypot(pos.x()-asc.x(), pos.y()-asc.y())
            if self._drag_anchor_dist > 0:
                ratio  = dist / self._drag_anchor_dist
                ns     = self._drag_scale_start * ratio
                old_s  = self._drag_scale_start / 100.0
                ox, oy = self._drag_offset_start
                sax    = (ax - ox) / old_s
                say    = (ay - oy) / old_s
                self.placement_changed.emit(ax - sax*(ns/100), ay - say*(ns/100), ns)

        if self._interact == _IDLE and self._bbox and self.groups:
            if self._hit_handle(pos):
                self.setCursor(QCursor(Qt.CursorShape.SizeFDiagCursor))
            elif self._inside_bbox(pos):
                self.setCursor(QCursor(Qt.CursorShape.SizeAllCursor))
            else:
                self.setCursor(QCursor(Qt.CursorShape.ArrowCursor))
        elif self._interact == _PAN:
            self.setCursor(QCursor(Qt.CursorShape.ClosedHandCursor))

    def mouseReleaseEvent(self, _):
        self._interact = _IDLE
        self.setCursor(QCursor(Qt.CursorShape.ArrowCursor))

    def _begin_pan(self, pos):
        self._interact           = _PAN
        self._drag_screen_start  = pos
        self._drag_voffset_start = QPointF(self._voffset)
        self.setCursor(QCursor(Qt.CursorShape.ClosedHandCursor))

    def _begin_move(self, pos):
        self._interact          = _MOVE
        self._drag_world_start  = self._s2w(pos.x(), pos.y())
        pa = self.settings.path
        self._drag_offset_start = (pa.offset_x, pa.offset_y)
        self._drag_scale_start  = pa.scale

    def _begin_scale(self, pos, sign):
        self._interact = _SCALE
        if not self._bbox: return
        x0, y0, x1, y1 = self._bbox
        sx, sy = sign
        ax = x0 if sx > 0 else x1
        ay = y0 if sy > 0 else y1
        self._drag_anchor_world = (ax, ay)
        asc = self._w2s(ax, ay)
        self._drag_anchor_dist  = max(1.0, math.hypot(pos.x()-asc.x(), pos.y()-asc.y()))
        pa = self.settings.path
        self._drag_offset_start = (pa.offset_x, pa.offset_y)
        self._drag_scale_start  = pa.scale
        self.setCursor(QCursor(Qt.CursorShape.SizeFDiagCursor))
