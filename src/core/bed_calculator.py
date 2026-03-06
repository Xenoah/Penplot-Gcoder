"""Bed / effective drawing area calculations."""
from __future__ import annotations
from typing import Tuple
from src.models.settings import AppSettings


def effective_area(settings: AppSettings) -> Tuple[float, float, float, float]:
    """
    Return (x_min, y_min, x_max, y_max) of the effective drawing area.

    Formula:
        X_min = max(0, -offset_x)
        X_max = min(bed_x, bed_x - offset_x)
        Y_min = max(0, -offset_y)
        Y_max = min(bed_y, bed_y - offset_y)
    """
    ox = settings.pen.offset_x
    oy = settings.pen.offset_y
    bx = settings.machine.bed_x
    by = settings.machine.bed_y

    x_min = max(0.0, -ox)
    x_max = min(bx, bx - ox)
    y_min = max(0.0, -oy)
    y_max = min(by, by - oy)
    return x_min, y_min, x_max, y_max


def effective_size(settings: AppSettings) -> Tuple[float, float]:
    """Return (width, height) of the effective drawing area."""
    x_min, y_min, x_max, y_max = effective_area(settings)
    return x_max - x_min, y_max - y_min


def paths_in_area(paths, settings: AppSettings) -> bool:
    """Return True if ALL path points are inside the effective drawing area."""
    x_min, y_min, x_max, y_max = effective_area(settings)
    for path in paths:
        for x, y in path.points:
            if x < x_min or x > x_max or y < y_min or y > y_max:
                return False
    return True


def paths_overflow(paths, settings: AppSettings) -> bool:
    """Return True if any path point falls outside the effective drawing area."""
    return not paths_in_area(paths, settings)
