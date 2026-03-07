"""Raw view — GIMP-style canvas for source image / vector editing."""
from __future__ import annotations
import os
import re
import shutil
import tempfile
from typing import List, Set

from PyQt6.QtCore import Qt, QTimer, pyqtSignal, QPointF
from PyQt6.QtGui import (
    QBrush, QColor, QPen, QPixmap, QPainter,
    QPainterPath, QShortcut, QKeySequence, QFont,
)
from PyQt6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel,
    QComboBox, QSlider, QDoubleSpinBox,
    QGraphicsView, QGraphicsScene, QGraphicsItem,
    QGraphicsPathItem,
)

from src.models.pen_path import PenPath, PathGroup
from src.models.settings import AppSettings


def _safe_path(filepath: str):
    needs_copy = False
    try:
        filepath.encode("ascii")
    except UnicodeEncodeError:
        needs_copy = True
    if len(filepath) > 200 or re.search(r'[#%&{}\<>\*\?\$!\'"@+`|=\(\)\[\]]', filepath):
        needs_copy = True
    if not needs_copy:
        return filepath, False
    ext = os.path.splitext(filepath)[1].lower()
    tmp = tempfile.NamedTemporaryFile(suffix=ext, delete=False)
    tmp.close()
    shutil.copy2(filepath, tmp.name)
    return tmp.name, True


# ---------------------------------------------------------------------------
# Canvas
# ---------------------------------------------------------------------------
class _Canvas(QGraphicsView):
    """Zoom/pan graphics canvas with mouse-wheel zoom."""

    def __init__(self, scene: QGraphicsScene):
        super().__init__(scene)
        self.setDragMode(QGraphicsView.DragMode.ScrollHandDrag)
        self.setTransformationAnchor(QGraphicsView.ViewportAnchor.AnchorUnderMouse)
        self.setRenderHints(
            QPainter.RenderHint.Antialiasing |
            QPainter.RenderHint.SmoothPixmapTransform)
        self.setBackgroundBrush(QBrush(QColor("#1a1a1a")))
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)

    def wheelEvent(self, event):
        factor = 1.15 if event.angleDelta().y() > 0 else 1.0 / 1.15
        self.scale(factor, factor)

    def keyPressEvent(self, event):
        key = event.key()
        if key in (Qt.Key.Key_Plus, Qt.Key.Key_Equal):
            self.scale(1.25, 1.25)
        elif key == Qt.Key.Key_Minus:
            self.scale(0.8, 0.8)
        elif key in (Qt.Key.Key_0, Qt.Key.Key_F):
            self.fitInView(self.scene().sceneRect(),
                           Qt.AspectRatioMode.KeepAspectRatio)
        else:
            super().keyPressEvent(event)


