"""Image importer — OpenCV contour extraction with Canny edge detection for line art."""
from __future__ import annotations
from typing import List
from src.models.pen_path import PenPath, PathGroup

try:
    import cv2
    import numpy as np
    CV2_OK = True
except ImportError:
    CV2_OK = False


def import_image(filepath: str,
                 threshold: int = 127,
                 simplify_epsilon: float = 0.5,
                 mode: str = "auto") -> List[PathGroup]:
    """
    Import a raster image by extracting contours / edges using OpenCV.

    Args:
        filepath:        Path to a PNG or JPG file.
        threshold:       Binary threshold value (0-255) for 'threshold' mode.
        simplify_epsilon: Contour simplification tolerance in pixels.
        mode:            'auto'      — use Canny if image looks like line art,
                                       otherwise binary threshold.
                         'canny'     — Canny edge detection (best for line drawings).
                         'threshold' — binary threshold (best for filled shapes / logos).
    """
    if not CV2_OK:
        raise ImportError("opencv-python is not installed. Run: pip install opencv-python")

    img = cv2.imread(filepath, cv2.IMREAD_GRAYSCALE)
    if img is None:
        raise RuntimeError(f"Failed to read image file: {filepath}")

    h, w = img.shape

    # ── Determine mode ──────────────────────────────────────────────────────
    if mode == "auto":
        # Heuristic: if the image has a mostly light background and dark lines,
        # use Canny (line art). Otherwise threshold.
        mean_val = float(np.mean(img))
        mode = "canny" if mean_val > 180 else "threshold"

    # ── Edge / contour extraction ────────────────────────────────────────────
    if mode == "canny":
        # Gentle Gaussian blur to reduce noise, then Canny edge detection
        blurred = cv2.GaussianBlur(img, (3, 3), 0)
        # Otsu threshold to pick good Canny thresholds automatically
        otsu_thr, _ = cv2.threshold(blurred, 0, 255,
                                    cv2.THRESH_BINARY + cv2.THRESH_OTSU)
        edges   = cv2.Canny(blurred,
                            threshold1=max(1, otsu_thr * 0.5),
                            threshold2=otsu_thr,
                            apertureSize=3)
        # Dilate edges slightly so findContours connects thin lines
        kernel  = cv2.getStructuringElement(cv2.MORPH_RECT, (2, 2))
        edges   = cv2.dilate(edges, kernel, iterations=1)
        binary  = edges
    else:
        # Binary threshold — dark lines on light background
        _, binary = cv2.threshold(img, threshold, 255, cv2.THRESH_BINARY_INV)

    # ── Find contours ────────────────────────────────────────────────────────
    contours, _ = cv2.findContours(binary, cv2.RETR_LIST, cv2.CHAIN_APPROX_TC89_KCOS)

    paths = []
    for contour in contours:
        if len(contour) < 2:
            continue
        if simplify_epsilon > 0:
            contour = cv2.approxPolyDP(contour, simplify_epsilon, closed=True)
        if len(contour) < 2:
            continue

        # Flip Y so Y increases upward (plotter convention)
        pts = [(float(pt[0][0]), float(h - pt[0][1])) for pt in contour]
        if pts[0] != pts[-1]:
            pts.append(pts[0])   # close the contour

        paths.append(PenPath(
            points=pts,
            color="#000000",
            fill_color=None,
            is_closed=True,
        ))

    if not paths:
        return []

    group = PathGroup(color="#000000", label=f"Contours ({mode})", paths=paths)
    return [group]
