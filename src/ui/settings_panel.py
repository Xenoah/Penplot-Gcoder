"""Left settings panel with tabs: Machine / Pen / Speed / Path / Fill / GCode."""
from __future__ import annotations
import glob
import json
import os
import re
from typing import Optional

from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QFormLayout,
    QLabel, QDoubleSpinBox, QSpinBox, QComboBox, QCheckBox,
    QPushButton, QTextEdit, QGroupBox, QTabWidget, QSizePolicy,
    QInputDialog, QMessageBox,
)
from src.models.settings import AppSettings


def _spin(min_val, max_val, decimals=2, step=0.1, suffix="") -> QDoubleSpinBox:
    w = QDoubleSpinBox()
    w.setRange(min_val, max_val)
    w.setDecimals(decimals)
    w.setSingleStep(step)
    if suffix:
        w.setSuffix(suffix)
    return w


def _ispin(min_val, max_val, suffix="") -> QSpinBox:
    w = QSpinBox()
    w.setRange(min_val, max_val)
    if suffix:
        w.setSuffix(suffix)
    return w


class SettingsPanel(QWidget):
    """Tab-based settings panel that emits settings_changed whenever a value changes."""

    settings_changed = pyqtSignal()   # emitted on any change
    center_clicked = pyqtSignal()     # center-placement button

    def __init__(self, settings: AppSettings, parent=None):
        super().__init__(parent)
        self.settings = settings
        self._updating = False   # prevent recursive signal loops

        self._tabs = QTabWidget()
        self._tabs.addTab(self._build_machine_tab(), "Machine")
        self._tabs.addTab(self._build_pen_tab(), "Pen")
        self._tabs.addTab(self._build_speed_tab(), "Speed")
        self._tabs.addTab(self._build_path_tab(), "Path")
        self._tabs.addTab(self._build_fill_tab(), "Fill")
        self._tabs.addTab(self._build_gcode_tab(), "GCode")
        # All widgets built — safe to call now
        self._on_fill_pattern_changed()

        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 4, 4, 4)
        layout.addWidget(self._tabs)
        self.setMinimumWidth(260)
        self.setMaximumWidth(340)

        self._load_printer_profiles()
        self._refresh_from_settings()

    # ------------------------------------------------------------------
    # Machine Tab
    # ------------------------------------------------------------------
    def _build_machine_tab(self) -> QWidget:
        w = QWidget()
        form = QFormLayout(w)
        form.setFieldGrowthPolicy(QFormLayout.FieldGrowthPolicy.AllNonFixedFieldsGrow)

        self._profile_combo = QComboBox()
        self._profile_combo.addItem("Custom")
        form.addRow("Printer Profile:", self._profile_combo)

        # Profile management buttons
        btn_row = QHBoxLayout()
        self._btn_new_profile  = QPushButton("New")
        self._btn_save_profile = QPushButton("Save")
        self._btn_del_profile  = QPushButton("Delete")
        for b in (self._btn_new_profile, self._btn_save_profile, self._btn_del_profile):
            b.setFixedHeight(22)
            btn_row.addWidget(b)
        profile_btn_widget = QWidget()
        profile_btn_widget.setLayout(btn_row)
        form.addRow("", profile_btn_widget)

        self._firmware_combo = QComboBox()
        self._firmware_combo.addItems(["Marlin", "Klipper"])
        form.addRow("Firmware:", self._firmware_combo)

        self._bed_x = _spin(10, 1000, 1, 1.0, " mm")
        self._bed_y = _spin(10, 1000, 1, 1.0, " mm")
        form.addRow("Bed X:", self._bed_x)
        form.addRow("Bed Y:", self._bed_y)

        self._origin_combo = QComboBox()
        self._origin_combo.addItems(["Left Bottom", "Center"])
        form.addRow("Origin:", self._origin_combo)

        self._eff_area_label = QLabel("—")
        self._eff_area_label.setStyleSheet("color: green; font-weight: bold;")
        form.addRow("Effective Area:", self._eff_area_label)

        # Signals
        self._profile_combo.currentIndexChanged.connect(self._on_profile_changed)
        self._btn_new_profile.clicked.connect(self._on_new_profile)
        self._btn_save_profile.clicked.connect(self._on_save_profile)
        self._btn_del_profile.clicked.connect(self._on_del_profile)
        self._firmware_combo.currentTextChanged.connect(self._on_any_changed)
        self._bed_x.valueChanged.connect(self._on_any_changed)
        self._bed_y.valueChanged.connect(self._on_any_changed)
        self._origin_combo.currentIndexChanged.connect(self._on_any_changed)
        return w

    # ------------------------------------------------------------------
    # Pen Tab
    # ------------------------------------------------------------------
    def _build_pen_tab(self) -> QWidget:
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(4, 4, 4, 4)

        grp_motion = QGroupBox("Pen Motion")
        f1 = QFormLayout(grp_motion)
        self._pen_down_z = _spin(-10, 50, 2, 0.1, " mm")
        self._pen_up_z = _spin(0, 50, 2, 0.1, " mm")
        self._pen_down_speed = _spin(1, 10000, 0, 10, " mm/min")
        self._pen_up_speed = _spin(1, 10000, 0, 10, " mm/min")
        self._touchdown_delay = _ispin(0, 10000, " ms")
        self._liftup_delay = _ispin(0, 10000, " ms")
        f1.addRow("Pen Down Z:", self._pen_down_z)
        f1.addRow("Pen Up Z:", self._pen_up_z)
        f1.addRow("Down Speed:", self._pen_down_speed)
        f1.addRow("Up Speed:", self._pen_up_speed)
        f1.addRow("Touchdown Delay:", self._touchdown_delay)
        f1.addRow("Liftup Delay:", self._liftup_delay)

        grp_offset = QGroupBox("Pen Offset (from nozzle)")
        f2 = QFormLayout(grp_offset)
        self._offset_x = _spin(-500, 500, 2, 0.5, " mm")
        self._offset_y = _spin(-500, 500, 2, 0.5, " mm")
        self._offset_z = _spin(-50, 50, 2, 0.1, " mm")
        f2.addRow("Offset X:", self._offset_x)
        f2.addRow("Offset Y:", self._offset_y)
        f2.addRow("Offset Z:", self._offset_z)

        layout.addWidget(grp_motion)
        layout.addWidget(grp_offset)
        layout.addStretch()

        for w_ in [self._pen_down_z, self._pen_up_z, self._pen_down_speed,
                   self._pen_up_speed, self._offset_x, self._offset_y, self._offset_z]:
            w_.valueChanged.connect(self._on_any_changed)
        self._touchdown_delay.valueChanged.connect(self._on_any_changed)
        self._liftup_delay.valueChanged.connect(self._on_any_changed)
        return w

    # ------------------------------------------------------------------
    # Speed Tab
    # ------------------------------------------------------------------
    def _build_speed_tab(self) -> QWidget:
        w = QWidget()
        form = QFormLayout(w)
        form.setFieldGrowthPolicy(QFormLayout.FieldGrowthPolicy.AllNonFixedFieldsGrow)

        self._draw_speed = _spin(1, 60000, 0, 100, " mm/min")
        self._travel_speed = _spin(1, 60000, 0, 100, " mm/min")
        self._acceleration = _spin(1, 10000, 0, 100, " mm/s²")
        self._corner_speed = _spin(0, 1000, 1, 0.5, " mm/s")

        form.addRow("Draw Speed:", self._draw_speed)
        form.addRow("Travel Speed:", self._travel_speed)
        form.addRow("Acceleration:", self._acceleration)
        form.addRow("Corner Speed:", self._corner_speed)

        for w_ in [self._draw_speed, self._travel_speed, self._acceleration, self._corner_speed]:
            w_.valueChanged.connect(self._on_any_changed)
        return w

    # ------------------------------------------------------------------
    # Path Tab
    # ------------------------------------------------------------------
    def _build_path_tab(self) -> QWidget:
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(4, 4, 4, 4)

        grp_place = QGroupBox("Placement")
        fp = QFormLayout(grp_place)
        self._scale = _spin(0.1, 10000, 1, 1.0, " %")
        self._path_offset_x = _spin(-10000, 10000, 2, 1.0, " mm")
        self._path_offset_y = _spin(-10000, 10000, 2, 1.0, " mm")
        self._rotation = _spin(-360, 360, 1, 1.0, "°")
        fp.addRow("Scale:", self._scale)
        fp.addRow("Offset X:", self._path_offset_x)
        fp.addRow("Offset Y:", self._path_offset_y)
        fp.addRow("Rotation:", self._rotation)

        self._center_btn = QPushButton("Center on Bed")
        self._center_btn.clicked.connect(self.center_clicked)
        fp.addRow("", self._center_btn)

        grp_opt = QGroupBox("Path Optimization")
        fo = QFormLayout(grp_opt)
        self._optimize_cb = QCheckBox()
        self._algo_combo = QComboBox()
        self._algo_combo.addItems(["Greedy", "2-opt"])
        self._join_dist = _spin(0, 100, 2, 0.1, " mm")
        self._curve_prec = _spin(0.01, 10, 3, 0.01, " mm")
        fo.addRow("Optimize Paths:", self._optimize_cb)
        fo.addRow("Algorithm:", self._algo_combo)
        fo.addRow("Join Distance:", self._join_dist)
        fo.addRow("Curve Precision:", self._curve_prec)

        layout.addWidget(grp_place)
        layout.addWidget(grp_opt)
        layout.addStretch()

        for w_ in [self._scale, self._path_offset_x, self._path_offset_y, self._rotation,
                   self._join_dist, self._curve_prec]:
            w_.valueChanged.connect(self._on_any_changed)
        self._optimize_cb.stateChanged.connect(self._on_any_changed)
        self._algo_combo.currentTextChanged.connect(self._on_any_changed)
        return w

    # ------------------------------------------------------------------
    # Fill Tab
    # ------------------------------------------------------------------
    def _build_fill_tab(self) -> QWidget:
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(4, 4, 4, 4)

        form = QFormLayout()
        self._fill_enabled = QCheckBox()
        self._fill_target = QComboBox()
        self._fill_target.addItems(["Fill color only", "All closed paths"])
        self._fill_layer_order = QComboBox()
        self._fill_layer_order.addItems(["Outline → Fill", "Fill → Outline", "Fill only"])
        self._fill_pattern = QComboBox()
        self._fill_pattern.addItems(["Lines", "Cross Hatch", "Tri Hatch",
                                     "Contour Offset", "Zigzag", "Dots", "Spiral"])
        self._fill_spacing = _spin(0.01, 100, 2, 0.1, " mm")
        self._fill_angle = _spin(-360, 360, 1, 1.0, "°")
        self._fill_margin = _spin(0, 100, 2, 0.1, " mm")

        form.addRow("Fill Enabled:", self._fill_enabled)
        form.addRow("Target:", self._fill_target)
        form.addRow("Layer Order:", self._fill_layer_order)
        form.addRow("Pattern:", self._fill_pattern)
        form.addRow("Spacing:", self._fill_spacing)
        form.addRow("Angle:", self._fill_angle)
        form.addRow("Margin:", self._fill_margin)

        # Pattern-specific
        self._crosshatch_angle2 = _spin(0, 360, 1, 1.0, "°")
        self._contour_innermost = QComboBox()
        self._contour_innermost.addItems(["Keep", "Skip", "Dots"])
        self._dots_grid = QComboBox()
        self._dots_grid.addItems(["Grid", "Stagger"])
        self._dots_spacing_y = _spin(0.01, 100, 2, 0.1, " mm")

        self._cross_row = self._make_form_row("2nd Angle Offset:", self._crosshatch_angle2)
        self._contour_row = self._make_form_row("Innermost Line:", self._contour_innermost)
        self._dots_grid_row = self._make_form_row("Dot Grid:", self._dots_grid)
        self._dots_sy_row = self._make_form_row("Spacing Y:", self._dots_spacing_y)

        form.addRow(self._cross_row)
        form.addRow(self._contour_row)
        form.addRow(self._dots_grid_row)
        form.addRow(self._dots_sy_row)

        layout.addLayout(form)
        layout.addStretch()

        # Signals
        self._fill_enabled.stateChanged.connect(self._on_fill_pattern_changed)
        self._fill_pattern.currentTextChanged.connect(self._on_fill_pattern_changed)
        for w_ in [self._fill_spacing, self._fill_angle, self._fill_margin,
                   self._crosshatch_angle2, self._dots_spacing_y]:
            w_.valueChanged.connect(self._on_any_changed)
        for cb in [self._fill_target, self._fill_layer_order, self._fill_pattern,
                   self._contour_innermost, self._dots_grid]:
            cb.currentTextChanged.connect(self._on_any_changed)
        return w

    def _make_form_row(self, label: str, widget: QWidget) -> QWidget:
        """Create a hidden-able form row container."""
        row = QWidget()
        hl = QHBoxLayout(row)
        hl.setContentsMargins(0, 0, 0, 0)
        hl.addWidget(QLabel(label))
        hl.addWidget(widget)
        return row

    def _on_fill_pattern_changed(self):
        pattern = self._fill_pattern.currentText()
        self._cross_row.setVisible(pattern == "Cross Hatch")
        self._contour_row.setVisible(pattern == "Contour Offset")
        self._dots_grid_row.setVisible(pattern == "Dots")
        self._dots_sy_row.setVisible(pattern == "Dots")
        self._on_any_changed()

    # ------------------------------------------------------------------
    # GCode Tab
    # ------------------------------------------------------------------
    def _build_gcode_tab(self) -> QWidget:
        w = QWidget()
        layout = QVBoxLayout(w)
        layout.setContentsMargins(4, 4, 4, 4)

        def labeled_editor(label: str) -> QTextEdit:
            layout.addWidget(QLabel(label))
            te = QTextEdit()
            te.setMaximumHeight(80)
            te.setFont(_mono_font())
            layout.addWidget(te)
            te.textChanged.connect(self._on_any_changed)
            return te

        self._start_code = labeled_editor("Start G-code:")
        self._end_code = labeled_editor("End G-code:")
        self._pen_up_code = labeled_editor("Pen Up code:")
        self._pen_down_code = labeled_editor("Pen Down code:")

        # Placeholder hint
        hint = QLabel(
            "Placeholders: {pen_down_z} {pen_up_z} {pen_down_speed} {pen_up_speed}\n"
            "{draw_speed} {travel_speed} {offset_x} {offset_y} {offset_z}\n"
            "{bed_x} {bed_y} {touchdown_delay} {liftup_delay}"
        )
        hint.setWordWrap(True)
        hint.setStyleSheet("color: gray; font-size: 10px;")
        layout.addWidget(hint)
        layout.addStretch()
        return w

    # ------------------------------------------------------------------
    # Printer profiles
    # ------------------------------------------------------------------
    def _get_profiles_dir(self) -> str:
        d = os.path.join(
            os.path.dirname(os.path.dirname(os.path.dirname(__file__))),
            "profiles", "printers"
        )
        os.makedirs(d, exist_ok=True)
        return d

    def _load_printer_profiles(self):
        profiles_dir = self._get_profiles_dir()
        self._profiles: dict = {}
        self._profile_paths: dict = {}
        for fn in glob.glob(os.path.join(profiles_dir, "*.json")):
            try:
                with open(fn, "r", encoding="utf-8") as f:
                    data = json.load(f)
                name = (data.get("name")
                        or data.get("machine", {}).get("profile_name")
                        or os.path.splitext(os.path.basename(fn))[0])
                self._profiles[name] = data
                self._profile_paths[name] = fn
                self._profile_combo.addItem(name)
            except Exception:
                pass

    def _on_profile_changed(self):
        name = self._profile_combo.currentText()
        if name == "Custom" or name not in self._profiles:
            return
        data = self._profiles[name]
        self._updating = True
        if "machine" in data:
            # Full AppSettings format — load all except path placement
            loaded = AppSettings.from_dict(data)
            self.settings.machine = loaded.machine
            self.settings.pen = loaded.pen
            self.settings.speed = loaded.speed
            self.settings.fill = loaded.fill
            self.settings.gcode = loaded.gcode
        else:
            # Legacy minimal format
            if "bed_x" in data:
                self.settings.machine.bed_x = float(data["bed_x"])
            if "bed_y" in data:
                self.settings.machine.bed_y = float(data["bed_y"])
            if "firmware" in data:
                self.settings.machine.firmware = data["firmware"]
            if "origin" in data:
                self.settings.machine.origin = data["origin"]
        self._updating = False
        self._refresh_from_settings()
        self._on_any_changed()

    def _on_new_profile(self):
        name, ok = QInputDialog.getText(self, "新しいプロファイル", "プロファイル名:")
        if not ok or not name.strip():
            return
        name = name.strip()
        self._write_to_settings()
        self.settings.machine.profile_name = name
        profiles_dir = self._get_profiles_dir()
        safe_name = re.sub(r'[^\w\-.]', '_', name)
        filepath = os.path.join(profiles_dir, f"{safe_name}.json")
        data = self.settings.to_dict()
        data["name"] = name
        try:
            with open(filepath, "w", encoding="utf-8") as f:
                json.dump(data, f, indent=2, ensure_ascii=False)
        except Exception as e:
            QMessageBox.critical(self, "保存エラー", f"プロファイルの保存に失敗しました:\n{e}")
            return
        self._profiles[name] = data
        self._profile_paths[name] = filepath
        # Add to combo if not already there
        if self._profile_combo.findText(name) < 0:
            self._profile_combo.addItem(name)
        self._profile_combo.blockSignals(True)
        self._profile_combo.setCurrentText(name)
        self._profile_combo.blockSignals(False)

    def _on_save_profile(self):
        name = self._profile_combo.currentText()
        if name == "Custom":
            self._on_new_profile()
            return
        if name not in self._profiles:
            return
        self._write_to_settings()
        self.settings.machine.profile_name = name
        filepath = self._profile_paths.get(name)
        if not filepath:
            return
        data = self.settings.to_dict()
        data["name"] = name
        try:
            with open(filepath, "w", encoding="utf-8") as f:
                json.dump(data, f, indent=2, ensure_ascii=False)
        except Exception as e:
            QMessageBox.critical(self, "保存エラー", f"プロファイルの保存に失敗しました:\n{e}")
            return
        self._profiles[name] = data

    def _on_del_profile(self):
        name = self._profile_combo.currentText()
        if name == "Custom":
            return
        ret = QMessageBox.question(
            self, "削除確認", f"プロファイル '{name}' を削除しますか?",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No)
        if ret != QMessageBox.StandardButton.Yes:
            return
        filepath = self._profile_paths.pop(name, None)
        if filepath and os.path.exists(filepath):
            try:
                os.remove(filepath)
            except OSError:
                pass
        self._profiles.pop(name, None)
        idx = self._profile_combo.findText(name)
        if idx >= 0:
            self._profile_combo.removeItem(idx)
        self._profile_combo.setCurrentText("Custom")

    # ------------------------------------------------------------------
    # Sync helpers
    # ------------------------------------------------------------------
    def _on_any_changed(self, *_):
        if self._updating:
            return
        self._write_to_settings()
        self._update_eff_area_label()
        self.settings_changed.emit()

    def _write_to_settings(self):
        """Copy widget values into self.settings."""
        s = self.settings
        m = s.machine
        m.firmware = self._firmware_combo.currentText()
        m.bed_x = self._bed_x.value()
        m.bed_y = self._bed_y.value()
        m.origin = "left_bottom" if self._origin_combo.currentIndex() == 0 else "center"

        p = s.pen
        p.pen_down_z = self._pen_down_z.value()
        p.pen_up_z = self._pen_up_z.value()
        p.pen_down_speed = self._pen_down_speed.value()
        p.pen_up_speed = self._pen_up_speed.value()
        p.touchdown_delay = self._touchdown_delay.value()
        p.liftup_delay = self._liftup_delay.value()
        p.offset_x = self._offset_x.value()
        p.offset_y = self._offset_y.value()
        p.offset_z = self._offset_z.value()

        sp = s.speed
        sp.draw_speed = self._draw_speed.value()
        sp.travel_speed = self._travel_speed.value()
        sp.acceleration = self._acceleration.value()
        sp.corner_speed = self._corner_speed.value()

        pa = s.path
        pa.scale = self._scale.value()
        pa.offset_x = self._path_offset_x.value()
        pa.offset_y = self._path_offset_y.value()
        pa.rotation = self._rotation.value()
        pa.optimize = self._optimize_cb.isChecked()
        pa.optimize_algorithm = self._algo_combo.currentText()
        pa.join_distance = self._join_dist.value()
        pa.curve_precision = self._curve_prec.value()

        fi = s.fill
        fi.enabled = self._fill_enabled.isChecked()
        fi.target = "fill_color" if self._fill_target.currentIndex() == 0 else "all_closed"
        lo_map = ["outline_first", "fill_first", "fill_only"]
        fi.layer_order = lo_map[self._fill_layer_order.currentIndex()]
        fi.pattern = self._fill_pattern.currentText()
        fi.spacing = self._fill_spacing.value()
        fi.angle = self._fill_angle.value()
        fi.margin = self._fill_margin.value()
        fi.cross_hatch_angle2 = self._crosshatch_angle2.value()
        contour_map = {"Keep": "keep", "Skip": "skip", "Dots": "dots"}
        fi.contour_innermost = contour_map.get(self._contour_innermost.currentText(), "keep")
        fi.dots_grid = "grid" if self._dots_grid.currentIndex() == 0 else "stagger"
        fi.dots_spacing_y = self._dots_spacing_y.value()

        gc = s.gcode
        gc.start_code = self._start_code.toPlainText()
        gc.end_code = self._end_code.toPlainText()
        gc.pen_up_code = self._pen_up_code.toPlainText()
        gc.pen_down_code = self._pen_down_code.toPlainText()

    def _refresh_from_settings(self):
        """Load widget values from self.settings (called on startup or profile load)."""
        self._updating = True
        s = self.settings
        m = s.machine
        fw_idx = self._firmware_combo.findText(m.firmware)
        if fw_idx >= 0:
            self._firmware_combo.setCurrentIndex(fw_idx)
        self._bed_x.setValue(m.bed_x)
        self._bed_y.setValue(m.bed_y)
        self._origin_combo.setCurrentIndex(0 if m.origin == "left_bottom" else 1)

        p = s.pen
        self._pen_down_z.setValue(p.pen_down_z)
        self._pen_up_z.setValue(p.pen_up_z)
        self._pen_down_speed.setValue(p.pen_down_speed)
        self._pen_up_speed.setValue(p.pen_up_speed)
        self._touchdown_delay.setValue(p.touchdown_delay)
        self._liftup_delay.setValue(p.liftup_delay)
        self._offset_x.setValue(p.offset_x)
        self._offset_y.setValue(p.offset_y)
        self._offset_z.setValue(p.offset_z)

        sp = s.speed
        self._draw_speed.setValue(sp.draw_speed)
        self._travel_speed.setValue(sp.travel_speed)
        self._acceleration.setValue(sp.acceleration)
        self._corner_speed.setValue(sp.corner_speed)

        pa = s.path
        self._scale.setValue(pa.scale)
        self._path_offset_x.setValue(pa.offset_x)
        self._path_offset_y.setValue(pa.offset_y)
        self._rotation.setValue(pa.rotation)
        self._optimize_cb.setChecked(pa.optimize)
        idx = self._algo_combo.findText(pa.optimize_algorithm)
        if idx >= 0:
            self._algo_combo.setCurrentIndex(idx)
        self._join_dist.setValue(pa.join_distance)
        self._curve_prec.setValue(pa.curve_precision)

        fi = s.fill
        self._fill_enabled.setChecked(fi.enabled)
        self._fill_target.setCurrentIndex(0 if fi.target == "fill_color" else 1)
        lo_map = {"outline_first": 0, "fill_first": 1, "fill_only": 2}
        self._fill_layer_order.setCurrentIndex(lo_map.get(fi.layer_order, 0))
        pi = self._fill_pattern.findText(fi.pattern)
        if pi >= 0:
            self._fill_pattern.setCurrentIndex(pi)
        self._fill_spacing.setValue(fi.spacing)
        self._fill_angle.setValue(fi.angle)
        self._fill_margin.setValue(fi.margin)
        self._crosshatch_angle2.setValue(fi.cross_hatch_angle2)
        ci_map = {"keep": "Keep", "skip": "Skip", "dots": "Dots"}
        ci = self._contour_innermost.findText(ci_map.get(fi.contour_innermost, "Keep"))
        if ci >= 0:
            self._contour_innermost.setCurrentIndex(ci)
        self._dots_grid.setCurrentIndex(0 if fi.dots_grid == "grid" else 1)
        self._dots_spacing_y.setValue(fi.dots_spacing_y)

        gc = s.gcode
        self._start_code.setPlainText(gc.start_code)
        self._end_code.setPlainText(gc.end_code)
        self._pen_up_code.setPlainText(gc.pen_up_code)
        self._pen_down_code.setPlainText(gc.pen_down_code)

        self._updating = False
        self._update_eff_area_label()

    def _update_eff_area_label(self):
        x_min, y_min, x_max, y_max = self.settings.effective_area()
        w = x_max - x_min
        h = y_max - y_min
        self._eff_area_label.setText(f"{w:.1f} × {h:.1f} mm")

    def set_path_offsets(self, ox: float, oy: float):
        """Programmatically set path placement offsets (e.g., from center button)."""
        self._updating = True
        self._path_offset_x.setValue(ox)
        self._path_offset_y.setValue(oy)
        self._updating = False
        self._on_any_changed()


def _mono_font():
    from PyQt6.QtGui import QFont
    f = QFont("Courier New", 9)
    f.setStyleHint(QFont.StyleHint.Monospace)
    return f
