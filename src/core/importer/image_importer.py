"""Image importer — uses OpenCV to extract contours from PNG/JPG."""
from __future__ import annotations
from typing import List
from src.models.pen_path import PenPath, PathGroup

try:
    import cv2
    import numpy as np
    CV2_OK = True
except ImportError:
    CV2_OK = False


def import_image(filepath: str, threshold: int = 127,
                 simplify_epsilon: float = 0.5) -> List[PathGroup]:
    """
    Import a raster image by extracting contours using OpenCV.

    Args:
        filepath: Path to a PNG or JPG file.
        threshold: Binary threshold (0-255).
        simplify_epsilon: Contour simplification tolerance in pixels.

    Returns:
        List of PathGroup objects (single group, black colour).
    """
    if not CV2_OK:
        raise ImportError("opencv-python is not installed. Run: pip install opencv-python")

    img = cv2.imread(filepath, cv2.IMREAD_GRAYSCALE)
    if img is None:
        raise RuntimeError(f"Failed to read image file: {filepath}")

    h, w = img.shape

    # Binarise — assume dark lines on light background
    _, binary = cv2.threshold(img, threshold, 255, cv2.THRESH_BINARY_INV)

    # Find contours
    contours, _ = cv2.findContours(binary, cv2.RETR_LIST, cv2.CHAIN_APPROX_TC89_KCOS)

    paths = []
    for contour in contours:
        if len(contour) < 2:
            continue
        # Optionally simplify
        if simplify_epsilon > 0:
            contour = cv2.approxPolyDP(contour, simplify_epsilon, closed=True)
        if len(contour) < 2:
            continue

        pts = [(float(pt[0][0]), float(h - pt[0][1])) for pt in contour]  # flip Y
        # Close the contour
        if pts[0] != pts[-1]:
            pts.append(pts[0])

        paths.append(PenPath(
            points=pts,
            color="#000000",
            fill_color=None,
            is_closed=True,
        ))

    if not paths:
        return []

    group = PathGroup(color="#000000", label="Contours", paths=paths)
    return [group]
