"""Right panel — path list grouped by colour, with drag-and-drop reordering."""
from __future__ import annotations
from typing import List, Optional

from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtGui import QColor, QIcon, QPixmap, QPainter
from PyQt6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QPushButton,
    QListWidget, QListWidgetItem, QCheckBox, QGroupBox, QSizePolicy,
)
from src.models.pen_path import PathGroup


class ColorSwatch(QLabel):
    """A small square that shows a colour."""

    def __init__(self, color: str, parent=None):
        super().__init__(parent)
        self.setFixedSize(16, 16)
        self._set_color(color)

    def _set_color(self, color: str):
        pix = QPixmap(16, 16)
        pix.fill(QColor(color) if color != "none" else QColor("transparent"))
        self.setPixmap(pix)


class GroupItem(QWidget):
    """Widget displayed for each colour group in the list."""

    pen_change_toggled = pyqtSignal(bool)

    def __init__(self, group: PathGroup, parent=None):
        super().__init__(parent)
        self.group = group
        hl = QHBoxLayout(self)
        hl.setContentsMargins(4, 2, 4, 2)

        swatch = ColorSwatch(group.color)
        hl.addWidget(swatch)

        label_text = group.label or group.color
        count = len(group.paths)
        lbl = QLabel(f"{label_text}  ({count} paths)")
        lbl.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
        hl.addWidget(lbl)

        self._pen_cb = QCheckBox("Pen change")
        self._pen_cb.setChecked(group.pen_change_before)
        self._pen_cb.stateChanged.connect(
            lambda s: self.pen_change_toggled.emit(bool(s))
        )
        hl.addWidget(self._pen_cb)


class PathListPanel(QWidget):
    """Right side panel showing colour groups and allowing reordering."""

    order_changed = pyqtSignal()    # emitted when groups are reordered
    pen_change_changed = pyqtSignal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.groups: List[PathGroup] = []

        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 4, 4, 4)

        title = QLabel("Path Groups")
        title.setStyleSheet("font-weight: bold;")
        layout.addWidget(title)

        self._list = QListWidget()
        self._list.setDragDropMode(QListWidget.DragDropMode.InternalMove)
        self._list.setSelectionMode(QListWidget.SelectionMode.SingleSelection)
        self._list.model().rowsMoved.connect(self._on_rows_moved)
        layout.addWidget(self._list)

        btn_row = QHBoxLayout()
        self._up_btn = QPushButton("▲")
        self._dn_btn = QPushButton("▼")
        self._up_btn.setFixedWidth(32)
        self._dn_btn.setFixedWidth(32)
        self._up_btn.clicked.connect(self._move_up)
        self._dn_btn.clicked.connect(self._move_down)
        btn_row.addWidget(self._up_btn)
        btn_row.addWidget(self._dn_btn)
        btn_row.addStretch()
        layout.addLayout(btn_row)

        self.setMinimumWidth(200)
        self.setMaximumWidth(280)

    # ------------------------------------------------------------------ public
    def set_groups(self, groups: List[PathGroup]):
        self.groups = list(groups)
        self._rebuild_list()

    def current_groups(self) -> List[PathGroup]:
        return self.groups

    # ------------------------------------------------------------------ internal
    def _rebuild_list(self):
        self._list.clear()
        for group in self.groups:
            item = QListWidgetItem(self._list)
            widget = GroupItem(group)
            widget.pen_change_toggled.connect(
                lambda checked, g=group: self._on_pen_change(g, checked)
            )
            item.setSizeHint(widget.sizeHint())
            self._list.addItem(item)
            self._list.setItemWidget(item, widget)

    def _on_rows_moved(self, *_):
        """Sync self.groups order after drag-drop."""
        new_groups = []
        for i in range(self._list.count()):
            item = self._list.item(i)
            w = self._list.itemWidget(item)
            if isinstance(w, GroupItem):
                new_groups.append(w.group)
        self.groups = new_groups
        self.order_changed.emit()

    def _on_pen_change(self, group: PathGroup, checked: bool):
        group.pen_change_before = checked
        self.pen_change_changed.emit()

    def _move_up(self):
        row = self._list.currentRow()
        if row > 0:
            self.groups.insert(row - 1, self.groups.pop(row))
            self._rebuild_list()
            self._list.setCurrentRow(row - 1)
            self.order_changed.emit()

    def _move_down(self):
        row = self._list.currentRow()
        if 0 <= row < len(self.groups) - 1:
            self.groups.insert(row + 1, self.groups.pop(row))
            self._rebuild_list()
            self._list.setCurrentRow(row + 1)
            self.order_changed.emit()
