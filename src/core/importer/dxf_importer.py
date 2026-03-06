"""DXF importer — uses ezdxf to read drawing entities."""
from __future__ import annotations
import math
from typing import List, Tuple
from src.models.pen_path import PenPath, PathGroup

try:
    import ezdxf
    from ezdxf.math import Vec3
    EZDXF_OK = True
except ImportError:
    EZDXF_OK = False


def _arc_points(cx: float, cy: float, r: float,
                start_deg: float, end_deg: float,
                precision: float = 0.1) -> List[Tuple[float, float]]:
    """Sample an arc into polyline points."""
    # Ensure start < end
    if end_deg <= start_deg:
        end_deg += 360.0
    arc_len = math.radians(end_deg - start_deg) * r
    n = max(2, int(math.ceil(arc_len / max(precision, 0.001))))
    pts = []
    for i in range(n + 1):
        t = start_deg + (end_deg - start_deg) * i / n
        rad = math.radians(t)
        pts.append((cx + r * math.cos(rad), cy + r * math.sin(rad)))
    return pts


def _circle_points(cx: float, cy: float, r: float,
                   precision: float = 0.1) -> List[Tuple[float, float]]:
    circumference = 2 * math.pi * r
    n = max(8, int(math.ceil(circumference / max(precision, 0.001))))
    pts = []
    for i in range(n + 1):
        t = 2 * math.pi * i / n
        pts.append((cx + r * math.cos(t), cy + r * math.sin(t)))
    return pts


def _spline_points(entity, precision: float = 0.1) -> List[Tuple[float, float]]:
    """Sample a SPLINE entity."""
    try:
        bspline = entity.construction_tool()
        length = sum(
            math.hypot(bspline.point(i/100).x - bspline.point((i+1)/100).x,
                       bspline.point(i/100).y - bspline.point((i+1)/100).y)
            for i in range(100)
        )
        n = max(2, int(math.ceil(length / max(precision, 0.001))))
        pts = []
        for i in range(n + 1):
            t = i / n
            pt = bspline.point(t)
            pts.append((pt.x, pt.y))
        return pts
    except Exception:
        # Fallback: use control points
        try:
            return [(p[0], p[1]) for p in entity.control_points]
        except Exception:
            return []


def _color_for_layer(layer_name: str) -> str:
    """Return a default hex colour for a DXF layer (simplified)."""
    # DXF has 255 ACI colours; we just use black for all layers here
    return "#000000"


def import_dxf(filepath: str, precision: float = 0.1) -> List[PathGroup]:
    """
    Import a DXF file and return a list of PathGroups grouped by layer.

    Supported entities: LINE, POLYLINE, LWPOLYLINE, SPLINE, ARC, CIRCLE.

    Args:
        filepath: Path to the DXF file.
        precision: Curve subdivision precision in mm (default 0.1).

    Returns:
        List of PathGroup objects.
    """
    if not EZDXF_OK:
        raise ImportError("ezdxf is not installed. Run: pip install ezdxf")

    try:
        doc = ezdxf.readfile(filepath)
    except Exception as e:
        raise RuntimeError(f"Failed to read DXF file: {e}") from e

    msp = doc.modelspace()
    groups = {}

    def add_path(pts, layer, is_closed=False, fill_color=None):
        if len(pts) < 2:
            return
        color = _color_for_layer(layer)
        pen_path = PenPath(
            points=pts,
            color=color,
            fill_color=fill_color,
            is_closed=is_closed,
            layer=layer,
        )
        if layer not in groups:
            groups[layer] = PathGroup(color=color, label=layer, paths=[])
        groups[layer].paths.append(pen_path)

    for entity in msp:
        dxftype = entity.dxftype()
        layer = entity.dxf.layer if hasattr(entity.dxf, "layer") else "0"

        try:
            if dxftype == "LINE":
                s = entity.dxf.start
                e = entity.dxf.end
                add_path([(s.x, s.y), (e.x, e.y)], layer)

            elif dxftype in ("POLYLINE", "LWPOLYLINE"):
                try:
                    pts = [(v.x, v.y) for v in entity.vertices()]
                except AttributeError:
                    try:
                        pts = [(v[0], v[1]) for v in entity.get_points()]
                    except Exception:
                        pts = []
                if pts:
                    closed = bool(entity.dxf.get("flags", 0) & 1)
                    if closed and pts[0] != pts[-1]:
                        pts.append(pts[0])
                    add_path(pts, layer, is_closed=closed)

            elif dxftype == "SPLINE":
                pts = _spline_points(entity, precision)
                add_path(pts, layer)

            elif dxftype == "ARC":
                cx, cy = entity.dxf.center.x, entity.dxf.center.y
                r = entity.dxf.radius
                start_a = entity.dxf.start_angle
                end_a = entity.dxf.end_angle
                pts = _arc_points(cx, cy, r, start_a, end_a, precision)
                add_path(pts, layer)

            elif dxftype == "CIRCLE":
                cx, cy = entity.dxf.center.x, entity.dxf.center.y
                r = entity.dxf.radius
                pts = _circle_points(cx, cy, r, precision)
                add_path(pts, layer, is_closed=True)

            elif dxftype == "ELLIPSE":
                try:
                    ellipse = entity.construction_tool()
                    circumference = ellipse.params.major_axis_length * 2 * math.pi
                    n = max(8, int(math.ceil(circumference / max(precision, 0.001))))
                    pts = []
                    for i in range(n + 1):
                        t = 2 * math.pi * i / n
                        pt = ellipse.point(t)
                        pts.append((pt.x, pt.y))
                    add_path(pts, layer, is_closed=True)
                except Exception:
                    pass

        except Exception:
            # Skip entities that can't be processed
            continue

    return list(groups.values())
