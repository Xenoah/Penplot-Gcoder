"""Fill pattern generator using Shapely."""
from __future__ import annotations
import math
from typing import List, Optional
import numpy as np

try:
    from shapely.geometry import Polygon, MultiPolygon, LineString, MultiLineString, Point
    from shapely.ops import unary_union
    from shapely.affinity import rotate as shapely_rotate
    SHAPELY_OK = True
except ImportError:
    SHAPELY_OK = False

from src.models.pen_path import PenPath
from src.models.settings import FillSettings


def _poly_from_path(path: PenPath):
    """Convert a closed PenPath to a Shapely Polygon (or None)."""
    if not SHAPELY_OK:
        return None
    pts = path.points
    if len(pts) < 3:
        return None
    try:
        poly = Polygon(pts)
        if not poly.is_valid:
            poly = poly.buffer(0)
        return poly
    except Exception:
        return None


def _lines_to_paths(lines, color: str) -> List[PenPath]:
    """Convert Shapely geometry (Line/MultiLine) to PenPath list."""
    result = []
    if lines is None or lines.is_empty:
        return result
    if lines.geom_type == "LineString":
        pts = list(lines.coords)
        if len(pts) >= 2:
            result.append(PenPath(points=pts, color=color))
    elif lines.geom_type in ("MultiLineString", "GeometryCollection"):
        for geom in lines.geoms:
            result.extend(_lines_to_paths(geom, color))
    return result


def _hatch_lines(polygon, angle_deg: float, spacing: float, margin: float, color: str) -> List[PenPath]:
    """Generate parallel hatch lines clipped to polygon."""
    if not SHAPELY_OK or polygon is None or polygon.is_empty:
        return []
    if margin > 0:
        polygon = polygon.buffer(-margin)
        if polygon is None or polygon.is_empty:
            return []

    # Rotate polygon to axis-aligned, generate horizontal lines, rotate back
    bounds = polygon.bounds  # minx, miny, maxx, maxy
    diag = math.hypot(bounds[2] - bounds[0], bounds[3] - bounds[1]) * 1.5
    cx = (bounds[0] + bounds[2]) / 2
    cy = (bounds[1] + bounds[3]) / 2

    rotated_poly = shapely_rotate(polygon, -angle_deg, origin=(cx, cy))
    rb = rotated_poly.bounds
    y = rb[1]
    lines = []
    while y <= rb[3] + spacing:
        line = LineString([(rb[0] - diag, y), (rb[2] + diag, y)])
        clipped = rotated_poly.intersection(line)
        if not clipped.is_empty:
            # Rotate clipped segment back
            clipped_back = shapely_rotate(clipped, angle_deg, origin=(cx, cy))
            lines.extend(_lines_to_paths(clipped_back, color))
        y += spacing
    return lines


def generate_fill(path: PenPath, settings: FillSettings) -> List[PenPath]:
    """Generate fill paths for a single closed PenPath."""
    if not SHAPELY_OK:
        return []
    polygon = _poly_from_path(path)
    if polygon is None or polygon.is_empty:
        return []

    pattern = settings.pattern
    spacing = max(0.01, settings.spacing)
    angle = settings.angle
    margin = settings.margin
    color = path.color

    if pattern == "Lines":
        return _hatch_lines(polygon, angle, spacing, margin, color)

    elif pattern == "Cross Hatch":
        lines1 = _hatch_lines(polygon, angle, spacing, margin, color)
        lines2 = _hatch_lines(polygon, angle + settings.cross_hatch_angle2, spacing, margin, color)
        return lines1 + lines2

    elif pattern == "Tri Hatch":
        lines1 = _hatch_lines(polygon, 0.0, spacing, margin, color)
        lines2 = _hatch_lines(polygon, 60.0, spacing, margin, color)
        lines3 = _hatch_lines(polygon, 120.0, spacing, margin, color)
        return lines1 + lines2 + lines3

    elif pattern == "Zigzag":
        # Generate zigzag by connecting alternating hatch lines
        raw = _hatch_lines(polygon, angle, spacing, margin, color)
        if len(raw) < 2:
            return raw
        result = []
        zigzag_pts = []
        for i, seg in enumerate(raw):
            pts = seg.points
            if i % 2 == 0:
                zigzag_pts.extend(pts)
            else:
                zigzag_pts.extend(reversed(pts))
        if zigzag_pts:
            result.append(PenPath(points=zigzag_pts, color=color))
        return result

    elif pattern == "Contour Offset":
        result = []
        inner = polygon
        if margin > 0:
            inner = inner.buffer(-margin)
        step = 0
        while True:
            if inner is None or inner.is_empty:
                break
            try:
                exterior = inner.exterior
                pts = list(exterior.coords)
            except AttributeError:
                # MultiPolygon
                pts = []
                if hasattr(inner, 'geoms'):
                    for g in inner.geoms:
                        pts.extend(list(g.exterior.coords))
            if len(pts) >= 2:
                innermost = inner.buffer(-spacing)
                is_innermost = innermost is None or innermost.is_empty
                if is_innermost and settings.contour_innermost == "skip":
                    break
                result.append(PenPath(points=pts, color=color, is_closed=True))
            inner = inner.buffer(-spacing)
            if inner is None or inner.is_empty:
                break
            step += 1
            if step > 1000:
                break
        return result

    elif pattern == "Dots":
        result = []
        if margin > 0:
            poly_inner = polygon.buffer(-margin)
        else:
            poly_inner = polygon
        if poly_inner is None or poly_inner.is_empty:
            return []
        bounds = poly_inner.bounds
        sx = spacing
        sy = settings.dots_spacing_y
        stagger = settings.dots_grid == "stagger"
        y = bounds[1]
        row = 0
        while y <= bounds[3]:
            x_start = bounds[0]
            if stagger and row % 2 == 1:
                x_start += sx / 2
            x = x_start
            while x <= bounds[2]:
                pt = Point(x, y)
                if poly_inner.contains(pt):
                    # tiny line segment to represent a dot
                    result.append(PenPath(points=[(x, y), (x + 0.01, y)], color=color))
                x += sx
            y += sy
            row += 1
        return result

    elif pattern == "Spiral":
        result = []
        center = polygon.centroid
        cx, cy = center.x, center.y
        # approximate bounding radius
        max_r = 0.0
        if margin > 0:
            inner_poly = polygon.buffer(-margin)
        else:
            inner_poly = polygon
        if inner_poly is None or inner_poly.is_empty:
            return []
        for pt in inner_poly.exterior.coords:
            r = math.hypot(pt[0] - cx, pt[1] - cy)
            max_r = max(max_r, r)
        pts = []
        r = max_r
        theta = 0.0
        while r > 0:
            x = cx + r * math.cos(theta)
            y = cy + r * math.sin(theta)
            p = Point(x, y)
            pts.append((x, y))
            # advance
            arc_step = spacing / max(r, 0.01)
            theta += arc_step
            if theta >= 2 * math.pi:
                theta -= 2 * math.pi
                r -= spacing
        if len(pts) >= 2:
            result.append(PenPath(points=pts, color=color))
        return result

    return []


def generate_fills_for_paths(paths: List[PenPath], settings: FillSettings) -> List[PenPath]:
    """Generate fill paths for a list of PenPaths according to fill settings."""
    if not settings.enabled:
        return []
    fill_paths = []
    for p in paths:
        if not p.is_closed:
            continue
        if settings.target == "fill_color" and p.fill_color is None:
            continue
        fill_paths.extend(generate_fill(p, settings))
    return fill_paths
