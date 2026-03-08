"""Right panel — layers and path groups with per-layer visibility, add/delete, rename."""
from __future__ import annotations
from typing import List, Optional

from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtGui import QColor, QPixmap, QBrush
from PyQt6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QPushButton,
    QListWidget, QListWidgetItem, QCheckBox, QSizePolicy, QInputDialog,
    QMessageBox,
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

        cb = QCheckBox("Pen \u21d5")
        cb.setChecked(group.pen_change_before)
        cb.stateChanged.connect(lambda s: self.pen_change_toggled.emit(bool(s)))
        hl.addWidget(cb)


class _LayerHeaderWidget(QWidget):
    """Row widget for a layer section header."""
    visibility_changed = pyqtSignal(bool)
    rename_requested   = pyqtSignal()

    def __init__(self, name: str, path_count: int,
                 visible: bool = True, active: bool = False, parent=None):
        super().__init__(parent)
        hl = QHBoxLayout(self)
        hl.setContentsMargins(4, 2, 4, 2)
        hl.setSpacing(6)

        self._vis = QCheckBox()
        self._vis.setChecked(visible)
        self._vis.setToolTip("\u3053\u306e\u30ec\u30a4\u30e4\u30fc\u3092\u8868\u793a/\u975e\u8868\u793a")
        self._vis.stateChanged.connect(lambda s: self.visibility_changed.emit(bool(s)))
        hl.addWidget(self._vis)

        icon = "\U0001f4c4"
        self._lbl = QLabel(f"{icon} {name}")
        style = "font-weight: bold; color: #ddd;"
        if active:
            style = "font-weight: bold; color: #88ccff;"
        self._lbl.setStyleSheet(style)
        self._lbl.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Preferred)
        hl.addWidget(self._lbl)

        cnt = QLabel(f"({path_count})")
        cnt.setStyleSheet("color: #888; font-size: 10px;")
        hl.addWidget(cnt)


