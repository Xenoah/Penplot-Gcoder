"""SVG importer — uses svgpathtools to read paths and group by colour."""
from __future__ import annotations
import math
import re
from typing import List, Optional, Tuple, Dict
from src.models.pen_path import PenPath, PathGroup

try:
    import svgpathtools
    from svgpathtools import svg2paths2, Path, Line, CubicBezier, QuadraticBezier, Arc
    SVGPATHTOOLS_OK = True
except ImportError:
    SVGPATHTOOLS_OK = False


def _normalise_color(color: Optional[str]) -> str:
    """Normalise SVG colour string to lowercase hex or 'none'."""
    if not color or color.lower() in ("none", "transparent", ""):
        return "none"
    color = color.strip()
    # named colours -> hex (minimal set)
    named = {"black": "#000000", "white": "#ffffff", "red": "#ff0000",
              "green": "#008000", "blue": "#0000ff", "gray": "#808080",
              "grey": "#808080"}
    if color.lower() in named:
        return named[color.lower()]
    if not color.startswith("#"):
        return "#000000"
    return color.lower()


def _sample_path_segment(segment, precision: float) -> List[Tuple[float, float]]:
    """Sample a single svgpathtools segment into (x, y) points."""
    pts = []
    if isinstance(segment, Line):
        pts.append((segment.start.real, segment.start.imag))
        pts.append((segment.end.real, segment.end.imag))
    else:
        # Adaptive sampling for curves
        length = segment.length(error=1e-3)
        n_steps = max(2, int(math.ceil(length / max(precision, 0.001))))
        for i in range(n_steps + 1):
            t = i / n_steps
            pt = segment.point(t)
            pts.append((pt.real, pt.imag))
    return pts


def _svgpath_to_polylines(svg_path: "Path", precision: float):
    """Convert an svgpathtools Path to a list of polylines (one per continuous segment)."""
    if not svg_path:
        return
    all_pts: List[Tuple[float, float]] = []
    prev_end = None

    for seg in svg_path:
        seg_start = (seg.start.real, seg.start.imag)
        # Start new sub-path if there is a gap
        if prev_end is not None:
            dx = seg_start[0] - prev_end[0]
            dy = seg_start[1] - prev_end[1]
            if math.hypot(dx, dy) > 1e-4:
                # gap — new sub-path
                if all_pts:
                    yield all_pts
                all_pts = []
        pts = _sample_path_segment(seg, precision)
        if all_pts and pts:
            # avoid duplicate endpoint
            all_pts.extend(pts[1:])
        else:
            all_pts.extend(pts)
        prev_end = (seg.end.real, seg.end.imag)

    if all_pts:
        yield all_pts


def _is_closed(svg_path) -> bool:
    try:
        return svg_path.isclosed()
    except Exception:
        if not svg_path:
            return False
        start = svg_path[0].start
        end = svg_path[-1].end
        return abs(start - end) < 1e-4


def import_svg(filepath: str, precision: float = 0.1) -> List[PathGroup]:
    """
    Import an SVG file and return a list of PathGroups grouped by stroke colour.

    Args:
        filepath: Path to the SVG file.
        precision: Curve subdivision precision in mm (default 0.1).

    Returns:
        List of PathGroup objects.
    """
    if not SVGPATHTOOLS_OK:
        raise ImportError("svgpathtools is not installed. Run: pip install svgpathtools")

    try:
        paths, attributes, svg_attributes = svg2paths2(filepath)
    except Exception as e:
        raise RuntimeError(f"Failed to read SVG file: {e}") from e

    # SVG coordinate system: Y increases downward.
    # We flip Y so that Y increases upward (standard plotter convention).
    # We need the SVG height to flip correctly.
    svg_height = None
    if svg_attributes:
        h_str = svg_attributes.get("height", "")
        m = re.search(r"([\d.]+)", str(h_str))
        if m:
            try:
                svg_height = float(m.group(1))
            except ValueError:
                pass
    if svg_height is None:
        # Try viewBox
        vb = svg_attributes.get("viewBox", "") if svg_attributes else ""
        parts = re.split(r"[\s,]+", str(vb).strip())
        if len(parts) == 4:
            try:
                svg_height = float(parts[3])
            except ValueError:
                pass
    if svg_height is None:
        svg_height = 0.0

    # Group by stroke colour
    groups: Dict[str, PathGroup] = {}

    for svg_path, attr in zip(paths, attributes):
        if not svg_path:
            continue

        stroke = _normalise_color(attr.get("stroke", None))
        fill = _normalise_color(attr.get("fill", None))
        path_id = attr.get("id", "")

        # Determine the effective drawing colour
        draw_color = stroke if stroke != "none" else (fill if fill != "none" else "#000000")

        closed = _is_closed(svg_path)
        fill_color = fill if fill != "none" else None

        for poly_pts in _svgpath_to_polylines(svg_path, precision):
            if len(poly_pts) < 2:
                continue
            # Flip Y axis
            if svg_height > 0:
                poly_pts = [(x, svg_height - y) for x, y in poly_pts]

            pen_path = PenPath(
                points=poly_pts,
                color=draw_color,
                fill_color=fill_color,
                is_closed=closed,
                source_id=path_id,
            )

            if draw_color not in groups:
                groups[draw_color] = PathGroup(
                    color=draw_color,
                    label=draw_color,
                    paths=[],
                )
            groups[draw_color].paths.append(pen_path)

    return list(groups.values())
