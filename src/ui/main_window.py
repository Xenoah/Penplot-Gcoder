"""Main application window — single-screen slicer layout."""
from __future__ import annotations
import os
import re
import shutil
import subprocess
import tempfile
from typing import List, Optional

import numpy as np

from PyQt6.QtCore import Qt, QTimer
from PyQt6.QtGui import QAction, QKeySequence, QUndoStack, QUndoCommand, QFont, QColor
from PyQt6.QtWidgets import (
    QMainWindow, QWidget, QVBoxLayout, QHBoxLayout, QFormLayout,
    QPushButton, QLabel, QStatusBar, QFileDialog,
    QMessageBox, QSplitter, QDoubleSpinBox, QSlider, QFrame,
    QGroupBox, QTextEdit, QSizePolicy, QApplication,
    QButtonGroup, QRadioButton,
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
# File-path safety: copy to ASCII temp if path contains non-ASCII / symbols
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
        self._win._refresh_display()

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

    def __init__(self):
        super().__init__()
        self.settings   = AppSettings()
        self._load_autosave()              # restore last session settings

        self._source_groups:  List[PathGroup] = []
        self._display_groups: List[PathGroup] = []
        self._source_files: List[str] = []
        self._overflow    = False
        self._gcode_text: Optional[str] = None
        self._applying    = False          # signal-loop guard
        self._view_mode   = "transformed"  # "raw" | "transformed" | "3d"

        self._undo_stack  = QUndoStack(self)
        self._prev_snap   = self._snap()

        # ── Shared child widgets (single Qt parent ownership) ──────────────
        self._preview_2d     = Preview2D(self.settings)
        self._preview_3d     = Preview3D(self.settings)
        self._path_list      = PathListPanel()
        self._settings_panel = SettingsPanel(self.settings)

        self._preview_2d.cursor_moved.connect(self._on_cursor_moved)
        self._preview_2d.placement_changed.connect(self._on_preview_placement)
        self._settings_panel.settings_changed.connect(self._on_settings_changed)
        self._settings_panel.center_clicked.connect(self._on_center_clicked)
        self._path_list.order_changed.connect(self._refresh_display)
        self._path_list.pen_change_changed.connect(self._refresh_display)

        # ── Timers ────────────────────────────────────────────────────────
        self._play_timer = QTimer(self)
        self._play_timer.setInterval(80)
        self._play_timer.timeout.connect(self._play_step)

        self._autosave_timer = QTimer(self)
        self._autosave_timer.setSingleShot(True)
        self._autosave_timer.timeout.connect(self._do_autosave)

        self.setWindowTitle("Penplot-Gcoder")
        self.resize(1400, 860)
        self._build_ui()
        self._build_menu()
        self._update_status()

    # ═══════════════════════════════════════════════════ layout

    def _build_ui(self):
        root = QWidget()
        self.setCentralWidget(root)
        vroot = QVBoxLayout(root)
        vroot.setContentsMargins(0, 0, 0, 0)
        vroot.setSpacing(0)

        # ── Main 3-column splitter ─────────────────────────────────────────
        splitter = QSplitter(Qt.Orientation.Horizontal)
        vroot.addWidget(splitter, 1)

        # Left: Settings panel
        left_wrap = QWidget()
        left_wrap.setMinimumWidth(260)
        left_wrap.setMaximumWidth(320)
        lw = QVBoxLayout(left_wrap)
        lw.setContentsMargins(4, 4, 2, 4)
        lw.addWidget(self._settings_panel)
        splitter.addWidget(left_wrap)

        # Centre: Preview + toolbar
        centre = QWidget()
        cvl = QVBoxLayout(centre)
        cvl.setContentsMargins(2, 4, 2, 2)
        cvl.setSpacing(2)

        # Preview toolbar
        ptb = QHBoxLayout()

        # View-mode buttons
        self._btn_raw   = QPushButton("Raw")
        self._btn_trans = QPushButton("変換後")
        self._btn_3d    = QPushButton("3D")
        for b in (self._btn_raw, self._btn_trans, self._btn_3d):
            b.setCheckable(True)
            b.setFixedHeight(26)
        self._btn_trans.setChecked(True)
        self._btn_raw.clicked.connect(lambda: self._set_view("raw"))
        self._btn_trans.clicked.connect(lambda: self._set_view("transformed"))
        self._btn_3d.clicked.connect(lambda: self._set_view("3d"))
        ptb.addWidget(self._btn_raw)
        ptb.addWidget(self._btn_trans)
        ptb.addWidget(self._btn_3d)

        ptb.addWidget(_sep())

        # Play / Pause button
        self._btn_play = QPushButton("▶")
        self._btn_play.setFixedSize(30, 26)
        self._btn_play.setCheckable(True)
        self._btn_play.clicked.connect(self._on_play_toggled)
        ptb.addWidget(self._btn_play)

        # Seekbar — takes remaining horizontal space
        self._seekbar = QSlider(Qt.Orientation.Horizontal)
        self._seekbar.setRange(0, 0)
        self._seekbar.valueChanged.connect(self._on_seekbar_changed)
        ptb.addWidget(self._seekbar, 1)

        # Cursor coordinate (top-right)
        self._coord_lbl = QLabel("X: ---.--  Y: ---.-- mm")
        self._coord_lbl.setStyleSheet("font-family: monospace; color: #aaa; font-size: 11px;")
        ptb.addWidget(self._coord_lbl)
        cvl.addLayout(ptb)

        # Preview area (stacked 2D / 3D)
        self._preview_3d.setVisible(False)
        cvl.addWidget(self._preview_2d, 1)
        cvl.addWidget(self._preview_3d, 1)

        # Overflow / info bar
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

        # ── Bottom bar: placement + actions ───────────────────────────────
        vroot.addWidget(self._build_bottom_bar())

        # Status bar
        self._status = QStatusBar()
        self.setStatusBar(self._status)

    def _build_bottom_bar(self) -> QWidget:
        bar = QWidget()
        bar.setFixedHeight(42)
        bar.setStyleSheet("background: #252525; border-top: 1px solid #444;")
        hl = QHBoxLayout(bar)
        hl.setContentsMargins(8, 4, 8, 4)
        hl.setSpacing(8)

        # File actions
        btn_open = QPushButton("📂 開く")
        btn_open.setFixedHeight(30)
        btn_open.clicked.connect(self._on_open)
        hl.addWidget(btn_open)

        self._file_lbl = QLabel("ファイル未選択")
        self._file_lbl.setStyleSheet("color:#888; font-size:11px;")
        self._file_lbl.setMaximumWidth(220)
        hl.addWidget(self._file_lbl)

        hl.addWidget(_sep())

        # Placement controls (inline)
        hl.addWidget(QLabel("Scale:"))
        self._bot_scale = _dspin(0.1, 10000, 1, 1.0, " %")
        self._bot_scale.setFixedWidth(80)
        hl.addWidget(self._bot_scale)

        hl.addWidget(QLabel("X:"))
        self._bot_x = _dspin(-9999, 9999, 2, 0.5, " mm")
        self._bot_x.setFixedWidth(90)
        hl.addWidget(self._bot_x)

        hl.addWidget(QLabel("Y:"))
        self._bot_y = _dspin(-9999, 9999, 2, 0.5, " mm")
        self._bot_y.setFixedWidth(90)
        hl.addWidget(self._bot_y)

        hl.addWidget(QLabel("R:"))
        self._bot_rot = _dspin(-360, 360, 1, 1.0, "°")
        self._bot_rot.setFixedWidth(75)
        hl.addWidget(self._bot_rot)

        btn_center = QPushButton("⊕ 中央")
        btn_center.setFixedHeight(28)
        btn_center.clicked.connect(self._on_center_clicked)
        hl.addWidget(btn_center)

        hl.addWidget(_sep())

        # W / H display
        self._bot_wh = QLabel("W: --  H: --")
        self._bot_wh.setStyleSheet("color:#aaa; font-size:11px; font-family:monospace;")
        hl.addWidget(self._bot_wh)

        hl.addStretch()

        # Main action buttons
        self._btn_gen  = QPushButton("⚙ G-code 生成")
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

        # Connect bottom bar spinboxes
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

    def _set_view(self, mode: str):
        self._view_mode = mode
        self._btn_raw.setChecked(mode == "raw")
        self._btn_trans.setChecked(mode == "transformed")
        self._btn_3d.setChecked(mode == "3d")
        self._preview_2d.setVisible(mode != "3d")
        self._preview_3d.setVisible(mode == "3d")
        self._push_to_preview()

    def _push_to_preview(self):
        """Feed the appropriate groups to the visible preview."""
        if self._view_mode == "raw":
            # Apply scale-only transform so source coordinates become mm-scale
            sc = self.settings.path.scale / 100.0
            raw_scaled = []
            for grp in self._source_groups:
                scaled_paths = [p.transformed(sc, 0.0, 0.0, 0.0) for p in grp.paths]
                raw_scaled.append(PathGroup(
                    color=grp.color, label=grp.label,
                    paths=scaled_paths, pen_change_before=grp.pen_change_before))
            self._preview_2d.set_groups(raw_scaled)
            self._preview_2d.set_overflow(False)
        elif self._view_mode == "transformed":
            self._preview_2d.set_groups(self._display_groups)
            self._preview_2d.set_overflow(self._overflow)
        else:  # 3d
            self._preview_3d.set_groups(self._display_groups)

    # ═══════════════════════════════════════════════════ play / seekbar

    def _on_play_toggled(self, checked: bool):
        if checked:
            # If at end, reset to start
            if self._seekbar.value() >= self._seekbar.maximum():
                self._seekbar.setValue(0)
            self._play_timer.start()
            self._btn_play.setText("⏸")
        else:
            self._play_timer.stop()
            self._btn_play.setText("▶")

    def _play_step(self):
        cur = self._seekbar.value()
        mx  = self._seekbar.maximum()
        if cur < mx:
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
        all_paths = [p for g in self._display_groups for p in g.paths]
        n = len(all_paths)
        self._seekbar.blockSignals(True)
        self._seekbar.setRange(0, n)
        self._seekbar.setValue(n)   # default: show all
        self._seekbar.blockSignals(False)

    # ═══════════════════════════════════════════════════ file loading

    def _on_open(self):
        paths, _ = QFileDialog.getOpenFileNames(
            self, "ファイルを開く", "",
            "ベクター/画像 (*.svg *.dxf *.png *.jpg *.jpeg);;すべて (*)")
        if not paths:
            return
        # Clear previous content, load all selected files
        self._source_groups = []
        self._source_files  = []
        for path in paths:
            self._load_file_append(path)
        if self._source_groups:
            self._finalize_load(paths)

    def _load_file_append(self, filepath: str):
        """Load a single file and append its groups to _source_groups."""
        ext = os.path.splitext(filepath)[1].lower()
        safe, is_tmp = _safe_filepath(filepath)
        try:
            precision = self.settings.path.curve_precision
            if ext == ".svg":
                from src.core.importer.svg_importer import import_svg
                groups = import_svg(safe, precision)
            elif ext == ".dxf":
                from src.core.importer.dxf_importer import import_dxf
                groups = import_dxf(safe, precision)
            elif ext in (".png", ".jpg", ".jpeg"):
                from src.core.importer.image_importer import import_image
                groups = import_image(safe)
            else:
                QMessageBox.warning(self, "非対応形式", f"非対応のファイル形式: {ext}")
                return
        except Exception as e:
            QMessageBox.critical(self, "読み込みエラー",
                                 f"{os.path.basename(filepath)}\n\n読み込みに失敗しました:\n{e}")
            return
        finally:
            if is_tmp and os.path.exists(safe):
                try: os.remove(safe)
                except OSError: pass
        self._source_groups.extend(groups)
        self._source_files.append(filepath)

    def _finalize_load(self, paths: List[str]):
        self._gcode_text = None
        self._path_list.set_groups(list(self._source_groups))

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

        self._auto_fit(self._source_groups)
        self._refresh_display()
        self._set_view("raw")
        QTimer.singleShot(600, lambda: self._set_view("transformed"))

    def _auto_fit(self, groups: List[PathGroup]):
        pts = np.array([pt for g in groups for p in g.paths for pt in p.points])
        if len(pts) == 0:
            return
        src_w = float(pts[:, 0].max() - pts[:, 0].min())
        src_h = float(pts[:, 1].max() - pts[:, 1].min())
        if src_w <= 0 and src_h <= 0:
            return
        x_min, y_min, x_max, y_max = self.settings.effective_area()
        eff_w, eff_h = x_max - x_min, y_max - y_min
        fit = min((eff_w / src_w if src_w else 1), (eff_h / src_h if src_h else 1)) * 0.9 * 100
        cx = float((pts[:, 0].min() + pts[:, 0].max()) / 2)
        cy = float((pts[:, 1].min() + pts[:, 1].max()) / 2)
        tx, ty = (x_min + x_max) / 2, (y_min + y_max) / 2
        self._applying = True
        pa = self.settings.path
        pa.scale = fit
        pa.offset_x = tx - cx * fit / 100.0
        pa.offset_y = ty - cy * fit / 100.0
        pa.rotation = 0.0
        self._prev_snap = self._snap()
        self._applying = False
        self._settings_panel._refresh_from_settings()
        self._sync_bottom_bar()

    # ═══════════════════════════════════════════════════ display refresh

    def _refresh_display(self):
        if not self._source_groups:
            self._display_groups = []
            self._overflow = False
            self._push_to_preview()
            self._update_status()
            self._update_seekbar_max()
            return

        pa = self.settings.path
        fi = self.settings.fill
        ordered = (self._path_list.current_groups()
                   if self._path_list.groups else self._source_groups)
        result: List[PathGroup] = []
        for grp in ordered:
            new_paths = [p.transformed(pa.scale / 100.0, pa.offset_x,
                                       pa.offset_y, pa.rotation)
                         for p in grp.paths]
            fills = generate_fills_for_paths(new_paths, fi) if fi.enabled else []
            if fi.layer_order == "outline_first":   combined = new_paths + fills
            elif fi.layer_order == "fill_first":    combined = fills + new_paths
            else:                                    combined = fills
            if pa.optimize and combined:
                combined = optimize(combined, pa.optimize_algorithm, pa.join_distance)
            result.append(PathGroup(color=grp.color, label=grp.label, paths=combined,
                                    pen_change_before=grp.pen_change_before))

        self._display_groups = result
        all_paths = [p for g in result for p in g.paths]
        self._overflow = paths_overflow(all_paths, self.settings)
        self._update_seekbar_max()
        self._push_to_preview()
        self._update_status()
        self._sync_bottom_bar()

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
        overflow = "  ⚠ はみ出しあり！" if self._overflow else ""
        self._info_bar.setText(
            f"有効エリア: {ew:.1f}×{eh:.1f}mm  |  {stats}{overflow}")
        self._stats_box.setText(
            f"有効エリア\n{ew:.1f} × {eh:.1f} mm\n\n"
            f"パス数: {n}\n" +
            (f"推定時間: {m}m{s:02d}s\n描画距離: {total_mm:.0f}mm"
             if all_paths else "") +
            ("\n\n⚠ はみ出しあり" if self._overflow else ""))
        self._status.showMessage(
            f"有効エリア: {ew:.1f}×{eh:.1f}mm  |  {stats}{overflow}")
        self._btn_gen.setEnabled(not self._overflow and bool(self._source_groups))

    # ═══════════════════════════════════════════════════ bottom bar sync

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
        self._refresh_display()

    # ═══════════════════════════════════════════════════ signals

    def _on_settings_changed(self):
        if self._applying:
            return
        new = self._snap()
        if new != self._prev_snap:
            self._push_undo(self._prev_snap, new)
        self._sync_bottom_bar()
        self._refresh_display()
        self._autosave_timer.start(1000)   # debounce 1 s

    def _on_cursor_moved(self, x: float, y: float):
        self._coord_lbl.setText(f"X: {x:8.2f}  Y: {y:8.2f} mm")

    def _on_preview_placement(self, ox: float, oy: float, scale: float):
        if self._applying:
            return
        old = self._snap()
        self._applying = True
        pa = self.settings.path
        pa.offset_x = ox; pa.offset_y = oy; pa.scale = scale
        self._applying = False
        self._settings_panel._refresh_from_settings()
        self._refresh_display()
        new = self._snap()
        if new != old:
            self._push_undo(old, new)

    def _on_center_clicked(self):
        if not self._display_groups:
            return
        pts_list = [p.np_points for g in self._display_groups
                    for p in g.paths if len(p.np_points)]
        if not pts_list:
            return
        all_pts = np.concatenate(pts_list)
        dx = float((all_pts[:, 0].min() + all_pts[:, 0].max()) / 2)
        dy = float((all_pts[:, 1].min() + all_pts[:, 1].max()) / 2)
        x_min, y_min, x_max, y_max = self.settings.effective_area()
        old = self._snap()
        pa = self.settings.path
        pa.offset_x += (x_min + x_max) / 2 - dx
        pa.offset_y += (y_min + y_max) / 2 - dy
        new = self._snap()
        self._push_undo(old, new)
        self._settings_panel._refresh_from_settings()
        self._refresh_display()

    # ═══════════════════════════════════════════════════ G-code

    def _on_generate(self):
        if self._overflow:
            QMessageBox.warning(self, "はみ出しエラー",
                                "パスが有効描画エリアをはみ出しています。\n配置を調整してください。")
            return
        if not self._source_groups:
            QMessageBox.information(self, "未読み込み", "先にファイルを開いてください。")
            return
        try:
            src = (os.path.basename(self._source_files[0])
                   if self._source_files else "")
            self._gcode_text = generate_gcode(self._display_groups, self.settings, src)
        except Exception as e:
            QMessageBox.critical(self, "生成エラー", f"G-code 生成に失敗しました:\n{e}")
            return
        lines = len(self._gcode_text.splitlines())
        QMessageBox.information(
            self, "生成完了",
            f"G-code を生成しました。\n{lines} 行  /  {len(self._gcode_text)} 文字\n\n"
            "「💾 保存」または「📋 コピー」で出力してください。")
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
                self.settings.pen = loaded.pen
                self.settings.speed = loaded.speed
                self.settings.path = loaded.path
                self.settings.fill = loaded.fill
                self.settings.gcode = loaded.gcode
                self._settings_panel._refresh_from_settings()
                self._on_settings_changed()
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
            # Restore machine/pen/speed/fill/gcode; keep path at defaults
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