# ---------------------------------------------------------------------------
# RawView
# ---------------------------------------------------------------------------
class RawView(QWidget):
    """
    Raw-data canvas.

    Image mode (PNG/JPG):
      - Displays the original raster image with a GIMP-style checkerboard.
      - Overlays extracted contours in green.
      - Parameter bar lets the user adjust extraction mode / threshold / simplify
        and see the result in real time.

    Vector mode (SVG/DXF):
      - Displays raw paths in original file coordinates (Y-up convention flipped).
      - Click a path to select it (highlighted orange).
      - Press Delete to remove selected paths (emits groups_changed).
    """

    # Emitted when the user edits the source data (param change or path delete)
    groups_changed = pyqtSignal(list)   # List[PathGroup]

    _MODE_LABELS = ["Auto", "Canny (線画向け)", "Threshold (塗り形状向け)"]
    _MODE_VALUES = ["auto", "canny", "threshold"]

    def __init__(self, settings: AppSettings, parent=None):
        super().__init__(parent)
        self.settings = settings

        self._image_path = ""
        self._image_h    = 1
        self._image_w    = 1
        self._is_image   = False
        self._groups: List[PathGroup] = []

        self._contour_items: List[QGraphicsPathItem] = []
        self._path_map: List[tuple] = []   # (item, group_idx, path_idx)
        self._bed_items  = []              # bed/effective-area overlay items

        self._scene  = QGraphicsScene(self)
        self._canvas = _Canvas(self._scene)

        # Debounce timer for re-extraction
        self._debounce = QTimer(self)
        self._debounce.setSingleShot(True)
        self._debounce.setInterval(450)
        self._debounce.timeout.connect(self._re_extract)

        self._build_ui()

    # ------------------------------------------------------------------
    def _build_ui(self):
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)
        layout.addWidget(self._canvas, 1)

        # ── Image parameter bar ────────────────────────────────────────
        self._param_bar = QWidget()
        self._param_bar.setFixedHeight(36)
        self._param_bar.setStyleSheet(
            "background:#252525; border-top:1px solid #444;")
        hl = QHBoxLayout(self._param_bar)
        hl.setContentsMargins(8, 4, 8, 4)
        hl.setSpacing(10)

        hl.addWidget(QLabel("抽出モード:"))
        self._mode_combo = QComboBox()
        self._mode_combo.addItems(self._MODE_LABELS)
        self._mode_combo.setFixedWidth(180)
        hl.addWidget(self._mode_combo)

        hl.addWidget(QLabel("Threshold:"))
        self._thr_slider = QSlider(Qt.Orientation.Horizontal)
        self._thr_slider.setRange(0, 255)
        self._thr_slider.setValue(127)
        self._thr_slider.setFixedWidth(130)
        self._thr_label = QLabel("127")
        self._thr_label.setFixedWidth(28)
        hl.addWidget(self._thr_slider)
        hl.addWidget(self._thr_label)

        hl.addWidget(QLabel("簡略化:"))
        self._eps_spin = QDoubleSpinBox()
        self._eps_spin.setRange(0.0, 10.0)
        self._eps_spin.setSingleStep(0.1)
        self._eps_spin.setDecimals(1)
        self._eps_spin.setValue(0.5)
        self._eps_spin.setFixedWidth(62)
        hl.addWidget(self._eps_spin)

        self._count_lbl = QLabel("輪郭: 0")
        self._count_lbl.setStyleSheet("color:#aaa; font-size:11px;")
        hl.addWidget(self._count_lbl)
        hl.addStretch()

        # ── Vector info bar ───────────────────────────────────────────
        self._vec_bar = QWidget()
        self._vec_bar.setFixedHeight(28)
        self._vec_bar.setStyleSheet(
            "background:#252525; border-top:1px solid #444;")
        hl2 = QHBoxLayout(self._vec_bar)
        hl2.setContentsMargins(8, 2, 8, 2)
        self._vec_info = QLabel("クリックでパスを選択  /  Delete で削除")
        self._vec_info.setStyleSheet("color:#888; font-size:11px;")
        hl2.addWidget(self._vec_info)
        hl2.addStretch()

        layout.addWidget(self._param_bar)
        layout.addWidget(self._vec_bar)

        # Signals
        self._thr_slider.valueChanged.connect(self._on_thr_changed)
        self._mode_combo.currentIndexChanged.connect(lambda _: self._debounce.start())
        self._eps_spin.valueChanged.connect(lambda _: self._debounce.start())
        self._scene.selectionChanged.connect(self._on_selection_changed)

        # Delete shortcut bound to canvas
        self._del_sc = QShortcut(QKeySequence(Qt.Key.Key_Delete), self._canvas)
        self._del_sc.activated.connect(self._delete_selected)

        self._param_bar.setVisible(False)
        self._vec_bar.setVisible(False)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------
    def set_image_file(self, filepath: str, groups: List[PathGroup]):
        """Show original raster image + extracted contour overlay."""
        self._image_path = filepath
        self._is_image   = True
        self._groups     = list(groups)

        self._scene.clear()
        self._contour_items = []
        self._path_map      = []

        # Load pixmap (handle non-ASCII paths)
        pixmap = QPixmap(filepath)
        if pixmap.isNull():
            safe, is_tmp = _safe_path(filepath)
            pixmap = QPixmap(safe)
            if is_tmp:
                try: os.remove(safe)
                except OSError: pass

        if not pixmap.isNull():
            self._image_w = pixmap.width()
            self._image_h = pixmap.height()
            # Checkerboard background under the image
            self._draw_checkerboard(self._image_w, self._image_h)
            self._scene.addPixmap(pixmap)
        else:
            self._image_w = self._image_h = 512
            self._scene.addRect(0, 0, 512, 512,
                                QPen(QColor("#555")), QBrush(QColor("#2a2a2a")))

        self._draw_contours(groups)
        self.update_bed_overlay()
        self._canvas.fitInView(self._scene.sceneRect(),
                               Qt.AspectRatioMode.KeepAspectRatio)
        self._param_bar.setVisible(True)
        self._vec_bar.setVisible(False)
        # Auto-trigger a first extraction if no groups were supplied
        if not groups:
            self._debounce.start()

    def set_vector_groups(self, groups: List[PathGroup]):
        """Show raw vector paths (click to select, Delete to remove)."""
        self._is_image = False
        self._groups   = list(groups)

        self._scene.clear()
        self._contour_items = []
        self._path_map      = []

        for gi, grp in enumerate(groups):
            color = QColor(grp.color if grp.color else "#00cc66")
            pen = QPen(color, 0)
            pen.setCosmetic(True)
            for pi, path in enumerate(grp.paths):
                if len(path.points) < 2:
                    continue
                qpath = QPainterPath()
                px, py = path.points[0]
                qpath.moveTo(px, -py)          # Y-flip: plotter Y-up → scene Y-down
                for x, y in path.points[1:]:
                    qpath.lineTo(x, -y)
                item = self._scene.addPath(qpath, pen)
                item.setFlag(QGraphicsItem.GraphicsItemFlag.ItemIsSelectable, True)
                item.setData(0, gi)
                item.setData(1, pi)
                self._path_map.append((item, gi, pi))

        if self._path_map:
            self._canvas.fitInView(self._scene.sceneRect(),
                                   Qt.AspectRatioMode.KeepAspectRatio)
        self._param_bar.setVisible(False)
        self._vec_bar.setVisible(True)
        self._vec_info.setText("クリックでパスを選択  /  Delete で削除")

    def clear(self):
        self._scene.clear()
        self._contour_items = []
        self._path_map      = []
        self._is_image      = False
        self._image_path    = ""
        self._groups        = []
        self._param_bar.setVisible(False)
        self._vec_bar.setVisible(False)

    # ------------------------------------------------------------------
    # Vector editing
    # ------------------------------------------------------------------
    def _on_selection_changed(self):
        if self._is_image:
            return
        selected = set(self._scene.selectedItems())
        pen_sel = QPen(QColor(255, 100, 40), 0)
        pen_sel.setCosmetic(True)
        for item, gi, _ in self._path_map:
            grp = self._groups[gi] if gi < len(self._groups) else None
            if item in selected:
                item.setPen(pen_sel)
            else:
                color = QColor((grp.color if grp else None) or "#00cc66")
                p = QPen(color, 0)
                p.setCosmetic(True)
                item.setPen(p)
        n = len(selected)
        self._vec_info.setText(
            f"{n} パス選択中  /  Delete で削除" if n else
            "クリックでパスを選択  /  Delete で削除")

    def _delete_selected(self):
        if self._is_image:
            return
        selected = set(self._scene.selectedItems())
        if not selected:
            return

        new_groups: List[PathGroup] = []
        for gi, grp in enumerate(self._groups):
            kept = []
            for pi, path in enumerate(grp.paths):
                item = next((it for it, g, p in self._path_map
                             if g == gi and p == pi), None)
                if item not in selected:
                    kept.append(path)
            if kept:
                new_groups.append(PathGroup(
                    color=grp.color, label=grp.label,
                    paths=kept, pen_change_before=grp.pen_change_before))

        self.set_vector_groups(new_groups)
        self.groups_changed.emit(new_groups)

    def get_current_groups(self) -> list:
        """Return the most-recently extracted groups (visual preview result)."""
        return list(self._groups)

    # ------------------------------------------------------------------
    # Image re-extraction
    # ------------------------------------------------------------------
    def _on_thr_changed(self, val: int):
        self._thr_label.setText(str(val))
        self._debounce.start()

    def _re_extract(self):
        if not self._image_path or not self._is_image:
            return
        mode      = self._MODE_VALUES[self._mode_combo.currentIndex()]
        threshold = self._thr_slider.value()
        epsilon   = self._eps_spin.value()

        safe, is_tmp = _safe_path(self._image_path)
        try:
            from src.core.importer.image_importer import import_image
            groups = import_image(safe, threshold=threshold,
                                  simplify_epsilon=epsilon, mode=mode)
        except Exception:
            return
        finally:
            if is_tmp and os.path.exists(safe):
                try: os.remove(safe)
                except OSError: pass

        self._groups = groups
        self._draw_contours(groups)
        # NOTE: groups_changed is NOT emitted here — extraction is visual-only.
        # Commit happens when the user presses Generate in the main window.

    # ------------------------------------------------------------------
    # Bed overlay (image mode)
    # ------------------------------------------------------------------
    def update_bed_overlay(self):
        """Draw bed boundary + effective area in scene (pixel) coordinates.

        Pixel → plotter-mm mapping (derived from current path settings):
            px = (wx - offset_x) / scale_frac
            py = H  - (wy - offset_y) / scale_frac   (Y-flip)
        so  wx = px * scale_frac + offset_x
            wy = (H - py) * scale_frac + offset_y
        """
        if not self._is_image:
            return
        # Remove stale overlay items
        for item in self._bed_items:
            self._scene.removeItem(item)
        self._bed_items = []

        pa  = self.settings.path
        s   = pa.scale / 100.0          # px → mm
        if s <= 0:
            return
        ox, oy = pa.offset_x, pa.offset_y
        H  = self._image_h

        def mm_to_scene(wx_mm: float, wy_mm: float):
            px = (wx_mm - ox) / s
            py = H - (wy_mm - oy) / s
            return px, py

        bx = self.settings.machine.bed_x
        by = self.settings.machine.bed_y
        x_min, y_min, x_max, y_max = self.settings.effective_area()

        # --- bed boundary (grey dashed) ---
        bx0, by0 = mm_to_scene(0,  0)
        bx1, by1 = mm_to_scene(bx, by)
        bed_pen = QPen(QColor(100, 100, 100, 160), 1.5)
        bed_pen.setCosmetic(True)
        bed_pen.setStyle(Qt.PenStyle.DashLine)
        r = self._scene.addRect(
            min(bx0, bx1), min(by0, by1),
            abs(bx1 - bx0), abs(by1 - by0),
            bed_pen, QBrush(Qt.BrushStyle.NoBrush))
        self._bed_items.append(r)

        # --- effective area (green solid) ---
        ex0, ey0 = mm_to_scene(x_min, y_min)
        ex1, ey1 = mm_to_scene(x_max, y_max)
        eff_pen = QPen(QColor(60, 200, 80, 200), 1.5)
        eff_pen.setCosmetic(True)
        r2 = self._scene.addRect(
            min(ex0, ex1), min(ey0, ey1),
            abs(ex1 - ex0), abs(ey1 - ey0),
            eff_pen, QBrush(Qt.BrushStyle.NoBrush))
        self._bed_items.append(r2)

        # --- bed size label ---
        lbl = self._scene.addSimpleText(
            f"{bx:.0f}×{by:.0f}mm")
        lbl.setPos(min(bx0, bx1) + 2, min(by0, by1) + 2)
        lbl.setBrush(QBrush(QColor(140, 140, 140, 200)))
        font = QFont()
        font.setPointSize(7)
        lbl.setFont(font)
        lbl.setFlag(lbl.GraphicsItemFlag.ItemIgnoresTransformations, False)
        self._bed_items.append(lbl)

    # ------------------------------------------------------------------
    # Drawing helpers
    # ------------------------------------------------------------------
    def _draw_checkerboard(self, w: int, h: int):
        """GIMP-style tiled checkerboard background."""
        sz = 16
        tile = QPixmap(sz * 2, sz * 2)
        tile.fill(QColor("#cccccc"))
        p = QPainter(tile)
        p.fillRect(sz, 0, sz, sz, QColor("#888888"))
        p.fillRect(0, sz, sz, sz, QColor("#888888"))
        p.end()
        self._scene.addRect(0, 0, w, h,
                            QPen(Qt.PenStyle.NoPen),
                            QBrush(tile))

    def _draw_contours(self, groups: List[PathGroup]):
        """Overlay contours on the image (flip Y: plotter-Y-up → image-Y-down)."""
        for item in self._contour_items:
            self._scene.removeItem(item)
        self._contour_items = []

        h   = self._image_h
        pen = QPen(QColor(0, 220, 110, 210), 1.5)
        pen.setCosmetic(True)

        total = 0
        for grp in groups:
            for path in grp.paths:
                if len(path.points) < 2:
                    continue
                qpath = QPainterPath()
                x0, y0 = path.points[0]
                qpath.moveTo(x0, h - y0)
                for x, y in path.points[1:]:
                    qpath.lineTo(x, h - y)
                item = self._scene.addPath(qpath, pen)
                self._contour_items.append(item)
                total += 1

        self._count_lbl.setText(f"輪郭: {total}")
