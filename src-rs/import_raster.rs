//! Threshold an image and trace the boundaries of its foreground pixel cells.
//!
//! Paths follow pixel edges (not pixel centers), retain holes and use 4-connected
//! foreground so diagonally touching pixels need not be joined by a drawn line.
use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits};

use crate::{Artwork, ImportOptions, Point, Stroke, TraceMode, centerline};

const MAX_INPUT: usize = 32 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
const MAX_PIXELS: u64 = 8_000_000;
const MAX_EDGES: usize = 1_000_000;
const MAX_RINGS: usize = 20_000;

pub fn raster(bytes: &[u8], options: &ImportOptions) -> Result<Artwork, String> {
    if !options.dpi.is_finite() || !(1.0..=9600.0).contains(&options.dpi) {
        return Err("Raster DPI must be between 1 and 9600.".into());
    }
    if bytes.len() > MAX_INPUT {
        return Err("Raster input exceeds 32 MiB.".into());
    }
    let format = image::guess_format(bytes).map_err(|e| format!("Raster input: {e}"))?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err("Raster input must be PNG or JPEG.".into());
    }
    let reader = || ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(96 * 1024 * 1024);
    let mut header = reader();
    header.limits(limits.clone());
    let (width, height) = header
        .into_dimensions()
        .map_err(|e| format!("Raster dimensions: {e}"))?;
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err("Raster exceeds the 8192 px per side or 8 megapixel limit.".into());
    }
    let mut decoder = reader();
    decoder.limits(limits);
    let rgba = decoder
        .decode()
        .map_err(|e| format!("Raster decoding: {e}"))?
        .into_rgba8();
    let mut mask = rgba
        .pixels()
        .map(|pixel| {
            let [r, g, b, a] = pixel.0.map(u32::from);
            // BT.601 luma, composited over white with one final rounding step.
            let luma = 299 * r + 587 * g + 114 * b;
            let luma = (luma * a + 255_000 * (255 - a) + 127_500) / 255_000;
            u8::from(a != 0 && ((luma <= u32::from(options.threshold)) != options.invert))
        })
        .collect::<Vec<_>>();
    drop(rgba);
    let mm = 25.4 / options.dpi;
    match options.trace {
        TraceMode::Outline => trace(&mut mask, width, height, mm),
        TraceMode::Centerline => {
            let art = centerline::trace(
                &mask,
                width as usize,
                height as usize,
                mm,
                Point::default(),
                "Raster centerlines",
            )?;
            if art.strokes.is_empty() {
                Err("No drawable centerline. Adjust the threshold or use Outline for isolated dots.".into())
            } else {
                Ok(art)
            }
        }
    }
}

