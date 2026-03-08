"""Application settings data model — serialisable to/from JSON."""
from __future__ import annotations
import json
from dataclasses import dataclass, field, asdict
from typing import Optional


# ---------------------------------------------------------------------------
# Sub-models
# ---------------------------------------------------------------------------

@dataclass
class MachineSettings:
    profile_name: str = "Custom"
    firmware: str = "Marlin"        # "Marlin" | "Klipper"
    bed_x: float = 220.0
    bed_y: float = 220.0
    origin: str = "left_bottom"     # "left_bottom" | "center"


@dataclass
class PenSettings:
    pen_down_z: float = 0.0
    pen_up_z: float = 5.0
    pen_down_speed: float = 5.0     # mm/s
    pen_up_speed: float = 17.0      # mm/s
    touchdown_delay: int = 0        # ms
    liftup_delay: int = 0           # ms
    offset_x: float = 0.0
    offset_y: float = 0.0
    offset_z: float = 0.0


@dataclass
class SpeedSettings:
    draw_speed: float = 25.0        # mm/s
    travel_speed: float = 100.0     # mm/s
    acceleration: float = 1000.0    # mm/s²
    corner_speed: float = 5.0       # mm/s


@dataclass
class PathSettings:
    scale: float = 100.0            # %
    offset_x: float = 0.0          # mm, placement on bed
    offset_y: float = 0.0
    rotation: float = 0.0          # degrees
    optimize: bool = True
    optimize_algorithm: str = "Greedy"  # "Greedy" | "2-opt"
    join_distance: float = 0.5     # mm
    curve_precision: float = 0.1   # mm


@dataclass
class FillSettings:
    enabled: bool = False
    target: str = "fill_color"      # "fill_color" | "all_closed"
    layer_order: str = "outline_first"  # "outline_first" | "fill_first" | "fill_only"
    pattern: str = "Lines"          # Lines/CrossHatch/TriHatch/ContourOffset/Zigzag/Dots/Spiral
    spacing: float = 1.0            # mm
    angle: float = 45.0             # degrees
    margin: float = 0.0             # mm inset from outline
    cross_hatch_angle2: float = 90.0   # offset added to angle for 2nd pass
    contour_innermost: str = "keep"    # "keep" | "skip" | "dots"
    dots_grid: str = "grid"            # "grid" | "stagger"
    dots_spacing_y: float = 1.0


@dataclass
class GCodeSettings:
    start_code: str = (
        "G28\n"
        "G90\n"
        "G21\n"
        "G1 Z{pen_up_z} F{pen_up_speed}"
    )
    end_code: str = (
        "G1 Z{pen_up_z} F{pen_up_speed}\n"
        "G1 X0 Y0 F{travel_speed}\n"
        "M84"
    )
    pen_up_code: str = "G1 Z{pen_up_z} F{pen_up_speed}"
    pen_down_code: str = "G1 Z{pen_down_z} F{pen_down_speed}"


# ---------------------------------------------------------------------------
# Root settings
# ---------------------------------------------------------------------------

@dataclass
class AppSettings:
    machine: MachineSettings = field(default_factory=MachineSettings)
    pen: PenSettings = field(default_factory=PenSettings)
    speed: SpeedSettings = field(default_factory=SpeedSettings)
    path: PathSettings = field(default_factory=PathSettings)
    fill: FillSettings = field(default_factory=FillSettings)
    gcode: GCodeSettings = field(default_factory=GCodeSettings)

    # ---- serialisation ---------------------------------------------------
    def to_dict(self) -> dict:
        return asdict(self)

    def to_json(self, path: str) -> None:
        with open(path, "w", encoding="utf-8") as f:
            json.dump(self.to_dict(), f, indent=2, ensure_ascii=False)

    @classmethod
    def from_dict(cls, d: dict) -> "AppSettings":
        s = cls()
        if "machine" in d:
            s.machine = MachineSettings(**d["machine"])
        if "pen" in d:
            s.pen = PenSettings(**d["pen"])
        if "speed" in d:
            s.speed = SpeedSettings(**d["speed"])
        if "path" in d:
            s.path = PathSettings(**d["path"])
        if "fill" in d:
            s.fill = FillSettings(**d["fill"])
        if "gcode" in d:
            s.gcode = GCodeSettings(**d["gcode"])
        return s

    @classmethod
    def from_json(cls, path: str) -> "AppSettings":
        with open(path, "r", encoding="utf-8") as f:
            return cls.from_dict(json.load(f))

    # ---- helper: effective drawing area ----------------------------------
    def effective_area(self):
        """Return (x_min, y_min, x_max, y_max) of the drawable area."""
        ox = self.pen.offset_x
        oy = self.pen.offset_y
        bx = self.machine.bed_x
        by = self.machine.bed_y
        x_min = max(0.0, -ox)
        x_max = min(bx, bx - ox)
        y_min = max(0.0, -oy)
        y_max = min(by, by - oy)
        return x_min, y_min, x_max, y_max

    def effective_area_size(self):
        x_min, y_min, x_max, y_max = self.effective_area()
        return x_max - x_min, y_max - y_min
