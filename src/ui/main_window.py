"""Main application window — single-screen slicer layout."""
from __future__ import annotations
import os
import re
import shutil
import tempfile
from typing import List, Optional

import numpy as np

from PyQt6.QtCore import Qt, QTimer
from PyQt6.QtGui import QAction, QKeySequence, QUndoStack, QUndoCommand
from PyQt6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QHBoxLayout,
    QPushButton, QLabel, QStatusBar, QFileDialog,
    QMessageBox, QSplitter, QDoubleSpinBox, QSlider, QFrame,
    QApplication,
)

from src.models.settings import AppSettings
from src.models.pen_path import PathGroup
from src.ui.settings_panel import SettingsPanel
from src.ui.preview_2d import Preview2D
from src.ui.preview_3d import Preview3D
from src.ui.raw_view import RawView
from src.ui.path_list import PathListPanel
from src.ui.raw_toolbar import RawToolbar
from src.core.bed_calculator import paths_overflow
from src.core.path_optimizer import optimize
from src.core.fill_generator import generate_fills_for_paths
from src.core.gcode_generator import generate_gcode


# ---------------------------------------------------------------------------
# File-path safety
# ---------------------------------------------------------------------------
def _safe_filepath(filepath: str) -> tuple[str, bool]:
    needs_copy = False
    try:
        filepath.encode("ascii")
    except UnicodeEncodeError:
        needs_copy = True
    if len(filepath) > 200:
        needs_copy = True
    if re.search(r'[#%&{}\<\>\*\?\$\!\'\"\@\+\`\|=\(\)\[\]]', filepath):
        needs_copy = True
    if not needs_copy:
        return filepath, False
    ext = os.path.splitext(filepath)[1].lower()
    tmp = tempfile.NamedTemporaryFile(suffix=ext, delete=False)
    tmp.close()
    shutil.copy2(filepath, tmp.name)
    return tmp.name, True


# ---------------------------------------------------------------------------
# Undo
# ---------------------------------------------------------------------------
class PlacementCommand(QUndoCommand):
    def __init__(self, win, old, new):
        super().__init__("配置変更")
        self._win = win
        self._old = old
        self._new = new

    def _apply(self, v):
        s, ox, oy, rot = v
        pa = self._win.settings.path
        pa.scale = s; pa.offset_x = ox; pa.offset_y = oy; pa.rotation = rot
        self._win._prev_snap = v
        self._win._settings_panel._refresh_from_settings()
        self._win._sync_bottom_bar()
        self._win._mark_preview_dirty()   # placement change → Preview stale

    def undo(self):  self._apply(self._old)
    def redo(self):  self._apply(self._new)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
def _dspin(lo, hi, dec=2, step=0.5, suffix="") -> QDoubleSpinBox:
    w = QDoubleSpinBox()
    w.setRange(lo, hi); w.setDecimals(dec); w.setSingleStep(step)
    if suffix: w.setSuffix(suffix)
    return w


def _sep(vertical=True) -> QFrame:
    f = QFrame()
    f.setFrameShape(QFrame.Shape.VLine if vertical else QFrame.Shape.HLine)
    f.setStyleSheet("color: #555;")
    return f