fn trace(mask: &mut [u8], width: u32, height: u32, mm: f64) -> Result<Artwork, String> {
    let mut artwork = Artwork::default();
    let stride = width + 1;
    let mut queue = Vec::new();
    let mut edges = Vec::new();
    let mut group = 0;
    let mut edge_total = 0;

    for start in 0..width * height {
        if mask[start as usize] != 1 {
            continue;
        }
        queue.clear();
        edges.clear();
        mask[start as usize] = 2;
        queue.push(start);
        let mut head = 0;
        while let Some(&pixel) = queue.get(head) {
            head += 1;
            let x = pixel % width;
            let y = pixel / width;
            let corner = y * stride + x;
            // Directions E/S/W/N around each foreground cell, clockwise in
            // image coordinates, with the foreground on the right.
            let sides = [
                (y.checked_sub(1).map(|_| pixel - width), corner),
                ((x + 1 < width).then_some(pixel + 1), corner + 1),
                (
                    (y + 1 < height).then_some(pixel + width),
                    corner + stride + 1,
                ),
                (x.checked_sub(1).map(|_| pixel - 1), corner + stride),
            ];
            for (dir, (neighbor, from)) in sides.into_iter().enumerate() {
                if let Some(n) = neighbor {
                    let state = &mut mask[n as usize];
                    if *state == 1 {
                        *state = 2;
                        queue.push(n);
                    }
                    if *state == 2 {
                        continue;
                    }
                }
                edge_total += 1;
                if edge_total > MAX_EDGES {
                    return Err("Raster has too many boundary edges. Reduce resolution or adjust the threshold.".into());
                }
                edges.push(from * 4 + dir as u32);
            }
        }
        edges.sort_unstable();
        let mut used = vec![false; edges.len()];
        for first in 0..edges.len() {
            if used[first] {
                continue;
            }
            if artwork.strokes.len() == MAX_RINGS {
                return Err(
                    "Raster has too many contours. Remove speckle or adjust the threshold.".into(),
                );
            }
            let mut ring = Vec::new();
            let mut index = first;
            loop {
                if used[index] {
                    return Err("Raster boundary could not be closed.".into());
                }
                used[index] = true;
                let (from, dir) = (edges[index] / 4, edges[index] % 4);
                let next = match dir {
                    0 => from + 1,
                    1 => from + stride,
                    2 => from - 1,
                    _ => from - stride,
                };
                // At a corner shared by diagonal pixels take the right-most
                // outgoing edge first, keeping the adjacent ink on the right.
                let out = [1, 0, 3]
                    .into_iter()
                    .find_map(|turn| {
                        let out_dir = (dir + turn) % 4;
                        edges.binary_search(&(next * 4 + out_dir)).ok()
                    })
                    .ok_or("Raster boundary has an open end.")?;
                if (edges[out] % 4) != dir {
                    ring.push(Point::new(
                        f64::from(next % stride) * mm,
                        f64::from(height - next / stride) * mm,
                    ));
                }
                if out == first {
                    break;
                }
                index = out;
            }
            if ring.len() < 4 {
                return Err("Raster boundary has fewer than four corners.".into());
            }
            artwork.strokes.push(Stroke {
                points: ring,
                layer: "Raster contours".into(),
                group,
                closed: true,
                filled: true,
            });
        }
        group += 1;
    }
    if artwork.strokes.is_empty() {
        return Err("No foreground pixels. Adjust the threshold or invert the image.".into());
    }
    Ok(artwork)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> ImportOptions {
        ImportOptions {
            threshold: 127,
            invert: false,
            dpi: 25.4,
            tolerance: 0.1,
            trace: TraceMode::Outline,
        }
    }

    fn png(width: u32, height: u32, data: &[u8]) -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(data, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    fn area(points: &[Point]) -> f64 {
        points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .map(|(p, q)| p.x * q.y - p.y * q.x)
            .sum::<f64>()
            * 0.5
    }

    #[test]
    fn preserves_hole_and_component_group() {
        let mut mask = [1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 1, 1];
        let result = trace(&mut mask, 4, 3, 1.0).unwrap();
        assert_eq!(result.strokes.len(), 2);
        assert_eq!(result.strokes[0].group, result.strokes[1].group);
        assert!(result.strokes.iter().all(|s| s.closed && s.filled));
        assert_eq!(area(&result.strokes[0].points), -10.0);
        assert_eq!(area(&result.strokes[1].points), 1.0);
    }

    #[test]
    fn diagonals_are_separate_and_coordinates_are_y_up() {
        let mut mask = [1, 0, 0, 1];
        let result = trace(&mut mask, 2, 2, 1.0).unwrap();
        assert_eq!(result.strokes.len(), 2);
        assert_ne!(result.strokes[0].group, result.strokes[1].group);
        assert_eq!(result.strokes[0].points.len(), 4);
        assert!(result.strokes[0].points.iter().all(|p| p.y >= 1.0));
        assert!(result.strokes[1].points.iter().all(|p| p.y <= 1.0));
    }

    #[test]
    fn all_four_by_three_masks_conserve_ink_area_at_junctions() {
        for bits in 1u16..(1 << 12) {
            let mut mask = std::array::from_fn::<_, 12, _>(|i| ((bits >> i) & 1) as u8);
            let result = trace(&mut mask, 4, 3, 1.0).unwrap();
            let signed_area = result.strokes.iter().map(|s| area(&s.points)).sum::<f64>();
            assert_eq!(
                signed_area,
                -f64::from(bits.count_ones()),
                "mask {bits:#05x}"
            );
        }
    }

    #[test]
    fn alpha_threshold_inversion_and_dpi_are_applied() {
        let bytes = png(
            4,
            1,
            &[0, 0, 0, 0, 0, 0, 0, 255, 0, 0, 0, 10, 255, 255, 255, 255],
        );
        let mut opts = options();
        let out = raster(&bytes, &opts).unwrap();
        assert_eq!(out.strokes.len(), 1);
        assert!(
            out.strokes[0]
                .points
                .iter()
                .all(|p| p.x >= 1.0 && p.x <= 2.0)
        );
        opts.invert = true;
        opts.dpi = 50.8;
        let out = raster(&bytes, &opts).unwrap();
        assert_eq!(out.strokes.len(), 1);
        assert!(
            out.strokes[0]
                .points
                .iter()
                .all(|p| p.x >= 1.0 && p.x <= 2.0)
        );
        assert_eq!(area(&out.strokes[0].points), -0.5);
    }

    #[test]
    fn centerline_png_uses_threshold_dpi_and_never_adds_contours() {
        let mut px = vec![0; 8 * 3 * 4];
        px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
        let bytes = png(8, 3, &px);
        let mut opts = options();
        let outline = raster(&bytes, &opts).unwrap();
        assert!(outline.strokes[0].filled);
        opts.trace = TraceMode::Centerline;
        let art = raster(&bytes, &opts).unwrap();
        assert_eq!(art.strokes.len(), 1);
        assert!(!art.strokes[0].filled && !art.strokes[0].closed);
        let left = art.strokes[0].points[0];
        assert_eq!(left.y, 1.5);
        opts.dpi *= 2.;
        assert_eq!(raster(&bytes, &opts).unwrap().strokes[0].points[0].y, 0.75);
        opts.invert = true;
        assert!(raster(&bytes, &opts).is_err());
    }

    #[test]
    fn decodes_jpeg_and_thresholds_to_a_contour() {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode(&[0; 64], 8, 8, image::ExtendedColorType::L8)
            .unwrap();
        let result = raster(&bytes, &options()).unwrap();
        assert_eq!(result.strokes.len(), 1);
        assert_eq!(result.strokes[0].points.len(), 4);
        assert_eq!(area(&result.strokes[0].points), -64.0);
    }

    #[test]
    fn fails_cleanly_for_invalid_image_blank_and_bad_dpi() {
        let mut opts = options();
        assert!(raster(b"not a raster", &opts).is_err());
        let blank = png(1, 1, &[255, 255, 255, 255]);
        assert!(raster(&blank, &opts).unwrap_err().contains("No foreground"));
        opts.dpi = f64::NAN;
        assert!(raster(&blank, &opts).unwrap_err().contains("DPI"));
    }
}
