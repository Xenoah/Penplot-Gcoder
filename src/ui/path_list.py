"""Right panel — layers and path groups with per-layer visibility toggles."""
from __future__ import annotations
from typing import List

from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtGui import QColor, QPixmap, QBrush
from PyQt6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QPushButton,
    QListWidget, QListWidgetItem, QCheckBox, QSizePolicy,
)
from src.models.pen_path import PathGroup


# QListWidgetItem data roles
_LAYER_IDX_ROLE = Qt.ItemDataRole.UserRole
_IS_HEADER_ROLE = Qt.ItemDataRole.UserRole + 1


# ---------------------------------------------------------------------------
# Sub-widgets
# ---------------------------------------------------------------------------
class _ColorSwatch(QLabel):
    def __init__(self, color: str, parent=None):
        super().__init__(parent)
        self.setFixedSize(14, 14)
        pix = QPixmap(14, 14)
        c = QColor(color) if color and color != "none" else QColor("transparent")
        pix.fill(c)
        self.setPixmap(pix)


class GroupItem(QWidget):
    """Row widget for a single PathGroup."""
    pen_change_toggled = pyqtSignal(bool)

    def __init__(self, group: PathGroup, indented: bool = False, parent=None):
        super().__init__(parent)
        self.group = group
        hl = QHBoxLayout(self)
        hl.setContentsMargins(4 + (14 if indented else 0), 2, 4, 2)
        hl.setSpacing(6)

        hl.addWidget(_ColorSwatch(group.color or "#00cc66"))

        label_text = group.label or group.color or "—"
        lbl = QLabel(f"{label_text}  ({len(group.paths)} paths)")
        lbl.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
        hl.addWidget(lbl)

        cb = QCheckBox("Pen ↕")
        cb.setChecked(group.pen_change_before)
        cb.stateChanged.connect(lambda s: self.pen_change_toggled.emit(bool(s)))
        hl.addWidget(cb)


class _LayerHeaderWidget(QWidget):
    """Row widget for a layer section header."""
    visibility_changed = pyqtSignal(bool)

    def __init__(self, name: str, path_count: int, visible: bool = True, parent=None):
        super().__init__(parent)
        hl = QHBoxLayout(self)
        hl.setContentsMargins(4, 2, 4, 2)
        hl.setSpacing(6)

        self._vis = QCheckBox()
        self._vis.setChecked(visible)
        self._vis.setToolTip("このレイヤーを表示/非表示")
        self._vis.stateChanged.connect(lambda s: self.visibility_changed.emit(bool(s)))
        hl.addWidget(self._vis)

        lbl = QLabel(f"\U0001f4c4 {name}")
        lbl.setStyleSheet("font-weight: bold; color: #ddd;")
        lbl.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
        hl.addWidget(lbl)

        cnt = QLabel(f"({path_count})")
        cnt.setStyleSheet("color: #888; font-size: 10px;")
        hl.addWidget(cnt)


