"""Raw-mode tool toolbar — Select, Pen, Path, Line, Rect, Circle + width + color."""
from __future__ import annotations

from PyQt6.QtCore import pyqtSignal
from PyQt6.QtGui import QColor
from PyQt6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QPushButton, QLabel,
    QDoubleSpinBox, QButtonGroup, QColorDialog, QFrame, QSizePolicy,
)

# Tool IDs (match Preview2D constants)
TOOL_SELECT = "select"
TOOL_PEN    = "pen"
TOOL_PATH   = "path"
TOOL_LINE   = "line"
TOOL_RECT   = "rect"
TOOL_CIRCLE = "circle"

_TOOLS = [
    (TOOL_SELECT, "↖", "選択/移動/拡大"),
    (TOOL_PEN,    "✏",  "フリーハンド描画"),
    (TOOL_PATH,   "📐", "パス描画 (クリックで点追加、ダブルクリックで確定)"),
    (TOOL_LINE,   "╱",  "直線"),
    (TOOL_RECT,   "□",  "矩形"),
    (TOOL_CIRCLE, "○",  "楕円"),
]


def _sep_h() -> QFrame:
    f = QFrame()
    f.setFrameShape(QFrame.Shape.HLine)
    f.setStyleSheet("color: #555;")
    return f


class RawToolbar(QWidget):
    """Compact horizontal toolbar shown in Raw tab."""
    tool_changed  = pyqtSignal(str)           # tool id
    color_changed = pyqtSignal(str)           # hex color
    width_changed = pyqtSignal(float)         # mm

    def __init__(self, parent=None):
        super().__init__(parent)
        self._color = "#ff0000"

        hl = QHBoxLayout(self)
        hl.setContentsMargins(4, 2, 4, 2)
        hl.setSpacing(4)

        # Tool buttons (exclusive)
        self._btn_group = QButtonGroup(self)
        self._btn_group.setExclusive(True)
        for tid, icon, tip in _TOOLS:
            btn = QPushButton(icon)
            btn.setCheckable(True)
            btn.setFixedSize(28, 28)
            btn.setToolTip(tip)
            btn.setStyleSheet(
                "QPushButton{font-size:14px;border:1px solid #555;"
                "border-radius:3px;background:#333;}"
                "QPushButton:checked{background:#4466aa;border-color:#88aaff;}"
                "QPushButton:hover{background:#444;}")
            btn.setProperty("tool_id", tid)
            btn.clicked.connect(self._on_tool_btn)
            self._btn_group.addButton(btn)
            hl.addWidget(btn)
            if tid == TOOL_SELECT:
                btn.setChecked(True)

        hl.addWidget(_sep_h())

        # Line width
        hl.addWidget(QLabel("太さ:"))
        self._width_spin = QDoubleSpinBox()
        self._width_spin.setRange(0.1, 10.0)
        self._width_spin.setSingleStep(0.1)
        self._width_spin.setDecimals(1)
        self._width_spin.setValue(0.5)
        self._width_spin.setSuffix(" mm")
        self._width_spin.setFixedWidth(80)
        self._width_spin.setToolTip("ストローク幅 (mm)")
        self._width_spin.valueChanged.connect(
            lambda v: self.width_changed.emit(v))
        hl.addWidget(self._width_spin)

        # Color picker
        hl.addWidget(QLabel("色:"))
        self._color_btn = QPushButton()
        self._color_btn.setFixedSize(28, 28)
        self._color_btn.setToolTip("ストローク色を選択")
        self._color_btn.clicked.connect(self._pick_color)
        self._update_color_btn()
        hl.addWidget(self._color_btn)

        hl.addStretch()

        self.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        self.setFixedHeight(36)

    # ── public ──────────────────────────────────────────────────────────────

    def current_tool(self) -> str:
        for btn in self._btn_group.buttons():
            if btn.isChecked():
                return btn.property("tool_id")
        return TOOL_SELECT

    def current_color(self) -> str:
        return self._color

    def current_width(self) -> float:
        return self._width_spin.value()

    # ── slots ────────────────────────────────────────────────────────────────

    def _on_tool_btn(self):
        btn = self.sender()
        if btn:
            self.tool_changed.emit(btn.property("tool_id"))

    def _pick_color(self):
        c = QColorDialog.getColor(QColor(self._color), self, "ストローク色")
        if c.isValid():
            self._color = c.name()
            self._update_color_btn()
            self.color_changed.emit(self._color)

    def _update_color_btn(self):
        self._color_btn.setStyleSheet(
            f"background:{self._color}; border:1px solid #888; border-radius:3px;")
