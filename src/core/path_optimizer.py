"""Path optimizer — Greedy nearest-neighbour and 2-opt improvement."""
from __future__ import annotations
import math
from typing import List, Tuple
import numpy as np
from src.models.pen_path import PenPath


def _dist(a: Tuple[float, float], b: Tuple[float, float]) -> float:
    return math.hypot(a[0] - b[0], a[1] - b[1])


def greedy_sort(paths: List[PenPath]) -> List[PenPath]:
    """
    Greedy nearest-neighbour reordering.
    For each path we consider both orientations (start→end and end→start).
    """
    if not paths:
        return []
    remaining = list(paths)
    result: List[PenPath] = []
    current_pos = (0.0, 0.0)

    while remaining:
        best_idx = 0
        best_dist = float("inf")
        best_reversed = False

        for i, p in enumerate(remaining):
            if not p.points:
                continue
            d_start = _dist(current_pos, p.start)
            d_end = _dist(current_pos, p.end)
            if d_start <= d_end:
                d = d_start
                rev = False
            else:
                d = d_end
                rev = True
            if d < best_dist:
                best_dist = d
                best_idx = i
                best_reversed = rev

        chosen = remaining.pop(best_idx)
        if best_reversed:
            chosen = chosen.reversed()
        result.append(chosen)
        current_pos = chosen.end if chosen.end else current_pos

    return result


def two_opt(paths: List[PenPath], max_iter: int = 100) -> List[PenPath]:
    """
    Apply 2-opt improvement to a greedy-sorted path list.
    Tries swapping pairs of paths to reduce total travel distance.
    """
    paths = greedy_sort(paths)
    n = len(paths)
    if n < 4:
        return paths

    def travel_cost(order: List[PenPath]) -> float:
        cost = 0.0
        pos = (0.0, 0.0)
        for p in order:
            if p.start:
                cost += _dist(pos, p.start)
                pos = p.end if p.end else pos
        return cost

    improved = True
    iterations = 0
    while improved and iterations < max_iter:
        improved = False
        iterations += 1
        for i in range(n - 1):
            for j in range(i + 2, n):
                # Try reversing the sub-segment [i+1 .. j]
                new_order = paths[:i + 1] + list(reversed(paths[i + 1:j + 1])) + paths[j + 1:]
                if travel_cost(new_order) < travel_cost(paths) - 1e-6:
                    paths = new_order
                    improved = True
    return paths


def join_paths(paths: List[PenPath], threshold: float) -> List[PenPath]:
    """
    Join paths whose endpoints are within *threshold* mm of each other
    to reduce pen-lift count.
    """
    if threshold <= 0 or not paths:
        return paths

    result: List[PenPath] = []
    remaining = list(paths)

    while remaining:
        current = remaining.pop(0)
        merged = True
        while merged:
            merged = False
            for i, p in enumerate(remaining):
                if not current.points or not p.points:
                    continue
                # end → start
                if _dist(current.end, p.start) <= threshold:
                    current = PenPath(
                        points=current.points + p.points,
                        color=current.color,
                        fill_color=current.fill_color,
                        is_closed=False,
                        layer=current.layer,
                        source_id=current.source_id,
                    )
                    remaining.pop(i)
                    merged = True
                    break
                # end → end (reverse p)
                elif _dist(current.end, p.end) <= threshold:
                    current = PenPath(
                        points=current.points + list(reversed(p.points)),
                        color=current.color,
                        fill_color=current.fill_color,
                        is_closed=False,
                        layer=current.layer,
                        source_id=current.source_id,
                    )
                    remaining.pop(i)
                    merged = True
                    break
        result.append(current)

    return result


def optimize(paths: List[PenPath], algorithm: str = "Greedy",
             join_distance: float = 0.5) -> List[PenPath]:
    """Top-level optimise entry point."""
    if not paths:
        return []
    joined = join_paths(paths, join_distance)
    if algorithm == "2-opt":
        return two_opt(joined)
    return greedy_sort(joined)