# ---------------------------------------------------------------------------
# PathListPanel
# ---------------------------------------------------------------------------
class PathListPanel(QWidget):
    """Right panel: layers → groups, per-layer visibility, add/delete/rename layers."""

    order_changed            = pyqtSignal()   # group order changed
    pen_change_changed       = pyqtSignal()   # pen_change_before toggled
    layer_visibility_changed = pyqtSignal()   # visibility toggled
    add_layer_requested      = pyqtSignal()   # "+" button
    delete_layer_requested   = pyqtSignal(int)  # "-" button, layer index

    def __init__(self, parent=None):
        super().__init__(parent)
        self._layers: List[dict] = []           # [{name, groups, visible, is_draw}]
        self._active_layer_idx: int = -1        # which layer receives drawn strokes
        self.groups: List[PathGroup] = []       # flat ordered visible groups (compat)

        vl = QVBoxLayout(self)
        vl.setContentsMargins(4, 4, 4, 4)
        vl.setSpacing(4)

        hdr = QLabel("\u30ec\u30a4\u30e4\u30fc / \u30b0\u30eb\u30fc\u30d7")
        hdr.setStyleSheet("font-weight: bold;")
        vl.addWidget(hdr)

        self._list = QListWidget()
        self._list.setDragDropMode(QListWidget.DragDropMode.NoDragDrop)
        self._list.setSelectionMode(QListWidget.SelectionMode.SingleSelection)
        self._list.itemClicked.connect(self._on_item_clicked)
        self._list.itemDoubleClicked.connect(self._on_item_double_clicked)
        vl.addWidget(self._list, 1)

        # Row: move ▲/▼ + add/delete layer
        btn_row = QHBoxLayout()
        self._up_btn  = QPushButton("\u25b2")
        self._dn_btn  = QPushButton("\u25bc")
        self._add_btn = QPushButton("+")
        self._del_btn = QPushButton("\u2212")
        for b in (self._up_btn, self._dn_btn, self._add_btn, self._del_btn):
            b.setFixedSize(28, 24)
        self._up_btn.setToolTip("\u30b0\u30eb\u30fc\u30d7\u3092\u4e0a\u3078")
        self._dn_btn.setToolTip("\u30b0\u30eb\u30fc\u30d7\u3092\u4e0b\u3078")
        self._add_btn.setToolTip("\u63cf\u753b\u30ec\u30a4\u30e4\u30fc\u3092\u8ffd\u52a0")
        self._del_btn.setToolTip("\u9078\u629e\u4e2d\u306e\u30ec\u30a4\u30e4\u30fc\u3092\u524a\u9664")
        self._add_btn.setStyleSheet(
            "QPushButton{background:#2a6a2a;color:white;font-weight:bold;"
            "border-radius:3px;font-size:14px;}"
            "QPushButton:hover{background:#3a8a3a;}")
        self._del_btn.setStyleSheet(
            "QPushButton{background:#6a2a2a;color:white;font-weight:bold;"
            "border-radius:3px;font-size:14px;}"
            "QPushButton:hover{background:#8a3a3a;}")
        self._up_btn.clicked.connect(self._move_up)
        self._dn_btn.clicked.connect(self._move_down)
        self._add_btn.clicked.connect(self.add_layer_requested)
        self._del_btn.clicked.connect(self._on_del_clicked)
        for b in (self._up_btn, self._dn_btn, self._add_btn, self._del_btn):
            btn_row.addWidget(b)
        btn_row.addStretch()
        vl.addLayout(btn_row)

        self.setMinimumWidth(200)
        self.setMaximumWidth(280)

    # ------------------------------------------------------------------ public

    def set_groups(self, groups: List[PathGroup]):
        """Backward-compat: treat as a single unnamed layer."""
        self.set_layers([{'name': 'Layer 1', 'groups': groups,
                          'visible': True, 'is_draw': False}])

    def set_layers(self, layers: List[dict]):
        """
        layers: list of dicts with keys:
            name      (str)
            groups    (List[PathGroup])
            visible   (bool, default True)
            is_draw   (bool, default False) — user-created draw layer
        """
        self._layers = [dict(l) for l in layers]
        if self._active_layer_idx >= len(self._layers):
            self._active_layer_idx = len(self._layers) - 1
        self._rebuild_list()
        self.groups = self.current_groups()

    def active_layer_index(self) -> int:
        return self._active_layer_idx

    def set_active_layer(self, idx: int):
        self._active_layer_idx = idx
        self._rebuild_list()

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
            active  = (li == self._active_layer_idx)
            if multi or layer.get('is_draw', False):
                # Layer header
                header_item = QListWidgetItem()
                header_item.setData(_IS_HEADER_ROLE, True)
                header_item.setData(_LAYER_IDX_ROLE, li)
                flags = Qt.ItemFlag.ItemIsEnabled | Qt.ItemFlag.ItemIsSelectable
                header_item.setFlags(flags)
                hw = _LayerHeaderWidget(
                    layer['name'],
                    sum(len(g.paths) for g in layer['groups']),
                    visible, active)
                hw.visibility_changed.connect(
                    lambda vis, idx=li: self._on_layer_vis(idx, vis))
                header_item.setSizeHint(hw.sizeHint())
                bg_color = "#2a3a5a" if active else "#2a2a3a"
                header_item.setBackground(QBrush(QColor(bg_color)))
                self._list.addItem(header_item)
                self._list.setItemWidget(header_item, hw)

            for group in layer['groups']:
                item = QListWidgetItem()
                item.setData(_IS_HEADER_ROLE, False)
                item.setData(_LAYER_IDX_ROLE, li)
                item.setFlags(Qt.ItemFlag.ItemIsEnabled | Qt.ItemFlag.ItemIsSelectable)
                gw = GroupItem(group, indented=multi or layer.get('is_draw', False))
                if not visible:
                    gw.setEnabled(False)
                    gw.setStyleSheet("color: #555;")
                gw.pen_change_toggled.connect(
                    lambda checked, g=group: self._on_pen_change(g, checked))
                item.setSizeHint(gw.sizeHint())
                self._list.addItem(item)
                self._list.setItemWidget(item, gw)

    def _on_item_clicked(self, item: QListWidgetItem):
        """Clicking a layer header makes it the active layer."""
        if item.data(_IS_HEADER_ROLE):
            li = item.data(_LAYER_IDX_ROLE)
            if li is not None:
                self._active_layer_idx = li
                self._rebuild_list()

    def _on_item_double_clicked(self, item: QListWidgetItem):
        """Double-clicking a layer header renames it."""
        if not item.data(_IS_HEADER_ROLE):
            return
        li = item.data(_LAYER_IDX_ROLE)
        if li is None or li >= len(self._layers):
            return
        old_name = self._layers[li]['name']
        new_name, ok = QInputDialog.getText(
            self, "\u30ec\u30a4\u30e4\u30fc\u540d\u5909\u66f4", "\u65b0\u3057\u3044\u540d\u524d:",
            text=old_name)
        if ok and new_name.strip():
            self._layers[li]['name'] = new_name.strip()
            self._rebuild_list()

    def _on_layer_vis(self, idx: int, visible: bool):
        self._layers[idx]['visible'] = visible
        self._rebuild_list()
        self.groups = self.current_groups()
        self.layer_visibility_changed.emit()

    def _on_pen_change(self, group: PathGroup, checked: bool):
        group.pen_change_before = checked
        self.pen_change_changed.emit()

    def _on_del_clicked(self):
        idx = self._active_layer_idx
        if idx < 0 or idx >= len(self._layers):
            return
        layer = self._layers[idx]
        if not layer.get('is_draw', False):
            QMessageBox.information(
                self, "\u524a\u9664\u4e0d\u53ef",
                "\u30d5\u30a1\u30a4\u30eb\u304b\u3089\u8aad\u307f\u8fbc\u3093\u3060"
                "\u30ec\u30a4\u30e4\u30fc\u306f\u524a\u9664\u3067\u304d\u307e\u305b\u3093\u3002\n"
                "\u63cf\u753b\u30ec\u30a4\u30e4\u30fc\u306e\u307f\u524a\u9664\u53ef\u80fd\u3067\u3059\u3002")
            return
        self.delete_layer_requested.emit(idx)

    # ---- reorder buttons ---------------------------------------------------

    def _move_up(self):
        row = self._list.currentRow()
        if row <= 0:
            return
        item = self._list.item(row)
        if not item or item.data(_IS_HEADER_ROLE):
            return
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
        target = row + 1
        while target < self._list.count() and self._list.item(target).data(_IS_HEADER_ROLE):
            target += 1
        if target >= self._list.count():
            return
        self._swap_rows(row, target)
        self._list.setCurrentRow(target)

    def _swap_rows(self, row_a: int, row_b: int):
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