# ---------------------------------------------------------------------------
# Main Window
# ---------------------------------------------------------------------------
class MainWindow(QMainWindow):

    # ── Generate-button pulse styles ────────────────────────────────────
    _PULSE_A = ("QPushButton{background:#c05000;color:white;font-weight:bold;"
                "border-radius:3px;border:1px solid #e06020;}")
    _PULSE_B = ("QPushButton{background:#f07820;color:white;font-weight:bold;"
                "border-radius:3px;border:1px solid #ffa040;}")
    _PULSE_OFF = ""   # reset to default stylesheet

    def __init__(self):
        super().__init__()
        self.settings  = AppSettings()
        self._load_autosave()

        # _layers: [{name, filepath, groups, visible}]  — one entry per imported file
        self._layers:         List[dict] = []
        self._display_groups: List[PathGroup] = []
        self._raw_image_path  = ""
        self._raw_view_dirty  = False
        self._preview_dirty   = False   # True when Generate is needed
        self._overflow        = False
        self._gcode_text: Optional[str] = None
        self._applying        = False
        self._view_mode       = "raw"   # "raw" | "transformed" | "3d"
        self._raw_show_params = False   # True = show GIMP extraction params panel

        self._undo_stack = QUndoStack(self)
        self._prev_snap  = self._snap()

        # ── Widgets ───────────────────────────────────────────────────────
        self._preview_2d     = Preview2D(self.settings)
        self._preview_3d     = Preview3D(self.settings)
        self._raw_view       = RawView(self.settings)
        self._path_list      = PathListPanel()
        self._settings_panel = SettingsPanel(self.settings)
        self._raw_toolbar    = RawToolbar()

        self._preview_2d.cursor_moved.connect(self._on_cursor_moved)
        self._preview_2d.placement_changed.connect(self._on_preview_placement)
        self._preview_2d.path_drawn.connect(self._on_path_drawn)
        self._settings_panel.settings_changed.connect(self._on_settings_changed)
        self._settings_panel.center_clicked.connect(self._on_center_clicked)
        self._path_list.order_changed.connect(self._on_path_order_changed)
        self._path_list.pen_change_changed.connect(self._on_path_order_changed)
        self._path_list.layer_visibility_changed.connect(self._on_layer_visibility_changed)
        self._path_list.add_layer_requested.connect(self._on_add_draw_layer)
        self._path_list.delete_layer_requested.connect(self._on_delete_layer)
        self._raw_view.groups_changed.connect(self._on_raw_groups_changed)
        self._raw_toolbar.tool_changed.connect(self._on_tool_changed)
        self._raw_toolbar.color_changed.connect(self._preview_2d.set_draw_color)
        self._raw_toolbar.width_changed.connect(self._preview_2d.set_draw_width_mm)
        self._raw_toolbar.img_params_toggled.connect(self._on_img_params_toggled)

        # ── Timers ────────────────────────────────────────────────────────
        self._play_timer = QTimer(self)
        self._play_timer.setInterval(80)
        self._play_timer.timeout.connect(self._play_step)

        self._autosave_timer = QTimer(self)
        self._autosave_timer.setSingleShot(True)
        self._autosave_timer.timeout.connect(self._do_autosave)

        self._pulse_timer = QTimer(self)
        self._pulse_timer.setInterval(550)
        self._pulse_timer.timeout.connect(self._pulse_step)
        self._pulse_state = False

        self.setWindowTitle("Penplot-Gcoder")
        self.resize(1400, 860)
        self._build_ui()
        self._build_menu()
        self._set_view("raw")
        self._update_status()

    # ═══════════════════════════════════════════════════ UI layout

    def _build_ui(self):
        root = QWidget()
        self.setCentralWidget(root)
        vroot = QVBoxLayout(root)
        vroot.setContentsMargins(0, 0, 0, 0)
        vroot.setSpacing(0)

        splitter = QSplitter(Qt.Orientation.Horizontal)
        vroot.addWidget(splitter, 1)

        # Left: settings
        left = QWidget()
        left.setMinimumWidth(260)
        left.setMaximumWidth(320)
        ll = QVBoxLayout(left)
        ll.setContentsMargins(4, 4, 2, 4)
        ll.addWidget(self._settings_panel)
        splitter.addWidget(left)

        # Centre: canvas + toolbar
        centre = QWidget()
        cvl = QVBoxLayout(centre)
        cvl.setContentsMargins(2, 4, 2, 2)
        cvl.setSpacing(2)

        # Toolbar
        ptb = QHBoxLayout()

        self._btn_raw   = QPushButton("Raw")
        self._btn_trans = QPushButton("Preview")
        self._btn_3d    = QPushButton("3D")
        for b in (self._btn_raw, self._btn_trans, self._btn_3d):
            b.setCheckable(True)
            b.setFixedHeight(26)
        self._btn_raw.setChecked(True)
        self._btn_raw.clicked.connect(lambda: self._set_view("raw"))
        self._btn_trans.clicked.connect(lambda: self._set_view("transformed"))
        self._btn_3d.clicked.connect(lambda: self._set_view("3d"))
        ptb.addWidget(self._btn_raw)
        ptb.addWidget(self._btn_trans)
        ptb.addWidget(self._btn_3d)
        ptb.addWidget(_sep())

        self._btn_play = QPushButton("▶")
        self._btn_play.setFixedSize(30, 26)
        self._btn_play.setCheckable(True)
        self._btn_play.clicked.connect(self._on_play_toggled)
        ptb.addWidget(self._btn_play)

        self._seekbar = QSlider(Qt.Orientation.Horizontal)
        self._seekbar.setRange(0, 0)
        self._seekbar.valueChanged.connect(self._on_seekbar_changed)
        ptb.addWidget(self._seekbar, 1)

        self._coord_lbl = QLabel("X: ---.--  Y: ---.-- mm")
        self._coord_lbl.setStyleSheet(
            "font-family: monospace; color: #aaa; font-size: 11px;")
        ptb.addWidget(self._coord_lbl)
        cvl.addLayout(ptb)

        # Raw drawing toolbar (only visible in Raw tab)
        cvl.addWidget(self._raw_toolbar)

        # Canvas stack  (Raw image | Preview2D | Preview3D)
        self._raw_view.setVisible(True)
        self._preview_2d.setVisible(False)
        self._preview_3d.setVisible(False)
        cvl.addWidget(self._raw_view,   1)
        cvl.addWidget(self._preview_2d, 1)
        cvl.addWidget(self._preview_3d, 1)

        self._info_bar = QLabel()
        self._info_bar.setStyleSheet(
            "background:#1e1e1e; color:#ccc; padding:3px 6px; font-size:11px;")
        self._info_bar.setFixedHeight(20)
        cvl.addWidget(self._info_bar)
        splitter.addWidget(centre)

        # Right: path list + stats
        right = QWidget()
        right.setMinimumWidth(180)
        right.setMaximumWidth(260)
        rvl = QVBoxLayout(right)
        rvl.setContentsMargins(2, 4, 4, 4)
        rvl.setSpacing(4)
        rvl.addWidget(QLabel("パスグループ"))
        rvl.addWidget(self._path_list, 1)
        self._stats_box = QLabel("—")
        self._stats_box.setWordWrap(True)
        self._stats_box.setStyleSheet(
            "background:#1a1a1a; color:#aaa; padding:6px; font-size:11px; "
            "border:1px solid #333; border-radius:4px;")
        rvl.addWidget(self._stats_box)
        splitter.addWidget(right)

        splitter.setStretchFactor(0, 0)
        splitter.setStretchFactor(1, 1)
        splitter.setStretchFactor(2, 0)

        vroot.addWidget(self._build_bottom_bar())
        self._status = QStatusBar()
        self.setStatusBar(self._status)

    def _build_bottom_bar(self) -> QWidget:
        bar = QWidget()
        bar.setFixedHeight(42)
        bar.setStyleSheet("background: #252525; border-top: 1px solid #444;")
        hl = QHBoxLayout(bar)
        hl.setContentsMargins(8, 4, 8, 4)
        hl.setSpacing(8)

        btn_open = QPushButton("📂 開く")
        btn_open.setFixedHeight(30)
        btn_open.clicked.connect(self._on_open)
        hl.addWidget(btn_open)

        self._file_lbl = QLabel("ファイル未選択")
        self._file_lbl.setStyleSheet("color:#888; font-size:11px;")
        self._file_lbl.setMaximumWidth(220)
        hl.addWidget(self._file_lbl)
        self._sep_placement = _sep()
        hl.addWidget(self._sep_placement)

        lbl_scale = QLabel("Scale:")
        lbl_x     = QLabel("X:")
        lbl_y     = QLabel("Y:")
        lbl_r     = QLabel("R:")
        hl.addWidget(lbl_scale)
        self._bot_scale = _dspin(0.1, 10000, 1, 1.0, " %")
        self._bot_scale.setFixedWidth(80)
        hl.addWidget(self._bot_scale)

        hl.addWidget(lbl_x)
        self._bot_x = _dspin(-9999, 9999, 2, 0.5, " mm")
        self._bot_x.setFixedWidth(90)
        hl.addWidget(self._bot_x)

        hl.addWidget(lbl_y)
        self._bot_y = _dspin(-9999, 9999, 2, 0.5, " mm")
        self._bot_y.setFixedWidth(90)
        hl.addWidget(self._bot_y)

        hl.addWidget(lbl_r)
        self._bot_rot = _dspin(-360, 360, 1, 1.0, "°")
        self._bot_rot.setFixedWidth(75)
        hl.addWidget(self._bot_rot)

        self._btn_center = QPushButton("⊕ 中央")
        self._btn_center.setFixedHeight(28)
        self._btn_center.clicked.connect(self._on_center_clicked)
        hl.addWidget(self._btn_center)

        self._bot_wh = QLabel("W: --  H: --")
        self._bot_wh.setStyleSheet(
            "color:#aaa; font-size:11px; font-family:monospace;")
        hl.addWidget(self._bot_wh)

        self._sep_placement2 = _sep()
        hl.addWidget(self._sep_placement2)

        hl.addStretch()

        self._btn_gen  = QPushButton("⚙ Generate")
        self._btn_save = QPushButton("💾 保存")
        self._btn_copy = QPushButton("📋 コピー")
        for b in (self._btn_gen, self._btn_save, self._btn_copy):
            b.setFixedHeight(30)
        self._btn_gen.clicked.connect(self._on_generate)
        self._btn_save.clicked.connect(self._on_save)
        self._btn_copy.clicked.connect(self._on_copy)
        hl.addWidget(self._btn_gen)
        hl.addWidget(self._btn_save)
        hl.addWidget(self._btn_copy)

        self._placement_widgets = [
            self._bot_scale, self._bot_x, self._bot_y,
            self._bot_rot, self._btn_center, self._bot_wh,
        ]
        self._placement_labels = [
            lbl_scale, lbl_x, lbl_y, lbl_r,
            self._sep_placement, self._sep_placement2,
        ]

        for sp in (self._bot_scale, self._bot_x, self._bot_y, self._bot_rot):
            sp.valueChanged.connect(self._on_bottom_spin)
        return bar

    # ═══════════════════════════════════════════════════ menu

    def _build_menu(self):
        mb = self.menuBar()
        fm = mb.addMenu("ファイル")
        _ma(fm, "開く...",           QKeySequence.StandardKey.Open,  self._on_open)
        _ma(fm, "G-code を保存...",  QKeySequence.StandardKey.Save,  self._on_save)
        _ma(fm, "設定を保存...",     None, self._on_save_settings)
        _ma(fm, "設定を読み込み...", None, self._on_load_settings)
        fm.addSeparator()
        _ma(fm, "終了", "Ctrl+Q", self.close)

        em = mb.addMenu("編集")
        ua = self._undo_stack.createUndoAction(self, "元に戻す")
        ua.setShortcut(QKeySequence.StandardKey.Undo)
        ra = self._undo_stack.createRedoAction(self, "やり直す")
        ra.setShortcut(QKeySequence.StandardKey.Redo)
        em.addAction(ua); em.addAction(ra)
        _ma(mb.addMenu("ヘルプ"), "バージョン情報", None, self._on_about)

    # ═══════════════════════════════════════════════════ view mode

    # ── layer helpers ────────────────────────────────────────────────────────

    @property
    def _source_files(self) -> List[str]:
        return [l['filepath'] for l in self._layers]

    def _active_source_groups(self) -> List[PathGroup]:
        """Flat list of PathGroups from all visible layers."""
        return [g for l in self._layers if l.get('visible', True) for g in l['groups']]

    def _is_image_mode(self) -> bool:
        """True when a raster image is loaded (regardless of additional draw layers)."""
        return bool(self._raw_image_path)

    def _set_view(self, mode: str):
        self._view_mode = mode
        self._btn_raw.setChecked(mode == "raw")
        self._btn_trans.setChecked(mode == "transformed")
        self._btn_3d.setChecked(mode == "3d")

        is_raw = (mode == "raw")
        is_img = self._is_image_mode()
        # Raw image mode: GIMP canvas shown only when params panel toggled on
        show_raw_view = is_raw and is_img and self._raw_show_params
        # Preview2D shown in: raw-vector, raw-image-placement, transformed
        show_2d = (mode != "3d") and not show_raw_view
        self._raw_view.setVisible(show_raw_view)
        self._preview_2d.setVisible(show_2d)
        self._preview_3d.setVisible(mode == "3d")

        # Toolbar visibility and image-mode button
        self._raw_toolbar.setVisible(is_raw)
        self._raw_toolbar.set_image_mode(is_raw and is_img)

        # Preview2D interactive only in Raw tab
        self._preview_2d.set_interactive(is_raw)

        # Seekbar / play only relevant in Preview / 3D modes
        can_anim = not is_raw
        self._seekbar.setEnabled(can_anim)
        self._btn_play.setEnabled(can_anim)

        # Placement controls only visible in Raw tab
        for w in self._placement_widgets:
            w.setVisible(is_raw)
        # Labels next to placement controls
        for w in self._placement_labels:
            w.setVisible(is_raw)

        self._push_to_preview()

    def _push_to_preview(self):
        """Feed data to the visible canvas."""
        mode    = self._view_mode
        is_img  = self._is_image_mode()

        if mode == "raw":
            if not self._active_source_groups() and not self._raw_image_path:
                self._preview_2d.set_groups([])
                self._preview_2d.set_background_image(None)
                self._raw_view.clear()
            elif is_img:
                if self._raw_show_params:
                    # GIMP canvas for extraction parameter editing
                    if self._raw_view_dirty:
                        self._raw_view.set_image_file(self._raw_image_path, [])
                        self._raw_view_dirty = False
                else:
                    # Placement view: Preview2D with background image
                    self._preview_2d.set_background_image(self._raw_image_path)
                    # Show draw layer paths on top of the image
                    draw_groups = [g for l in self._layers
                                   if l.get('is_draw', False) and l.get('visible', True)
                                   for g in l['groups']]
                    self._preview_2d.set_groups(draw_groups)
                    self._preview_2d.set_overflow(False)
            else:
                # Vector raw: show source data with placement transform applied
                self._preview_2d.set_background_image(None)
                self._preview_2d.set_groups(self._raw_placed_groups())
                self._preview_2d.set_overflow(False)
        elif mode == "transformed":
            self._preview_2d.set_background_image(None)
            self._preview_2d.set_groups(self._display_groups)
            self._preview_2d.set_overflow(self._overflow)
        else:  # 3d
            self._preview_3d.set_groups(self._display_groups)

    def _raw_placed_groups(self) -> List[PathGroup]:
        """Source groups with placement transform applied. Draw layer groups pass through."""
        pa = self.settings.path
        sc = pa.scale / 100.0
        draw_ids = {
            id(g)
            for layer in self._layers if layer.get('is_draw', False)
            for g in layer['groups']
        }
        result = []
        for grp in self._active_source_groups():
            if id(grp) in draw_ids:
                result.append(grp)   # already in bed coords
            else:
                paths = [p.transformed(sc, pa.offset_x, pa.offset_y, pa.rotation)
                         for p in grp.paths]
                result.append(PathGroup(color=grp.color, label=grp.label,
                                        paths=paths, pen_change_before=grp.pen_change_before))
        return result

    # ═══════════════════════════════════════════════════ Generate button pulse

    def _start_pulse(self):
        if not self._pulse_timer.isActive():
            self._pulse_state = False
            self._pulse_step()
            self._pulse_timer.start()

    def _stop_pulse(self):
        self._pulse_timer.stop()
        self._btn_gen.setStyleSheet(self._PULSE_OFF)

    def _pulse_step(self):
        self._pulse_state = not self._pulse_state
        self._btn_gen.setStyleSheet(
            self._PULSE_A if self._pulse_state else self._PULSE_B)

    # ═══════════════════════════════════════════════════ Preview dirty flag

    def _mark_preview_dirty(self):
        """Invalidate the Preview. Raw view updates live; Preview waits for Generate."""
        self._preview_dirty = True
        # Clear stale display data
        if self._display_groups:
            self._display_groups = []
            self._overflow = False
            self._update_seekbar_max()
            if self._view_mode == "transformed":
                self._preview_2d.set_groups([])
                self._preview_2d.set_overflow(False)
            elif self._view_mode == "3d":
                self._preview_3d.set_groups([])
        # Refresh raw canvas with latest placement
        if self._view_mode == "raw":
            if self._is_image_mode():
                if self._raw_show_params:
                    self._raw_view.update_bed_overlay()
                else:
                    # Placement view: repaint with current settings (bbox auto-refreshes)
                    draw_groups = [g for l in self._layers
                                   if l.get('is_draw', False) and l.get('visible', True)
                                   for g in l['groups']]
                    self._preview_2d.set_groups(draw_groups)
                    self._preview_2d.update()
            else:
                self._preview_2d.set_groups(self._raw_placed_groups())
                self._preview_2d.set_overflow(False)
        self._update_status()

    # ═══════════════════════════════════════════════════ play / seekbar

    def _on_play_toggled(self, checked: bool):
        if checked:
            if self._seekbar.value() >= self._seekbar.maximum():
                self._seekbar.setValue(0)
            self._play_timer.start()
            self._btn_play.setText("⏸")
        else:
            self._play_timer.stop()
            self._btn_play.setText("▶")

    def _play_step(self):
        cur = self._seekbar.value()
        if cur < self._seekbar.maximum():
            self._seekbar.setValue(cur + 1)
        else:
            self._play_timer.stop()
            self._btn_play.setChecked(False)
            self._btn_play.setText("▶")

    def _on_seekbar_changed(self, value: int):
        if self._view_mode == "3d":
            self._preview_3d.set_draw_limit(value)
        else:
            self._preview_2d.set_draw_limit(value)

    def _update_seekbar_max(self):
        n = sum(len(g.paths) for g in self._display_groups)
        self._seekbar.blockSignals(True)
        self._seekbar.setRange(0, n)
        self._seekbar.setValue(n)
        self._seekbar.blockSignals(False)

    # ═══════════════════════════════════════════════════ file loading

    def _on_open(self):
        paths, _ = QFileDialog.getOpenFileNames(
            self, "ファイルを開く", "",
            "ベクター/画像 (*.svg *.dxf *.png *.jpg *.jpeg);;すべて (*)")
        if not paths:
            return
        self._layers         = []
        self._raw_image_path = ""
        for path in paths:
            self._load_file_append(path)
        if self._active_source_groups() or self._raw_image_path:
            self._finalize_load(paths)

    def _load_file_append(self, filepath: str):
        ext = os.path.splitext(filepath)[1].lower()
        name = os.path.basename(filepath)
        # Raster: defer extraction until Generate
        if ext in (".png", ".jpg", ".jpeg"):
            self._raw_image_path = filepath
            self._layers.append({'name': name, 'filepath': filepath,
                                 'groups': [], 'visible': True})
            return
        # Vector: extract immediately
        safe, is_tmp = _safe_filepath(filepath)
        try:
            precision = self.settings.path.curve_precision
            if ext == ".svg":
                from src.core.importer.svg_importer import import_svg
                groups = import_svg(safe, precision)
            elif ext == ".dxf":
                from src.core.importer.dxf_importer import import_dxf
                groups = import_dxf(safe, precision)
            else:
                QMessageBox.warning(self, "非対応形式", f"非対応のファイル形式: {ext}")
                return
        except Exception as e:
            QMessageBox.critical(self, "読み込みエラー",
                                 f"{name}\n\n読み込みに失敗しました:\n{e}")
            return
        finally:
            if is_tmp and os.path.exists(safe):
                try: os.remove(safe)
                except OSError: pass
        self._layers.append({'name': name, 'filepath': filepath,
                             'groups': groups, 'visible': True})

    def _finalize_load(self, paths: List[str]):
        self._gcode_text     = None
        self._display_groups = []
        self._preview_dirty  = True
        self._raw_view_dirty = True
        self._path_list.set_layers(self._layers)

        if len(paths) == 1:
            name = os.path.basename(paths[0])
            self.setWindowTitle(f"Penplot-Gcoder — {name}")
            self._file_lbl.setText(name)
        else:
            names = ", ".join(os.path.basename(p) for p in paths[:3])
            if len(paths) > 3:
                names += f" 他{len(paths)-3}件"
            self.setWindowTitle(f"Penplot-Gcoder — {names}")
            self._file_lbl.setText(f"{len(paths)} ファイル")

        self._auto_fit()
        self._update_seekbar_max()
        self._update_status()
        self._set_view("raw")   # open in Raw, stay there until Generate

    def _auto_fit(self, groups: Optional[List[PathGroup]] = None):
        if groups is None:
            groups = self._active_source_groups()
        pts = np.array([pt for g in groups for p in g.paths for pt in p.points])
        if len(pts) == 0:
            return
        src_w = float(pts[:, 0].max() - pts[:, 0].min())
        src_h = float(pts[:, 1].max() - pts[:, 1].min())
        if src_w <= 0 and src_h <= 0:
            return
        x_min, y_min, x_max, y_max = self.settings.effective_area()
        eff_w, eff_h = x_max - x_min, y_max - y_min
        fit = min((eff_w / src_w if src_w else 1),
                  (eff_h / src_h if src_h else 1)) * 0.9 * 100
        cx = float((pts[:, 0].min() + pts[:, 0].max()) / 2)
        cy = float((pts[:, 1].min() + pts[:, 1].max()) / 2)
        tx = (x_min + x_max) / 2
        ty = (y_min + y_max) / 2
        self._applying = True
        pa = self.settings.path
        pa.scale    = fit
        pa.offset_x = tx - cx * fit / 100.0
        pa.offset_y = ty - cy * fit / 100.0
        pa.rotation = 0.0
        self._prev_snap = self._snap()
        self._applying  = False
        self._settings_panel._refresh_from_settings()
        self._sync_bottom_bar()

    # ═══════════════════════════════════════════════════ display refresh
    # Only called explicitly from _on_generate().

    def _refresh_display(self):
        """Full processing pipeline: fill + optimize + transform → display_groups."""
        source = self._active_source_groups()
        if not source:
            self._display_groups = []
            self._overflow = False
            self._update_seekbar_max()
            return

        pa = self.settings.path
        fi = self.settings.fill
        ordered = self._path_list.current_groups() or source

        # Groups from draw layers are already in bed-mm coordinates; no transform needed
        draw_group_ids = {
            id(g)
            for layer in self._layers if layer.get('is_draw', False)
            for g in layer['groups']
        }

        result: List[PathGroup] = []
        for grp in ordered:
            if id(grp) in draw_group_ids:
                # User-drawn paths: already at final bed coordinates
                new_paths = list(grp.paths)
            else:
                new_paths = [p.transformed(pa.scale / 100.0, pa.offset_x,
                                           pa.offset_y, pa.rotation)
                             for p in grp.paths]
            fills = generate_fills_for_paths(new_paths, fi) if fi.enabled else []
            if fi.layer_order == "outline_first":   combined = new_paths + fills
            elif fi.layer_order == "fill_first":    combined = fills + new_paths
            else:                                   combined = fills
            if pa.optimize and combined:
                combined = optimize(combined, pa.optimize_algorithm, pa.join_distance)
            result.append(PathGroup(color=grp.color, label=grp.label, paths=combined,
                                    pen_change_before=grp.pen_change_before))

        self._display_groups = result
        self._overflow = paths_overflow(
            [p for g in result for p in g.paths], self.settings)
        self._update_seekbar_max()

    # ═══════════════════════════════════════════════════ status / stats

    def _update_status(self):
        from src.core.gcode_generator import _estimate_time
        all_paths = [p for g in self._display_groups for p in g.paths]
        n = len(all_paths)
        if all_paths:
            total_mm, sec = _estimate_time([PathGroup(paths=all_paths)], self.settings)
            m, s = divmod(int(sec), 60)
            stats = f"パス: {n}  /  時間: {m}m{s:02d}s  /  描画: {total_mm:.0f}mm"
        else:
            stats = "パスなし"

        eff = self.settings.effective_area()
        ew, eh = eff[2] - eff[0], eff[3] - eff[1]
        overflow_str = "  ⚠ はみ出しあり！" if self._overflow else ""
        pending_str  = "  ⚙ Generate が必要" if self._preview_dirty else ""

        self._info_bar.setText(
            f"有効エリア: {ew:.1f}×{eh:.1f}mm  |  {stats}{overflow_str}{pending_str}")
        self._stats_box.setText(
            f"有効エリア\n{ew:.1f} × {eh:.1f} mm\n\n"
            f"パス数: {n}\n" +
            (f"推定時間: {m}m{s:02d}s\n描画距離: {total_mm:.0f}mm" if all_paths else "") +
            ("\n\n⚠ はみ出しあり" if self._overflow else "") +
            ("\n\n⚙ Generate を押してください" if self._preview_dirty else ""))
        self._status.showMessage(
            f"有効エリア: {ew:.1f}×{eh:.1f}mm  |  {stats}{overflow_str}{pending_str}")

        has_draw = any(l.get('is_draw', False) and l.get('groups')
                       for l in self._layers)
        has_content = bool(self._active_source_groups()) or bool(self._raw_image_path) or has_draw
        is_enabled  = not self._overflow and has_content
        self._btn_gen.setEnabled(is_enabled)
        if is_enabled and self._preview_dirty:
            self._start_pulse()
        else:
            self._stop_pulse()

    # ═══════════════════════════════════════════════════ bottom bar

    def _sync_bottom_bar(self):
        if self._applying:
            return
        self._applying = True
        pa = self.settings.path
        self._bot_scale.setValue(pa.scale)
        self._bot_x.setValue(pa.offset_x)
        self._bot_y.setValue(pa.offset_y)
        self._bot_rot.setValue(pa.rotation)
        bbox = self._preview_2d._compute_bbox()
        if bbox:
            x0, y0, x1, y1 = bbox
            self._bot_wh.setText(f"W: {x1-x0:.1f}  H: {y1-y0:.1f} mm")
        else:
            self._bot_wh.setText("W: --  H: -- mm")
        self._applying = False

    def _on_bottom_spin(self):
        if self._applying:
            return
        old = self._snap()
        self._applying = True
        pa = self.settings.path
        pa.scale    = self._bot_scale.value()
        pa.offset_x = self._bot_x.value()
        pa.offset_y = self._bot_y.value()
        pa.rotation = self._bot_rot.value()
        self._applying = False
        new = self._snap()
        if new != old:
            self._push_undo(old, new)
        self._settings_panel._refresh_from_settings()
        self._mark_preview_dirty()
        self._sync_bottom_bar()

    # ═══════════════════════════════════════════════════ signals

    def _on_settings_changed(self):
        if self._applying:
            return
        new = self._snap()
        if new != self._prev_snap:
            self._push_undo(self._prev_snap, new)
        self._sync_bottom_bar()
        self._mark_preview_dirty()
        self._autosave_timer.start(1000)

    def _on_cursor_moved(self, x: float, y: float):
        self._coord_lbl.setText(f"X: {x:8.2f}  Y: {y:8.2f} mm")

    def _on_preview_placement(self, ox: float, oy: float, scale: float):
        """Drag-in-canvas placement update — only active in Raw tab."""
        if self._applying or self._view_mode != "raw":
            return
        old = self._snap()
        self._applying = True
        pa = self.settings.path
        pa.offset_x = ox; pa.offset_y = oy; pa.scale = scale
        self._applying = False
        self._settings_panel._refresh_from_settings()
        self._mark_preview_dirty()
        self._sync_bottom_bar()
        new = self._snap()
        if new != old:
            self._push_undo(old, new)

    def _on_center_clicked(self):
        """Center placed content on the effective area."""
        # Compute bbox from whatever Preview2D is currently showing
        bbox = self._preview_2d._compute_bbox()
        if not bbox:
            return
        x0, y0, x1, y1 = bbox
        dx = (x0 + x1) / 2
        dy = (y0 + y1) / 2
        x_min, y_min, x_max, y_max = self.settings.effective_area()
        old = self._snap()
        pa  = self.settings.path
        pa.offset_x += (x_min + x_max) / 2 - dx
        pa.offset_y += (y_min + y_max) / 2 - dy
        new = self._snap()
        self._push_undo(old, new)
        self._settings_panel._refresh_from_settings()
        self._mark_preview_dirty()
        self._sync_bottom_bar()

    def _on_path_order_changed(self):
        """Path list reorder / pen-change toggle → Preview stale."""
        self._mark_preview_dirty()

    def _on_raw_groups_changed(self, groups: list):
        """Raw-view edited source data (vector path delete). Update the image layer."""
        img_layer = next((l for l in self._layers
                          if l['filepath'] == self._raw_image_path), None)
        if img_layer is not None:
            img_layer['groups'] = groups
        self._raw_view_dirty = False
        self._path_list.set_layers(self._layers)
        self._auto_fit()
        self._mark_preview_dirty()

    def _on_layer_visibility_changed(self):
        """Layer visibility toggle in path list — refresh raw canvas and mark dirty."""
        if self._view_mode == "raw" and not self._is_image_mode():
            self._preview_2d.set_groups(self._raw_placed_groups())
        self._mark_preview_dirty()

    def _on_img_params_toggled(self, show_params: bool):
        """Toggle between placement view (Preview2D+bg) and GIMP extraction params."""
        self._raw_show_params = show_params
        self._set_view("raw")

    def _on_tool_changed(self, tool: str):
        """Raw toolbar tool selection."""
        self._preview_2d.set_active_tool(tool)

    def _on_path_drawn(self, pen_path):
        """User drew a stroke in Preview2D — add it to the active draw layer."""
        from src.models.pen_path import PathGroup as PG
        idx = self._path_list.active_layer_index()
        # Find or create a draw layer
        draw_layer = None
        if 0 <= idx < len(self._layers) and self._layers[idx].get('is_draw', False):
            draw_layer = self._layers[idx]
        else:
            # Look for any existing draw layer
            for l in self._layers:
                if l.get('is_draw', False):
                    draw_layer = l
                    break
        if draw_layer is None:
            # Create a new draw layer
            draw_layer = {'name': '描画レイヤー', 'filepath': '',
                          'groups': [], 'visible': True, 'is_draw': True}
            self._layers.append(draw_layer)
            new_idx = len(self._layers) - 1
            self._path_list.set_layers(self._layers)
            self._path_list.set_active_layer(new_idx)
        # Add the path as a new group (or extend existing draw group with same color)
        target_grp = None
        for g in draw_layer['groups']:
            if g.color == pen_path.color:
                target_grp = g
                break
        if target_grp is None:
            target_grp = PG(color=pen_path.color, label=f"Draw ({pen_path.color})")
            draw_layer['groups'].append(target_grp)
        target_grp.paths.append(pen_path)
        self._path_list.set_layers(self._layers)
        self._mark_preview_dirty()

    def _on_add_draw_layer(self):
        """Add a new empty draw layer."""
        n = sum(1 for l in self._layers if l.get('is_draw', False)) + 1
        new_layer = {'name': f'描画レイヤー {n}', 'filepath': '',
                     'groups': [], 'visible': True, 'is_draw': True}
        self._layers.append(new_layer)
        new_idx = len(self._layers) - 1
        self._path_list.set_layers(self._layers)
        self._path_list.set_active_layer(new_idx)

    def _on_delete_layer(self, idx: int):
        """Delete a draw layer."""
        if 0 <= idx < len(self._layers) and self._layers[idx].get('is_draw', False):
            self._layers.pop(idx)
            new_active = max(0, idx - 1)
            self._path_list.set_layers(self._layers)
            if self._layers:
                self._path_list.set_active_layer(new_active)
            self._mark_preview_dirty()

    # ═══════════════════════════════════════════════════ Generate

    def _extract_image_now(self) -> bool:
        """Convert the pending raster image to vector groups. Returns True on success."""
        if not self._raw_image_path:
            return False
        self._status.showMessage("画像から輪郭を抽出中...", 0)
        QApplication.processEvents()

        groups = (self._raw_view.get_current_groups()
                  if self._raw_view._is_image else [])

        if not groups:
            safe, is_tmp = _safe_filepath(self._raw_image_path)
            try:
                from src.core.importer.image_importer import import_image
                groups = import_image(safe)
            except Exception as e:
                QMessageBox.critical(self, "抽出エラー", f"輪郭抽出に失敗しました:\n{e}")
                return False
            finally:
                if is_tmp and os.path.exists(safe):
                    try: os.remove(safe)
                    except OSError: pass

        if not groups:
            QMessageBox.warning(self, "輪郭未検出",
                                "輪郭が検出されませんでした。\n"
                                "Raw ビューでパラメーターを調整してください。")
            return False

        # Write extracted groups back to the image layer
        img_layer = next((l for l in self._layers
                          if l['filepath'] == self._raw_image_path), None)
        if img_layer is not None:
            img_layer['groups'] = groups
        else:
            self._layers.append({'name': os.path.basename(self._raw_image_path),
                                 'filepath': self._raw_image_path,
                                 'groups': groups, 'visible': True})
        self._raw_view_dirty = False
        self._path_list.set_layers(self._layers)
        self._auto_fit()
        self._status.showMessage(
            f"抽出完了: {sum(len(g.paths) for g in groups)} パス", 3000)
        return True

    def _on_generate(self):
        if self._overflow:
            QMessageBox.warning(self, "はみ出しエラー",
                                "パスが有効描画エリアをはみ出しています。\n"
                                "配置を調整してください。")
            return
        # 1. Extract raster image if pending (may have draw layers on top)
        if self._raw_image_path:
            img_layer = next((l for l in self._layers
                              if l['filepath'] == self._raw_image_path), None)
            if img_layer is not None and not img_layer.get('groups'):
                if not self._extract_image_now():
                    return
        if not self._active_source_groups():
            QMessageBox.information(self, "未読み込み",
                                    "先にファイルを開くか、描画レイヤーに図形を描いてください。")
            return
        # 2. Full processing pipeline
        self._refresh_display()
        if not self._display_groups and not self._overflow:
            QMessageBox.warning(self, "生成失敗", "処理後のパスが空です。設定を確認してください。")
            return
        # 3. Generate G-code
        try:
            src = (os.path.basename(self._source_files[0])
                   if self._source_files else "")
            self._gcode_text = generate_gcode(self._display_groups, self.settings, src)
        except Exception as e:
            QMessageBox.critical(self, "生成エラー", f"G-code 生成に失敗しました:\n{e}")
            return
        # 4. Mark Preview fresh, switch to Preview tab, stop pulse
        self._preview_dirty = False
        self._stop_pulse()
        self._set_view("transformed")   # ← auto-switch to Preview
        self._update_status()
        lines = len(self._gcode_text.splitlines())
        self._status.showMessage(f"G-code 生成完了: {lines} 行", 5000)

    def _on_save(self):
        if not self._gcode_text:
            self._on_generate()
        if not self._gcode_text:
            return
        default = (os.path.splitext(self._source_files[0])[0] + ".gcode"
                   if self._source_files else "output.gcode")
        path, _ = QFileDialog.getSaveFileName(
            self, "G-code を保存", default,
            "G-code (*.gcode *.nc *.txt);;すべて (*)")
        if not path:
            return
        try:
            with open(path, "w", encoding="utf-8") as f:
                f.write(self._gcode_text)
            self._status.showMessage(f"保存しました: {path}", 5000)
        except Exception as e:
            QMessageBox.critical(self, "保存エラー", f"保存に失敗しました:\n{e}")

    def _on_copy(self):
        if not self._gcode_text:
            self._on_generate()
        if not self._gcode_text:
            return
        QApplication.clipboard().setText(self._gcode_text)
        self._status.showMessage("クリップボードにコピーしました", 3000)

    # ═══════════════════════════════════════════════════ settings persistence

    def _on_save_settings(self):
        path, _ = QFileDialog.getSaveFileName(
            self, "設定を保存", "", "JSON (*.json);;すべて (*)")
        if path:
            self.settings.to_json(path)
            self._status.showMessage(f"設定を保存: {path}", 3000)

    def _on_load_settings(self):
        path, _ = QFileDialog.getOpenFileName(
            self, "設定を読み込み", "", "JSON (*.json);;すべて (*)")
        if path:
            try:
                loaded = AppSettings.from_json(path)
                self.settings.machine = loaded.machine
                self.settings.pen     = loaded.pen
                self.settings.speed   = loaded.speed
                self.settings.path    = loaded.path
                self.settings.fill    = loaded.fill
                self.settings.gcode   = loaded.gcode
                self._settings_panel._refresh_from_settings()
                self._mark_preview_dirty()
                self._status.showMessage(f"設定を読み込み: {path}", 3000)
            except Exception as e:
                QMessageBox.critical(self, "エラー", f"読み込みに失敗しました:\n{e}")

    # ═══════════════════════════════════════════════════ auto-save

    def _autosave_path(self) -> str:
        d = os.path.normpath(os.path.join(
            os.path.dirname(os.path.abspath(__file__)),
            "..", "..", "profiles", "user"))
        os.makedirs(d, exist_ok=True)
        return os.path.join(d, "autosave.json")

    def _load_autosave(self):
        path = self._autosave_path()
        if not os.path.exists(path):
            return
        try:
            loaded = AppSettings.from_json(path)
            self.settings.machine = loaded.machine
            self.settings.pen     = loaded.pen
            self.settings.speed   = loaded.speed
            self.settings.fill    = loaded.fill
            self.settings.gcode   = loaded.gcode
        except Exception:
            pass

    def _do_autosave(self):
        try:
            self.settings.to_json(self._autosave_path())
        except Exception:
            pass

    # ═══════════════════════════════════════════════════ undo helpers

    def _snap(self):
        pa = self.settings.path
        return (pa.scale, pa.offset_x, pa.offset_y, pa.rotation)

    def _push_undo(self, old, new):
        self._prev_snap = new
        cmd = PlacementCommand(self, old, new)
        self._undo_stack.blockSignals(True)
        self._undo_stack.push(cmd)
        self._undo_stack.blockSignals(False)

    def _on_about(self):
        QMessageBox.about(
            self, "Penplot-Gcoder について",
            "Penplot-Gcoder\n\n"
            "SVG / DXF / PNG を3Dプリンター用ペンプロッター G-code に変換\n\n"
            "Python / PyQt6 / svgpathtools / ezdxf / OpenCV / Shapely / pyqtgraph")


# ---------------------------------------------------------------------------
def _ma(menu, label, shortcut, slot):
    a = QAction(label, menu.parent())
    if shortcut: a.setShortcut(shortcut)
    a.triggered.connect(slot)
    menu.addAction(a)