# ---------------------------------------------------------------------------
# PathListPanel
# ---------------------------------------------------------------------------
class PathListPanel(QWidget):
    """Right panel: layers → groups, with per-layer visibility and group reorder."""

    order_changed          = pyqtSignal()   # group order changed
    pen_change_changed     = pyqtSignal()   # pen_change_before toggled
    layer_visibility_changed = pyqtSignal() # layer visibility toggled

    def __init__(self, parent=None):
        super().__init__(parent)
        # Internal state
        self._layers: List[dict] = []  # [{name, groups, visible}]
        self.groups: List[PathGroup] = []  # flat ordered visible groups (compat)

        vl = QVBoxLayout(self)
        vl.setContentsMargins(4, 4, 4, 4)
        vl.setSpacing(4)

        hdr = QLabel("レイヤー / グループ")
        hdr.setStyleSheet("font-weight: bold;")
        vl.addWidget(hdr)

        self._list = QListWidget()
        # Drag-drop disabled; use ▲/▼ buttons to reorder groups
        self._list.setDragDropMode(QListWidget.DragDropMode.NoDragDrop)
        self._list.setSelectionMode(QListWidget.SelectionMode.SingleSelection)
        vl.addWidget(self._list, 1)

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
        vl.addLayout(btn_row)

        self.setMinimumWidth(200)
        self.setMaximumWidth(280)

    # ------------------------------------------------------------------ public

    def set_groups(self, groups: List[PathGroup]):
        """Backward-compat: treat as a single unnamed layer."""
        self.set_layers([{'name': 'Layer 1', 'groups': groups, 'visible': True}])

    def set_layers(self, layers: List[dict]):
        """
        layers: list of dicts with keys:
            name    (str)
            groups  (List[PathGroup])
            visible (bool, default True)
        """
        self._layers = [dict(l) for l in layers]
        self._rebuild_list()
        self.groups = self.current_groups()

    def current_groups(self) -> List[PathGroup]:
        """Return all groups in display order, from visible layers only."""
        result = []
        for i in range(self._list.count()):
            item = self._list.item(i)
            if item.data(_IS_HEADER_ROLE):
                continue
            li = item.data(_LAYER_IDX_ROLE)
            if li is None or li >= len(self._layers) or self._layers[li].get('visible', True):
                w = self._list.itemWidget(item)
                if isinstance(w, GroupItem):
                    result.append(w.group)
        return result

    # ------------------------------------------------------------------ internal

    def _rebuild_list(self):
        self._list.clear()
        multi = len(self._layers) > 1
        for li, layer in enumerate(self._layers):
            visible = layer.get('visible', True)
            if multi:
                # Layer header
                header_item = QListWidgetItem()
                header_item.setData(_IS_HEADER_ROLE, True)
                header_item.setData(_LAYER_IDX_ROLE, li)
                header_item.setFlags(Qt.ItemFlag.ItemIsEnabled)  # not selectable/draggable
                hw = _LayerHeaderWidget(
                    layer['name'],
                    sum(len(g.paths) for g in layer['groups']),
                    visible)
                hw.visibility_changed.connect(
                    lambda vis, idx=li: self._on_layer_vis(idx, vis))
                header_item.setSizeHint(hw.sizeHint())
                header_item.setBackground(QBrush(QColor("#2a2a3a")))
                self._list.addItem(header_item)
                self._list.setItemWidget(header_item, hw)

            for group in layer['groups']:
                item = QListWidgetItem()
                item.setData(_IS_HEADER_ROLE, False)
                item.setData(_LAYER_IDX_ROLE, li)
                gw = GroupItem(group, indented=multi)
                if not visible:
                    gw.setEnabled(False)
                    gw.setStyleSheet("color: #555;")
                gw.pen_change_toggled.connect(
                    lambda checked, g=group: self._on_pen_change(g, checked))
                item.setSizeHint(gw.sizeHint())
                self._list.addItem(item)
                self._list.setItemWidget(item, gw)

    def _on_layer_vis(self, idx: int, visible: bool):
        self._layers[idx]['visible'] = visible
        self._rebuild_list()
        self.groups = self.current_groups()
        self.layer_visibility_changed.emit()

    def _on_pen_change(self, group: PathGroup, checked: bool):
        group.pen_change_before = checked
        self.pen_change_changed.emit()

    # ---- reorder buttons (groups only, within their layer) ----------------

    def _move_up(self):
        row = self._list.currentRow()
        if row <= 0:
            return
        item = self._list.item(row)
        if not item or item.data(_IS_HEADER_ROLE):
            return
        # Find previous non-header row
        target = row - 1
        while target >= 0 and self._list.item(target).data(_IS_HEADER_ROLE):
            target -= 1
        if target < 0:
            return
        self._swap_rows(row, target)
        self._list.setCurrentRow(target)

    def _move_down(self):
        row = self._list.currentRow()
        if row < 0 or row >= self._list.count() - 1:
            return
        item = self._list.item(row)
        if not item or item.data(_IS_HEADER_ROLE):
            return
        # Find next non-header row
        target = row + 1
        while target < self._list.count() and self._list.item(target).data(_IS_HEADER_ROLE):
            target += 1
        if target >= self._list.count():
            return
        self._swap_rows(row, target)
        self._list.setCurrentRow(target)

    def _swap_rows(self, row_a: int, row_b: int):
        """Swap two group items in _layers and rebuild the list."""
        def _find_in_layers(row: int):
            item = self._list.item(row)
            li = item.data(_LAYER_IDX_ROLE)
            w  = self._list.itemWidget(item)
            if isinstance(w, GroupItem):
                try:
                    gi = self._layers[li]['groups'].index(w.group)
                    return li, gi
                except (ValueError, IndexError):
                    pass
            return None, None

        la, ga = _find_in_layers(row_a)
        lb, gb = _find_in_layers(row_b)
        if la is None or lb is None:
            return

        gl_a = self._layers[la]['groups']
        gl_b = self._layers[lb]['groups']
        gl_a[ga], gl_b[gb] = gl_b[gb], gl_a[ga]
        self._rebuild_list()
        self.groups = self.current_groups()
        self.order_changed.emit()
