//! Optional image treatments. Legacy global thresholding remains unchanged.
use crate::{Artwork, ImportOptions, check_size, import_raster};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RasterMode {
    #[default]
    Threshold,
    CleanLine,
    Color,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RasterProcessing {
    pub mode: RasterMode,
    /// 0 disables physical-width filtering; otherwise millimeters at import DPI.
    #[serde(default, skip_serializing_if = "width_off")]
    pub pen_width_mm: f64,
    /// Local background radius in source pixels, independent of output DPI.
    pub radius: u32,
    pub contrast: u8,
    /// Remove whole 8-connected components of at most this many pixels.
    pub speckle: u32,
    pub palette_size: u8,
    pub exclude_background: bool,
    /// Exclude near-white pixels only if all composited channels reach this value.
    pub background_cutoff: u8,
}
fn width_off(value: &f64) -> bool {
    *value == 0.
}
impl Default for RasterProcessing {
    fn default() -> Self {
        Self {
            mode: RasterMode::Threshold,
            pen_width_mm: 0.,
            radius: 16,
            contrast: 12,
            speckle: 3,
            palette_size: 4,
            exclude_background: true,
            background_cutoff: 245,
        }
    }
}
impl RasterProcessing {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    pub fn validate(&self) -> Result<(), String> {
        if !self.pen_width_mm.is_finite() || !(0.0..=3.).contains(&self.pen_width_mm) {
            return Err("Image pen width must be 0 (off) to 3 mm".into());
        }
        if !(2..=128).contains(&self.radius)
            || !(1..=80).contains(&self.contrast)
            || self.speckle > 100
            || !(2..=8).contains(&self.palette_size)
            || self.background_cutoff < 128
        {
            return Err("Image cleanup requires radius 2..128 px, contrast 1..80, speckle 0..100 px, palette 2..8 and background cutoff 128..255.".into());
        }
        Ok(())
    }
}
fn rgb(pixel: &image::Rgba<u8>) -> [u8; 3] {
    let a = u32::from(pixel[3]);
    std::array::from_fn(|c| ((u32::from(pixel[c]) * a + 255 * (255 - a) + 127) / 255) as u8)
}
pub(crate) fn clean_mask(image: &RgbaImage, options: &ImportOptions) -> Vec<u8> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let luma: Vec<_> = image
        .pixels()
        .map(|p| {
            let [r, g, b] = rgb(p).map(u32::from);
            let v = ((299 * r + 587 * g + 114 * b + 500) / 1000) as u8;
            if options.invert { 255 - v } else { v }
        })
        .collect();
    // Summed-area means give O(pixels) correction without eroding thin strokes.
    let stride = w + 1;
    let mut integral = vec![0u64; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0u64;
        for x in 0..w {
            row += u64::from(luma[y * w + x]);
            integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + row;
        }
    }
    let radius = options.processing.radius as usize;
    let mut mask = vec![0; w * h];
    for y in 0..h {
        for x in 0..w {
            if image.get_pixel(x as u32, y as u32)[3] == 0 {
                continue;
            }
            let (x0, y0, x1, y1) = (
                x.saturating_sub(radius),
                y.saturating_sub(radius),
                (x + radius + 1).min(w),
                (y + radius + 1).min(h),
            );
            let sum = integral[y1 * stride + x1] + integral[y0 * stride + x0]
                - integral[y0 * stride + x1]
                - integral[y1 * stride + x0];
            let count = ((x1 - x0) * (y1 - y0)) as u64;
            mask[y * w + x] = u8::from(
                (u64::from(luma[y * w + x]) + u64::from(options.processing.contrast)) * count < sum,
            );
        }
    }
    remove_speckles(&mut mask, w, h, options.processing.speckle as usize);
    mask
}
fn remove_speckles(mask: &mut [u8], w: usize, h: usize, max_size: usize) {
    if max_size == 0 {
        return;
    }
    let mut queue = Vec::new();
    for start in 0..mask.len() {
        if mask[start] != 1 {
            continue;
        }
        queue.clear();
        queue.push(start);
        mask[start] = 2;
        let mut head = 0;
        while head < queue.len() {
            let i = queue[head];
            head += 1;
            let (x, y) = (i % w, i / w);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let next = ny * w + nx;
                    if mask[next] == 1 {
                        mask[next] = 2;
                        queue.push(next);
                    }
                }
            }
        }
        if queue.len() <= max_size {
            for &i in &queue {
                mask[i] = 0;
            }
        }
    }
    for p in mask {
        *p = u8::from(*p != 0);
    }
}
#[derive(Clone)]
struct Bin {
    count: u64,
    sum: [u64; 3],
}
impl Bin {
    fn mean(&self) -> [u8; 3] {
        self.sum.map(|v| ((v + self.count / 2) / self.count) as u8)
    }
}
fn centroid(bins: &[Bin]) -> [u8; 3] {
    let count: u64 = bins.iter().map(|b| b.count).sum();
    std::array::from_fn(|c| {
        ((bins.iter().map(|b| b.sum[c]).sum::<u64>() + count / 2) / count) as u8
    })
}
fn distance(a: [u8; 3], b: [u8; 3]) -> u32 {
    (0..3)
        .map(|c| (i32::from(a[c]) - i32::from(b[c])).pow(2) as u32)
        .sum()
}
fn visible_rgb(p: &image::Rgba<u8>, o: &RasterProcessing) -> Option<[u8; 3]> {
    let rgb = rgb(p);
    (p[3] != 0 && !(o.exclude_background && rgb.iter().all(|c| *c >= o.background_cutoff)))
        .then_some(rgb)
}
fn bin_id(rgb: [u8; 3]) -> usize {
    ((usize::from(rgb[0]) >> 3) << 10)
        | ((usize::from(rgb[1]) >> 3) << 5)
        | (usize::from(rgb[2]) >> 3)
}
pub(crate) fn colors(image: &RgbaImage, options: &ImportOptions) -> Result<Artwork, String> {
    let o = &options.processing;
    let mut histogram = vec![
        Bin {
            count: 0,
            sum: [0; 3]
        };
        32768
    ];
    for p in image.pixels() {
        if let Some(rgb) = visible_rgb(p, o) {
            let bin = &mut histogram[bin_id(rgb)];
            bin.count += 1;
            for (c, value) in rgb.into_iter().enumerate() {
                bin.sum[c] += u64::from(value);
            }
        }
    }
    let bins: Vec<_> = histogram.iter().filter(|b| b.count > 0).cloned().collect();
    if bins.is_empty() {
        return Err(
            "No color regions. Disable white background exclusion or raise its cutoff.".into(),
        );
    }
    // Weighted median cut, deterministic tie-breaking and no random seeds.
    let mut boxes = vec![bins];
    while boxes.len() < usize::from(o.palette_size) {
        let choice = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .map(|(i, b)| {
                let ranges: [u8; 3] = std::array::from_fn(|c| {
                    b.iter().map(|v| v.mean()[c]).max().unwrap()
                        - b.iter().map(|v| v.mean()[c]).min().unwrap()
                });
                let channel = (0..3).max_by_key(|c| ranges[*c]).unwrap();
                (
                    i,
                    channel,
                    u64::from(ranges[channel]) * b.iter().map(|v| v.count).sum::<u64>(),
                )
            })
            .max_by_key(|v| v.2);
        let Some((i, channel, _)) = choice else { break };
        let mut group = boxes.remove(i);
        group.sort_by_key(|b| (b.mean()[channel], b.mean()));
        let half = group.iter().map(|b| b.count).sum::<u64>().div_ceil(2);
        let mut total = 0;
        let cut = (1..group.len())
            .find(|j| {
                total += group[j - 1].count;
                total >= half
            })
            .unwrap_or(group.len() - 1);
        let other = group.split_off(cut);
        boxes.push(group);
        boxes.push(other);
    }
    let mut palette: Vec<_> = boxes.iter().map(|b| centroid(b)).collect();
    palette.sort_unstable();
    palette.dedup();
    let labels: Vec<_> = histogram
        .iter()
        .map(|b| {
            if b.count == 0 {
                return 0;
            }
            palette
                .iter()
                .enumerate()
                .min_by_key(|(_, p)| distance(b.mean(), **p))
                .unwrap()
                .0 as u8
        })
        .collect();
    let pixels: Vec<_> = image
        .pixels()
        .map(|p| visible_rgb(p, o).map_or(255, |rgb| labels[bin_id(rgb)]))
        .collect();
    let mut art = Artwork::default();
    for (i, color) in palette.into_iter().enumerate() {
        let mut mask: Vec<_> = pixels
            .iter()
            .map(|p| u8::from(usize::from(*p) == i))
            .collect();
        if !mask.contains(&1) {
            continue;
        }
        crate::pen_width::open_mask(
            &mut mask,
            image.width() as usize,
            image.height() as usize,
            options.processing.pen_width_mm * options.dpi / 25.4,
        )?;
        if !mask.contains(&1) {
            continue;
        }
        let mut region =
            import_raster::trace(&mut mask, image.width(), image.height(), 25.4 / options.dpi)?;
        let name = format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2]);
        for s in &mut region.strokes {
            s.layer = name.clone();
        }
        crate::append_artwork(&mut art.strokes, &region.strokes)?;
        check_size(&art.strokes)?;
    }
    Ok(art)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    #[test]
    fn shaded_scan_keeps_faint_one_pixel_t_removes_dots_and_transparency() {
        let mut image = RgbaImage::from_fn(160, 100, |x, _| {
            let v = 150 + (x / 2) as u8;
            Rgba([v, v, v, 255])
        });
        let mut expected = vec![0; 160 * 100];
        for x in 20..140 {
            let v = image.get_pixel(x, 40)[0] - 35;
            image.put_pixel(x, 40, Rgba([v, v, v, 255]));
            expected[40 * 160 + x as usize] = 1;
        }
        for y in 41..80 {
            let v = image.get_pixel(80, y)[0] - 35;
            image.put_pixel(80, y, Rgba([v, v, v, 255]));
            expected[y as usize * 160 + 80] = 1;
        }
        image.put_pixel(50, 10, Rgba([0, 0, 0, 255]));
        image.put_pixel(51, 10, Rgba([0, 0, 0, 255]));
        image.put_pixel(110, 10, Rgba([0, 0, 0, 0]));
        let mut options = ImportOptions::default();
        options.processing.mode = RasterMode::CleanLine;
        let mask = clean_mask(&image, &options);
        assert_eq!(mask, expected);
        let paths = crate::centerline::trace(&mask, 160, 100, 1., crate::Point::default(), "clean")
            .unwrap();
        assert_eq!(paths.strokes.len(), 3);
        assert!(paths.strokes.iter().all(|s| !s.closed && !s.filled));
        let length: f64 = paths
            .strokes
            .iter()
            .flat_map(|s| s.points.windows(2))
            .map(|p| p[0].distance(p[1]))
            .sum();
        assert_eq!(length, 158.); // 119-pixel bar + 39-pixel stem, each edge once.
        options.processing.speckle = 0;
        assert_eq!(clean_mask(&image, &options)[10 * 160 + 50], 1);
        options.processing.contrast = 50;
        assert_eq!(clean_mask(&image, &options)[40 * 160 + 80], 0);
    }
    #[test]
    fn legacy_import_defaults_and_invalid_cleanup_settings() {
        let old = r#"{"threshold":128,"invert":false,"dpi":96.0,"tolerance":0.1}"#;
        let mut opts: ImportOptions = serde_json::from_str(old).unwrap();
        assert_eq!(opts, ImportOptions::default());
        assert!(!serde_json::to_string(&opts).unwrap().contains("processing"));
        opts.processing.radius = 0;
        assert!(opts.processing.validate().is_err());
        opts.processing.radius = 16;
        opts.processing.palette_size = 9;
        assert!(opts.processing.validate().is_err());
    }
}
