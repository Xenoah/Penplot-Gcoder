"""2D preview — QPainter, interactive move/scale, draw-limit for seekbar, drawing tools."""
from __future__ import annotations
import math
from typing import List, Optional, Tuple

from PyQt6.QtCore import Qt, QPointF, QRectF, pyqtSignal
from PyQt6.QtGui import (
    QPainter, QPen, QColor, QFont, QWheelEvent, QCursor, QPainterPath,
)
from PyQt6.QtWidgets import QWidget, QSizePolicy

from src.models.pen_path import PenPath, PathGroup
from src.models.settings import AppSettings

_HANDLE_PX   = 9
_HANDLE_HALF = _HANDLE_PX / 2
# Interact states
_IDLE = "idle"; _PAN = "pan"; _MOVE = "move"; _SCALE = "scale"
# Drawing tools
_TOOL_SELECT = "select"
_TOOL_PEN    = "pen"
_TOOL_PATH   = "path"
_TOOL_LINE   = "line"
_TOOL_RECT   = "rect"
_TOOL_CIRCLE = "circle"

_DRAW_TOOLS = {_TOOL_PEN, _TOOL_PATH, _TOOL_LINE, _TOOL_RECT, _TOOL_CIRCLE}


class Preview2D(QWidget):
    cursor_moved      = pyqtSignal(float, float)         # x_mm, y_mm
    placement_changed = pyqtSignal(float, float, float)  # ox, oy, scale_pct
    path_drawn        = pyqtSignal(object)               # PenPath (user-drawn stroke)

    def __init__(self, settings: AppSettings, parent=None):
        super().__init__(parent)
        self.settings = settings
        self.groups: List[PathGroup] = []
        self._overflow   = False
        self._draw_limit = -1   # -1 = all; ≥0 = show first N paths

        self._vscale  = 1.0
        self._voffset = QPointF(0, 0)

        # Placement interaction state
        self._interact            = _IDLE
        self._drag_screen_start   = QPointF()
        self._drag_voffset_start  = QPointF()
        self._drag_world_start    = (0.0, 0.0)
        self._drag_offset_start   = (0.0, 0.0)
        self._drag_scale_start    = 100.0
        self._drag_anchor_world   = (0.0, 0.0)
        self._drag_anchor_dist    = 1.0

        self._bbox: Optional[Tuple[float, float, float, float]] = None

        # ── Drawing tool state ──────────────────────────────────────────────
        self._tool          = _TOOL_SELECT  # active tool
        self._draw_color    = "#ff0000"     # colour for new paths
        self._draw_width_mm = 0.5           # stroke width in mm
        self._draw_pts: List[Tuple[float, float]] = []   # in-progress anchor points (mm)
        self._draw_cursor: Optional[Tuple[float, float]] = None  # live cursor pos (mm)
        self._draw_active   = False

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

    def set_active_tool(self, tool: str):
        """Switch the active interaction tool."""
        self._tool = tool
        self._cancel_draw()
        if tool == _TOOL_SELECT:
            self.setCursor(QCursor(Qt.CursorShape.ArrowCursor))
        else:
            self.setCursor(QCursor(Qt.CursorShape.CrossCursor))

    def set_draw_color(self, color: str):
        self._draw_color = color

    def set_draw_width_mm(self, width: float):
        self._draw_width_mm = max(0.1, width)

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

    def keyPressEvent(self, event):
        if event.key() == Qt.Key.Key_Escape and self._draw_active:
            self._cancel_draw()
        super().keyPressEvent(event)

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
            # Use path stroke_width_mm for user-drawn paths
            pw_px = max(1.0, path.stroke_width_mm * self._vscale)
            c = QColor(path.color) if path.color and path.color != "none" else QColor("#4488ff")
            painter.setPen(QPen(c, pw_px))
            pts = [self._w2s(*pt) for pt in path.points]
            for i in range(1, len(pts)):
                painter.drawLine(pts[i-1], pts[i])
            if path.is_closed and len(pts) >= 2:
                painter.drawLine(pts[-1], pts[0])
            prev_end = path.points[-1]

        # Selection bbox + handles (select tool only)
        if self._tool == _TOOL_SELECT and self._bbox and self.groups:
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

        # In-progress drawing overlay
        self._paint_draw_overlay(painter)

        # Overflow banner
        if self._overflow:
            painter.setFont(QFont("Arial", 12, QFont.Weight.Bold))
            painter.fillRect(0, 0, self.width(), 32, QColor(200, 50, 50, 200))
            painter.setPen(Qt.GlobalColor.white)
            painter.drawText(8, 22, "\u26a0 \u30d1\u30b9\u304c\u6709\u52b9\u63cf\u753b\u30a8\u30ea\u30a2\u3092\u306f\u307f\u51fa\u3057\u3066\u3044\u307e\u3059")

        painter.end()

    def _paint_draw_overlay(self, painter: QPainter):
        """Draw in-progress stroke / shape preview."""
        if not self._draw_pts and not self._draw_active:
            return
        col = QColor(self._draw_color)
        col.setAlpha(200)
        pw_px = max(1.5, self._draw_width_mm * self._vscale)
        pen = QPen(col, pw_px)
        pen.setCapStyle(Qt.PenCapStyle.RoundCap)
        pen.setJoinStyle(Qt.PenJoinStyle.RoundJoin)
        painter.setPen(pen)
        painter.setBrush(Qt.BrushStyle.NoBrush)

        if self._tool in (_TOOL_PEN, _TOOL_PATH):
            if len(self._draw_pts) >= 2:
                pts = [self._w2s(*p) for p in self._draw_pts]
                for i in range(1, len(pts)):
                    painter.drawLine(pts[i-1], pts[i])
            # Preview line to cursor
            if self._draw_pts and self._draw_cursor:
                p0 = self._w2s(*self._draw_pts[-1])
                p1 = self._w2s(*self._draw_cursor)
                dash_pen = QPen(col, pw_px, Qt.PenStyle.DashLine)
                painter.setPen(dash_pen)
                painter.drawLine(p0, p1)
                painter.setPen(pen)

        elif self._tool == _TOOL_LINE:
            if self._draw_pts and self._draw_cursor:
                painter.drawLine(self._w2s(*self._draw_pts[0]),
                                 self._w2s(*self._draw_cursor))

        elif self._tool == _TOOL_RECT:
            if self._draw_pts and self._draw_cursor:
                p0 = self._w2s(*self._draw_pts[0])
                p1 = self._w2s(*self._draw_cursor)
                painter.drawRect(QRectF(p0, p1).normalized())

        elif self._tool == _TOOL_CIRCLE:
            if self._draw_pts and self._draw_cursor:
                cx, cy = self._draw_pts[0]
                ex, ey = self._draw_cursor
                rx = abs(ex - cx); ry = abs(ey - cy)
                sc = self._w2s(cx, cy)
                rxp = rx * self._vscale; ryp = ry * self._vscale
                painter.drawEllipse(QRectF(sc.x()-rxp, sc.y()-ryp, rxp*2, ryp*2))

        # Dot on each anchor point
        painter.setPen(QPen(col, 1))
        painter.setBrush(col)
        for wx, wy in self._draw_pts:
            sc = self._w2s(wx, wy)
            painter.drawEllipse(sc, 3.0, 3.0)

    def wheelEvent(self, event: QWheelEvent):
        factor = 1.15 if event.angleDelta().y() > 0 else 1.0/1.15
        mouse  = QPointF(event.position())
        self._voffset = mouse - (mouse - self._voffset) * factor
        self._vscale *= factor
        self.update()

    def mousePressEvent(self, event):
        pos = QPointF(event.position())
        wx, wy = self._s2w(pos.x(), pos.y())

        if event.button() == Qt.MouseButton.LeftButton:
            # ── Drawing tools ──────────────────────────────────────────
            if self._tool == _TOOL_PEN:
                self._draw_pts  = [(wx, wy)]
                self._draw_active = True
                return

            if self._tool == _TOOL_PATH:
                if not self._draw_active:
                    self._draw_pts  = [(wx, wy)]
                    self._draw_active = True
                else:
                    self._draw_pts.append((wx, wy))
                self.update()
                return

            if self._tool in (_TOOL_LINE, _TOOL_RECT, _TOOL_CIRCLE):
                self._draw_pts    = [(wx, wy)]
                self._draw_cursor = (wx, wy)
                self._draw_active = True
                return

            # ── Select / placement ─────────────────────────────────────
            if self._bbox and self.groups:
                hit = self._hit_handle(pos)
                if hit:
                    self._begin_scale(pos, hit); return
                if self._inside_bbox(pos):
                    self._begin_move(pos); return
            self._begin_pan(pos)

        elif event.button() == Qt.MouseButton.RightButton:
            if self._draw_active:
                self._cancel_draw()
            else:
                self._begin_pan(pos)

    def mouseDoubleClickEvent(self, event):
        if self._tool == _TOOL_PATH and self._draw_active:
            # Remove last point (it was added by the click that triggered double-click)
            if len(self._draw_pts) > 1:
                self._draw_pts.pop()
            self._finalize_draw()

    def mouseMoveEvent(self, event):
        pos = QPointF(event.position())
        wx, wy = self._s2w(pos.x(), pos.y())
        self.cursor_moved.emit(wx, wy)

        # Drawing live preview
        if self._tool in _DRAW_TOOLS:
            self._draw_cursor = (wx, wy)
            if self._tool == _TOOL_PEN and self._draw_active:
                self._draw_pts.append((wx, wy))
            self.update()
            return

        # Placement interactions
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

    def mouseReleaseEvent(self, event):
        if event.button() == Qt.MouseButton.LeftButton:
            if self._tool in (_TOOL_LINE, _TOOL_RECT, _TOOL_CIRCLE) and self._draw_active:
                self._finalize_draw()
                return
            if self._tool == _TOOL_PEN and self._draw_active:
                self._finalize_draw()
                return
        self._interact = _IDLE
        self.setCursor(QCursor(Qt.CursorShape.ArrowCursor))

    # ── placement helpers ────────────────────────────────────────────────────

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

    # ── drawing helpers ──────────────────────────────────────────────────────

    def _cancel_draw(self):
        self._draw_pts    = []
        self._draw_cursor = None
        self._draw_active = False
        self.update()

    def _finalize_draw(self):
        """Commit the in-progress drawing as a PenPath and emit path_drawn."""
        pts = self._build_draw_points()
        self._draw_pts    = []
        self._draw_cursor = None
        self._draw_active = False
        self.update()
        if len(pts) >= 2:
            p = PenPath(
                points=pts,
                color=self._draw_color,
                is_closed=(self._tool in (_TOOL_RECT, _TOOL_CIRCLE)),
                layer="draw",
                stroke_width_mm=self._draw_width_mm,
            )
            self.path_drawn.emit(p)

    def _build_draw_points(self) -> List[Tuple[float, float]]:
        """Construct the final point list for the current tool and draw state."""
        if not self._draw_pts:
            return []

        if self._tool in (_TOOL_PEN, _TOOL_PATH):
            return list(self._draw_pts)

        if not self._draw_cursor:
            return list(self._draw_pts)

        x0, y0 = self._draw_pts[0]
        x1, y1 = self._draw_cursor

        if self._tool == _TOOL_LINE:
            return [(x0, y0), (x1, y1)]

        if self._tool == _TOOL_RECT:
            return [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]

        if self._tool == _TOOL_CIRCLE:
            cx, cy = x0, y0
            rx, ry = abs(x1 - x0), abs(y1 - y0)
            n = max(32, int(2 * math.pi * max(rx, ry) / 0.5))
            pts = []
            for i in range(n + 1):
                a = 2 * math.pi * i / n
                pts.append((cx + rx * math.cos(a), cy + ry * math.sin(a)))
            return pts

        return list(self._draw_pts)
