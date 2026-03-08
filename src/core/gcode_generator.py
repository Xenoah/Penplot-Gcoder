"""G-code generation engine."""
from __future__ import annotations
import math
from datetime import datetime
from typing import List, Optional, Tuple
from src.models.pen_path import PenPath, PathGroup
from src.models.settings import AppSettings


def _resolve(template: str, settings: AppSettings) -> str:
    """Replace placeholder variables in a G-code template string."""
    p = settings.pen
    sp = settings.speed
    ox, oy, oz = p.offset_x, p.offset_y, p.offset_z
    bx, by = settings.machine.bed_x, settings.machine.bed_y
    mapping = {
        "{pen_down_z}":     str(p.pen_down_z),
        "{pen_up_z}":       str(p.pen_up_z),
        "{pen_down_speed}": str(int(p.pen_down_speed * 60)),
        "{pen_up_speed}":   str(int(p.pen_up_speed * 60)),
        "{draw_speed}":     str(int(sp.draw_speed * 60)),
        "{travel_speed}":   str(int(sp.travel_speed * 60)),
        "{offset_x}":       str(ox),
        "{offset_y}":       str(oy),
        "{offset_z}":       str(oz),
        "{bed_x}":          str(bx),
        "{bed_y}":          str(by),
        "{touchdown_delay}": str(p.touchdown_delay),
        "{liftup_delay}":   str(p.liftup_delay),
    }
    result = template
    for key, val in mapping.items():
        result = result.replace(key, val)
    return result


def _estimate_time(groups: List[PathGroup], settings: AppSettings) -> Tuple[float, float]:
    """Return (total_draw_mm, estimated_seconds)."""
    draw_speed = settings.speed.draw_speed          # mm/s
    travel_speed = settings.speed.travel_speed
    pen_down_speed = settings.pen.pen_down_speed
    pen_up_speed = settings.pen.pen_up_speed
    z_travel = abs(settings.pen.pen_up_z - settings.pen.pen_down_z)

    total_draw = 0.0
    total_time = 0.0
    current_pos = (0.0, 0.0)

    for group in groups:
        for path in group.paths:
            if not path.points:
                continue
            start = path.points[0]
            # travel to start
            d_travel = math.hypot(start[0] - current_pos[0], start[1] - current_pos[1])
            total_time += d_travel / max(travel_speed, 0.001)
            # pen down
            total_time += z_travel / max(pen_down_speed, 0.001)
            total_time += settings.pen.touchdown_delay / 1000.0
            # draw
            pts = path.points
            for i in range(1, len(pts)):
                d = math.hypot(pts[i][0] - pts[i-1][0], pts[i][1] - pts[i-1][1])
                total_draw += d
                total_time += d / max(draw_speed, 0.001)
            total_time += settings.pen.liftup_delay / 1000.0
            # pen up
            total_time += z_travel / max(pen_up_speed, 0.001)
            current_pos = pts[-1]

    return total_draw, total_time


def _fmt_time(seconds: float) -> str:
    seconds = int(seconds)
    m, s = divmod(seconds, 60)
    h, m = divmod(m, 60)
    if h > 0:
        return f"{h}h{m:02d}m{s:02d}s"
    return f"{m}m{s:02d}s"


def generate_gcode(groups: List[PathGroup], settings: AppSettings,
                   source_filename: str = "") -> str:
    """Generate complete G-code string from path groups and settings."""
    pen = settings.pen
    sp = settings.speed
    ox, oy, oz = pen.offset_x, pen.offset_y, pen.offset_z

    # Count paths
    total_paths = sum(len(g.paths) for g in groups)
    total_draw_mm, est_sec = _estimate_time(groups, settings)

    lines: List[str] = []

    # Header
    lines.append("; === Penplot-Gcoder ===")
    if source_filename:
        lines.append(f"; Input: {source_filename}")
    lines.append(f"; Generated: {datetime.now().strftime('%Y-%m-%d %H:%M:%S')}")
    lines.append(f"; Bed: {settings.machine.bed_x}x{settings.machine.bed_y}mm  "
                 f"Pen offset: X={ox} Y={oy} Z={oz}")
    x_min, y_min, x_max, y_max = settings.effective_area()
    lines.append(f"; Effective area: {x_max-x_min:.1f}x{y_max-y_min:.1f}mm")
    lines.append(f"; Paths: {total_paths} / Est. draw time: {_fmt_time(est_sec)}")
    lines.append("")

    # Start G-code
    lines.append("; ---- START G-CODE ----")
    lines.append(_resolve(settings.gcode.start_code, settings))
    lines.append("")

    pen_up_cmd = _resolve(settings.gcode.pen_up_code, settings)
    pen_down_cmd = _resolve(settings.gcode.pen_down_code, settings)

    # Draw paths
    lines.append("; ---- DRAW ----")
    path_index = 0
    for group_idx, group in enumerate(groups):
        if group.pen_change_before and group_idx > 0:
            lines.append(f"; ---- PEN CHANGE (group: {group.label or group.color}) ----")
            lines.append("M0 ; Pause for pen change")
            lines.append("")

        for path in group.paths:
            if not path.points:
                continue
            path_index += 1
            lines.append(f"; Path {path_index}/{total_paths}")

            # Lift pen
            lines.append(pen_up_cmd)

            # Travel to start
            sx = path.points[0][0] + ox
            sy = path.points[0][1] + oy
            lines.append(f"G1 X{sx:.4f} Y{sy:.4f} F{int(sp.travel_speed * 60)}")

            # Pen down
            lines.append(pen_down_cmd)

            # Touchdown delay
            if pen.touchdown_delay > 0:
                lines.append(f"G4 P{pen.touchdown_delay}")

            # Draw
            for pt in path.points:
                px = pt[0] + ox
                py = pt[1] + oy
                lines.append(f"G1 X{px:.4f} Y{py:.4f} F{int(sp.draw_speed * 60)}")

            # Liftup delay
            if pen.liftup_delay > 0:
                lines.append(f"G4 P{pen.liftup_delay}")

            lines.append("")

    # End G-code
    lines.append("; ---- END G-CODE ----")
    lines.append(_resolve(settings.gcode.end_code, settings))
    lines.append("")

    return "\n".join(lines)
