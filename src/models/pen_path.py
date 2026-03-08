"""PenPath data model — represents a single contiguous pen-down stroke."""
from __future__ import annotations
from dataclasses import dataclass, field
from typing import List, Optional, Tuple
import numpy as np


@dataclass
class PenPath:
    """A single contiguous pen-down stroke as an ordered list of (x, y) points."""
    points: List[Tuple[float, float]] = field(default_factory=list)
    color: str = "#000000"          # stroke colour (hex)
    fill_color: Optional[str] = None  # fill colour (hex or None)
    is_closed: bool = False
    layer: str = "default"
    source_id: str = ""             # original SVG id / DXF handle
    stroke_width_mm: float = 0.5   # visual stroke width in mm (for preview/user-drawn paths)

    # ---- derived / cached ------------------------------------------------
    @property
    def np_points(self) -> np.ndarray:
        """Return points as Nx2 numpy array."""
        if not self.points:
            return np.empty((0, 2))
        return np.array(self.points, dtype=float)

    @property
    def start(self) -> Optional[Tuple[float, float]]:
        return self.points[0] if self.points else None

    @property
    def end(self) -> Optional[Tuple[float, float]]:
        return self.points[-1] if self.points else None

    def length(self) -> float:
        """Approximate arc length."""
        pts = self.np_points
        if len(pts) < 2:
            return 0.0
        diffs = np.diff(pts, axis=0)
        return float(np.sum(np.hypot(diffs[:, 0], diffs[:, 1])))

    def reversed(self) -> "PenPath":
        """Return a copy with reversed point order."""
        return PenPath(
            points=list(reversed(self.points)),
            color=self.color,
            fill_color=self.fill_color,
            is_closed=self.is_closed,
            layer=self.layer,
            source_id=self.source_id,
            stroke_width_mm=self.stroke_width_mm,
        )

    def transformed(self, scale: float = 1.0,
                    offset_x: float = 0.0, offset_y: float = 0.0,
                    angle_deg: float = 0.0) -> "PenPath":
        """Return a copy with scale / rotation / translation applied."""
        pts = self.np_points
        if len(pts) == 0:
            return PenPath(color=self.color, fill_color=self.fill_color,
                           is_closed=self.is_closed, layer=self.layer,
                           source_id=self.source_id)
        pts = pts * scale
        if angle_deg != 0.0:
            rad = np.deg2rad(angle_deg)
            c, s = np.cos(rad), np.sin(rad)
            rot = np.array([[c, -s], [s, c]])
            pts = pts @ rot.T
        pts[:, 0] += offset_x
        pts[:, 1] += offset_y
        return PenPath(
            points=[tuple(p) for p in pts.tolist()],
            color=self.color,
            fill_color=self.fill_color,
            is_closed=self.is_closed,
            layer=self.layer,
            source_id=self.source_id,
            stroke_width_mm=self.stroke_width_mm,
        )


@dataclass
class PathGroup:
    """A group of PenPaths sharing the same colour (= one pen)."""
    color: str = "#000000"
    label: str = ""
    paths: List[PenPath] = field(default_factory=list)
    pen_change_before: bool = False   # insert M0 pause before this group
