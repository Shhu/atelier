//! Filters of the Filter menu: blurs, sharpening, noise, high pass, offset,
//! minimum and maximum, emboss, find edges, clouds, mosaic — with
//! Photoshop's settings. 8 bits per channel like the layers; colors are
//! averaged premultiplied by alpha, so that the invisible color of
//! transparent pixels never bleeds into the visible ones. Beyond the edges
//! of the image, its edge pixels repeat.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::pixel_ops::{Clip, mix, threads};

/// A filter and its settings, in Photoshop's units: radii, distances and
/// heights in pixels, amounts in %, thresholds in levels (0 to 255), angles
/// in degrees (counterclockwise, 0 pointing right). Serialized for the
/// Actions panel, its kind as the `filter` field.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "filter", rename_all = "kebab-case")]
pub enum Filter {
    GaussianBlur {
        radius: f32,
    },
    BoxBlur {
        radius: f32,
    },
    MotionBlur {
        angle: f32,
        distance: f32,
    },
    /// Blurs the areas, keeps the edges: only neighbors whose value is
    /// within about `threshold` levels count.
    SurfaceBlur {
        radius: f32,
        threshold: f32,
    },
    UnsharpMask {
        amount: f32,
        radius: f32,
        threshold: f32,
    },
    SmartSharpen(SmartSharpen),
    AddNoise(AddNoise),
    ReduceNoise(ReduceNoise),
    Median {
        radius: f32,
    },
    /// Median, only where a pixel differs from it by more than `threshold`.
    DustAndScratches {
        radius: f32,
        threshold: f32,
    },
    HighPass {
        radius: f32,
    },
    Offset(Offset),
    Minimum(Morphology),
    Maximum(Morphology),
    /// Gray relief lit from `angle`.
    Emboss {
        angle: f32,
        height: f32,
        amount: f32,
    },
    FindEdges,
    Clouds(Clouds),
    Mosaic {
        cell: f32,
    },
}

/// Smart Sharpen.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SmartSharpen {
    pub amount: f32,
    pub radius: f32,
    /// Sharpens less the small differences (noise), in %.
    pub reduce_noise: f32,
    pub remove: Remove,
    /// Direction of the motion blur to remove.
    pub angle: f32,
    /// Less sharpening in the shadows, and in the highlights.
    pub shadows: ToneFade,
    pub highlights: ToneFade,
}

/// Blur that Smart Sharpen removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Remove {
    Gaussian,
    Lens,
    Motion,
}

/// Fading of the sharpening in a tonal range (Smart Sharpen).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToneFade {
    /// In %; 0: no fading.
    pub fade: f32,
    /// Extent of the range, in % of half the tones.
    pub tonal_width: f32,
    /// Size of the area whose tone decides, in pixels.
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AddNoise {
    pub amount: f32,
    /// Gaussian distribution, else uniform.
    pub gaussian: bool,
    /// The same noise on the three channels.
    pub monochromatic: bool,
    pub seed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReduceNoise {
    /// 0 to 10.
    pub strength: f32,
    /// The others in %.
    pub preserve_details: f32,
    pub color_noise: f32,
    pub sharpen_details: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Offset {
    /// Pixels to the right, and down.
    pub horizontal: f32,
    pub vertical: f32,
    pub undefined: Undefined,
}

/// What fills the area an offset uncovers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Undefined {
    Transparent,
    RepeatEdges,
    WrapAround,
}

/// Minimum and Maximum: the darkest (lightest) value around each pixel.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Morphology {
    pub radius: f32,
    /// A round neighborhood, else square.
    pub roundness: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clouds {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub seed: u32,
}

/// The filters, as the menu lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FilterKind {
    BoxBlur,
    GaussianBlur,
    MotionBlur,
    SurfaceBlur,
    AddNoise,
    DustAndScratches,
    Median,
    ReduceNoise,
    Mosaic,
    Clouds,
    SmartSharpen,
    UnsharpMask,
    Emboss,
    FindEdges,
    HighPass,
    Maximum,
    Minimum,
    Offset,
}

impl FilterKind {
    /// Settings of a first use (Photoshop's). Clouds get their colors, and
    /// noise and clouds a seed, when applied.
    pub fn initial(self) -> Filter {
        let fade = |fade| ToneFade {
            fade,
            tonal_width: 50.0,
            radius: 1.0,
        };
        let morphology = Morphology {
            radius: 1.0,
            roundness: false,
        };
        match self {
            FilterKind::BoxBlur => Filter::BoxBlur { radius: 10.0 },
            FilterKind::GaussianBlur => Filter::GaussianBlur { radius: 5.0 },
            FilterKind::MotionBlur => Filter::MotionBlur {
                angle: 0.0,
                distance: 10.0,
            },
            FilterKind::SurfaceBlur => Filter::SurfaceBlur {
                radius: 5.0,
                threshold: 15.0,
            },
            FilterKind::AddNoise => Filter::AddNoise(AddNoise {
                amount: 12.5,
                gaussian: false,
                monochromatic: false,
                seed: 0,
            }),
            FilterKind::DustAndScratches => Filter::DustAndScratches {
                radius: 1.0,
                threshold: 0.0,
            },
            FilterKind::Median => Filter::Median { radius: 1.0 },
            FilterKind::ReduceNoise => Filter::ReduceNoise(ReduceNoise {
                strength: 6.0,
                preserve_details: 60.0,
                color_noise: 45.0,
                sharpen_details: 25.0,
            }),
            FilterKind::Mosaic => Filter::Mosaic { cell: 10.0 },
            FilterKind::Clouds => Filter::Clouds(Clouds {
                foreground: [0, 0, 0],
                background: [255, 255, 255],
                seed: 0,
            }),
            FilterKind::SmartSharpen => Filter::SmartSharpen(SmartSharpen {
                amount: 200.0,
                radius: 1.0,
                reduce_noise: 10.0,
                remove: Remove::Gaussian,
                angle: 0.0,
                shadows: fade(0.0),
                highlights: fade(0.0),
            }),
            FilterKind::UnsharpMask => Filter::UnsharpMask {
                amount: 50.0,
                radius: 1.0,
                threshold: 0.0,
            },
            FilterKind::Emboss => Filter::Emboss {
                angle: 135.0,
                height: 3.0,
                amount: 100.0,
            },
            FilterKind::FindEdges => Filter::FindEdges,
            FilterKind::HighPass => Filter::HighPass { radius: 10.0 },
            FilterKind::Maximum => Filter::Maximum(morphology),
            FilterKind::Minimum => Filter::Minimum(morphology),
            FilterKind::Offset => Filter::Offset(Offset {
                horizontal: 0.0,
                vertical: 0.0,
                undefined: Undefined::WrapAround,
            }),
        }
    }

    /// Has settings to choose (a dialog); the others apply at once.
    pub fn has_settings(self) -> bool {
        !matches!(self, FilterKind::FindEdges | FilterKind::Clouds)
    }
}

impl Filter {
    pub fn kind(&self) -> FilterKind {
        match self {
            Filter::GaussianBlur { .. } => FilterKind::GaussianBlur,
            Filter::BoxBlur { .. } => FilterKind::BoxBlur,
            Filter::MotionBlur { .. } => FilterKind::MotionBlur,
            Filter::SurfaceBlur { .. } => FilterKind::SurfaceBlur,
            Filter::UnsharpMask { .. } => FilterKind::UnsharpMask,
            Filter::SmartSharpen(_) => FilterKind::SmartSharpen,
            Filter::AddNoise(_) => FilterKind::AddNoise,
            Filter::ReduceNoise(_) => FilterKind::ReduceNoise,
            Filter::Median { .. } => FilterKind::Median,
            Filter::DustAndScratches { .. } => FilterKind::DustAndScratches,
            Filter::HighPass { .. } => FilterKind::HighPass,
            Filter::Offset(_) => FilterKind::Offset,
            Filter::Minimum(_) => FilterKind::Minimum,
            Filter::Maximum(_) => FilterKind::Maximum,
            Filter::Emboss { .. } => FilterKind::Emboss,
            Filter::FindEdges => FilterKind::FindEdges,
            Filter::Clouds(_) => FilterKind::Clouds,
            Filter::Mosaic { .. } => FilterKind::Mosaic,
        }
    }

    /// How far from a pixel the filter looks, in pixels; `None`: anywhere in
    /// the processed area (Offset).
    pub fn reach(&self) -> Option<u32> {
        let whole = |r: f32| r.max(0.0).ceil() as u32;
        let passes = |p: &[Pass]| p.iter().map(Pass::reach).sum::<usize>() as u32;
        Some(match *self {
            Filter::GaussianBlur { radius }
            | Filter::UnsharpMask { radius, .. }
            | Filter::HighPass { radius } => passes(&gaussian_passes(radius)),
            Filter::BoxBlur { radius } => whole(radius) + 1,
            Filter::MotionBlur { distance, .. } => whole(distance / 2.0) + 2,
            Filter::SurfaceBlur { radius, .. }
            | Filter::Median { radius }
            | Filter::DustAndScratches { radius, .. }
            | Filter::Minimum(Morphology { radius, .. })
            | Filter::Maximum(Morphology { radius, .. }) => whole(radius),
            Filter::SmartSharpen(s) => {
                let blur = match s.remove {
                    Remove::Gaussian => passes(&gaussian_passes(s.radius)),
                    Remove::Lens => 2 * (whole(s.radius) + 1),
                    Remove::Motion => whole(s.radius) + 2,
                };
                let fade = [s.shadows, s.highlights]
                    .iter()
                    .filter(|f| f.fade > 0.0)
                    .map(|f| passes(&gaussian_passes(f.radius)))
                    .max()
                    .unwrap_or(0);
                blur.max(fade)
            }
            Filter::AddNoise(_) | Filter::Clouds(_) => 0,
            Filter::ReduceNoise(_) => REDUCE_NOISE_HALO as u32,
            Filter::Offset(_) => return None,
            Filter::Emboss { height, .. } => whole(height / 2.0) + 1,
            Filter::FindEdges => 1,
            Filter::Mosaic { cell } => whole(cell),
        })
    }

    /// Applies the filter to `image`, whose top-left pixel is at `origin`
    /// in the canvas: noise, clouds and mosaic cells are anchored to the
    /// canvas, so a part of an image gets what the whole image would.
    pub fn apply(&self, image: &mut RgbaImage, origin: (i64, i64)) {
        if image.width() == 0 || image.height() == 0 {
            return;
        }
        match *self {
            Filter::GaussianBlur { radius } => blur(image, &gaussian_passes(radius)),
            Filter::BoxBlur { radius } => blur(image, &[moving_average(radius)]),
            Filter::MotionBlur { angle, distance } => motion_blur(image, angle, distance),
            Filter::SurfaceBlur { radius, threshold } => {
                let r = radius.round().max(1.0) as usize;
                by_tiles(image, r, |t| surface_blur_tile(t, r, threshold.max(1.0)));
            }
            Filter::UnsharpMask {
                amount,
                radius,
                threshold,
            } => {
                let blurred = blurred(image, &gaussian_passes(radius));
                let (k, t) = (amount / 100.0, threshold);
                combine(image, &blurred, |c, b| {
                    let d = c - b;
                    if d.abs() < t { c } else { c + k * d }
                });
            }
            Filter::SmartSharpen(s) => smart_sharpen(image, &s),
            Filter::AddNoise(n) => add_noise(image, &n, origin),
            Filter::ReduceNoise(n) => {
                by_tiles(image, REDUCE_NOISE_HALO, |t| reduce_noise_tile(t, &n));
            }
            Filter::Median { radius } => {
                let r = radius.round().max(1.0) as usize;
                by_tiles(image, r, |t| median_tile(t, r, None));
            }
            Filter::DustAndScratches { radius, threshold } => {
                let r = radius.round().max(1.0) as usize;
                let t = threshold.round().clamp(0.0, 255.0) as u8;
                by_tiles(image, r, |tile| median_tile(tile, r, Some(t)));
            }
            Filter::HighPass { radius } => {
                let blurred = blurred(image, &gaussian_passes(radius));
                combine(image, &blurred, |c, b| c - b + 128.0);
            }
            Filter::Offset(o) => offset(image, &o),
            Filter::Minimum(m) | Filter::Maximum(m) => {
                let max = matches!(self, Filter::Maximum(_));
                let r = m.radius.round().max(1.0) as usize;
                by_tiles(image, r, |t| extreme_tile(t, r, max, m.roundness));
            }
            Filter::Emboss {
                angle,
                height,
                amount,
            } => {
                let halo = (height / 2.0).ceil() as usize + 1;
                by_tiles(image, halo, |t| emboss_tile(t, angle, height, amount));
            }
            Filter::FindEdges => by_tiles(image, 1, find_edges_tile),
            Filter::Clouds(c) => clouds(image, &c, origin),
            Filter::Mosaic { cell } => mosaic(image, cell.round().max(1.0) as i64, origin),
        }
    }
}

// ------------------------------------------------------------ target

/// Pixels a filter (or an adjustment) works on, and the area it may change:
/// what [`crate::document::Document::active_target`] gives, which can be
/// processed on another thread.
#[derive(Clone)]
pub struct Target {
    pub image: Arc<RgbaImage>,
    /// Editable pixels (selection); `None`: all of them.
    pub clip: Option<Clip>,
    /// Position of the image in the canvas.
    pub offset: (i64, i64),
}

impl Target {
    /// Box of the editable pixels: x0, y0, x1, y1 (exclusive).
    pub fn area(&self) -> [u32; 4] {
        let (w, h) = self.image.dimensions();
        self.clip.as_ref().map_or([0, 0, w, h], |c| c.rect)
    }

    /// The image with `op` applied inside the area, blended by its coverage
    /// (a soft selection), like Photoshop. `op` gets the area plus `reach`
    /// pixels around it, and the canvas position of what it gets.
    pub fn process(&self, reach: u32, op: impl FnOnce(&mut RgbaImage, (i64, i64))) -> RgbaImage {
        let (w, h) = self.image.dimensions();
        self.process_region([0, 0, w, h], Some(reach), op)
    }

    /// `region` (x0, y0, x1, y1) of what [`Self::process`] gives: the
    /// preview of a part of the image. With `reach` `None`, `op` needs the
    /// whole area, however small the region.
    pub fn process_region(
        &self,
        region: [u32; 4],
        reach: Option<u32>,
        op: impl FnOnce(&mut RgbaImage, (i64, i64)),
    ) -> RgbaImage {
        let (w, h) = self.image.dimensions();
        let region = intersect(region, [0, 0, w, h]).unwrap_or([0, 0, 0, 0]);
        let [rx0, ry0, rx1, _] = region;
        let crop = |[x0, y0, x1, y1]: [u32; 4]| {
            image::imageops::crop_imm(&*self.image, x0, y0, x1 - x0, y1 - y0).to_image()
        };
        let Some(changed) = intersect(self.area(), region) else {
            return crop(region);
        };
        let window = match reach {
            Some(m) => {
                let [x0, y0, x1, y1] = changed;
                [
                    x0.saturating_sub(m),
                    y0.saturating_sub(m),
                    x1.saturating_add(m).min(w),
                    y1.saturating_add(m).min(h),
                ]
            }
            None => self.area(),
        };
        let at = |[x0, y0, ..]: [u32; 4]| (self.offset.0 + x0 as i64, self.offset.1 + y0 as i64);
        if self.clip.is_none() && window == [0, 0, w, h] && region == window {
            let mut out = (*self.image).clone();
            op(&mut out, self.offset);
            return out;
        }
        let mut part = crop(window);
        op(&mut part, at(window));
        let mut out = crop(region);
        let [cx0, cy0, cx1, cy1] = changed;
        let row = (rx1 - rx0) as usize * 4;
        let part_row = part.width() as usize * 4;
        let (part, clip) = (part.as_raw(), self.clip.as_ref());
        par_rows(out.as_mut(), row, |first, band| {
            for (i, line) in band.chunks_exact_mut(row).enumerate() {
                let y = ry0 + (first + i) as u32;
                if y < cy0 || y >= cy1 {
                    continue;
                }
                let src = &part[(y - window[1]) as usize * part_row..];
                for x in cx0..cx1 {
                    let coverage = clip.map_or(255, |c| c.coverage(x, y));
                    let d = (x - rx0) as usize * 4;
                    let s = (x - window[0]) as usize * 4;
                    let from = [line[d], line[d + 1], line[d + 2], line[d + 3]];
                    let to = [src[s], src[s + 1], src[s + 2], src[s + 3]];
                    line[d..d + 4].copy_from_slice(&mix(from, to, coverage));
                }
            }
        });
        out
    }
}

/// Common part of two boxes (x0, y0, x1, y1), if not empty.
fn intersect(a: [u32; 4], b: [u32; 4]) -> Option<[u32; 4]> {
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

// ------------------------------------------------------------ cancel

thread_local! {
    /// Cancel flag of the filter being applied on this thread, if any.
    static CANCEL: std::cell::RefCell<Option<Arc<AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `f`, the filters it applies stopping early (their result is then
/// meaningless) once `flag` is set: a filter being applied, cancelled.
pub fn cancellable<T>(flag: Option<Arc<AtomicBool>>, f: impl FnOnce() -> T) -> T {
    let before = CANCEL.with(|c| c.replace(flag));
    let out = f();
    CANCEL.with(|c| *c.borrow_mut() = before);
    out
}

/// The computation running on this thread was cancelled (see
/// [`cancellable`]).
pub fn cancel_requested() -> bool {
    cancelled(&cancel_flag())
}

fn cancel_flag() -> Option<Arc<AtomicBool>> {
    CANCEL.with(|c| c.borrow().clone())
}

fn cancelled(flag: &Option<Arc<AtomicBool>>) -> bool {
    flag.as_ref().is_some_and(|f| f.load(Ordering::Relaxed))
}

// ------------------------------------------------------------ parallel loops

/// Runs `f(first_row, band)` on bands of whole rows (`row` elements each)
/// of `data`, in parallel for large data.
pub fn par_rows<T: Send>(data: &mut [T], row: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = data.len() / row.max(1);
    let n = threads().min(rows);
    if n <= 1 || data.len() < 1 << 14 {
        return f(0, data);
    }
    let per = rows.div_ceil(n);
    let f = &f;
    let flag = cancel_flag();
    std::thread::scope(|s| {
        for (i, band) in data.chunks_mut(per * row).enumerate() {
            let flag = flag.clone();
            s.spawn(move || cancellable(flag, || f(i * per, band)));
        }
    });
}

/// Runs `f` on blocks of columns of `data` (rows of `row` elements; blocks
/// of `block` elements of each row), in parallel: `f` gets the segments of
/// a block, from the top row down.
fn par_columns<T: Send>(
    data: &mut [T],
    row: usize,
    block: usize,
    f: impl Fn(&mut [&mut [T]]) + Sync,
) {
    if row == 0 || data.is_empty() {
        return;
    }
    let rows = data.len() / row;
    let mut blocks: Vec<Vec<&mut [T]>> = (0..row.div_ceil(block))
        .map(|_| Vec::with_capacity(rows))
        .collect();
    for line in data.chunks_exact_mut(row) {
        for (b, segment) in line.chunks_mut(block).enumerate() {
            blocks[b].push(segment);
        }
    }
    let per = blocks.len().div_ceil(threads().max(1));
    let f = &f;
    let flag = cancel_flag();
    std::thread::scope(|s| {
        for group in blocks.chunks_mut(per) {
            let flag = flag.clone();
            s.spawn(move || cancellable(flag, || group.iter_mut().for_each(|b| f(b))));
        }
    });
}

// ------------------------------------------------------------ blurs

/// Pixels per block of columns blurred together (vertical passes).
const COLUMN_BLOCK: usize = 16;

/// One pass of a separable blur along a line.
#[derive(Clone, Debug, PartialEq)]
enum Pass {
    /// Average of 2r + 1 samples and of `w` (0..1) of the next one on each
    /// side: an exact variance whatever the size.
    Box { r: usize, w: f32 },
    /// Symmetric weights, from the center out.
    Kernel(Vec<f32>),
}

impl Pass {
    /// Samples read on each side.
    fn reach(&self) -> usize {
        match self {
            Pass::Box { r, .. } => r + 1,
            Pass::Kernel(k) => k.len() - 1,
        }
    }
}

/// Passes of a Gaussian blur of standard deviation `radius` (Photoshop's
/// radius): the exact kernel when small, else three moving averages of the
/// same variance — like Photoshop, fast whatever the radius.
fn gaussian_passes(radius: f32) -> Vec<Pass> {
    if radius.is_nan() || radius < 0.05 {
        return Vec::new();
    }
    if radius < 2.0 {
        let k = (3.0 * radius).ceil().max(1.0) as usize;
        let mut w: Vec<f32> = (0..=k)
            .map(|i| (-((i * i) as f32) / (2.0 * radius * radius)).exp())
            .collect();
        let total = w[0] + 2.0 * w[1..].iter().sum::<f32>();
        w.iter_mut().for_each(|v| *v /= total);
        return vec![Pass::Kernel(w)];
    }
    let variance = radius * radius / 3.0;
    let r = (((1.0 + 12.0 * variance).sqrt() - 1.0) / 2.0).floor();
    // Variance of 2r + 1 samples, plus w at r + 1 on each side.
    let w =
        (2.0 * r + 1.0) * (variance - r * (r + 1.0) / 3.0) / (2.0 * ((r + 1.0).powi(2) - variance));
    let pass = Pass::Box {
        r: r as usize,
        w: w.clamp(0.0, 1.0),
    };
    vec![pass; 3]
}

/// Average over `radius` pixels on each side (the fraction weighing the
/// last one): Box Blur.
fn moving_average(radius: f32) -> Pass {
    let radius = radius.max(0.0);
    Pass::Box {
        r: radius.floor() as usize,
        w: radius.fract(),
    }
}

/// Premultiplied pixels with 8 bits of precision more: c × a / 255 × 256
/// (alpha × 256).
fn premultiplied(image: &RgbaImage) -> Vec<u16> {
    let row = image.width() as usize * 4;
    let src = image.as_raw();
    let mut buf = vec![0u16; src.len()];
    par_rows(&mut buf, row, |first, band| {
        let src = &src[first * row..first * row + band.len()];
        for (d, p) in band.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let a = p[3] as u32;
            for c in 0..3 {
                d[c] = ((p[c] as u32 * a * 256 + 127) / 255) as u16;
            }
            d[3] = (a * 256) as u16;
        }
    });
    buf
}

/// Back from [`premultiplied`].
fn unpremultiply(buf: &[u16], image: &mut RgbaImage) {
    let row = image.width() as usize * 4;
    par_rows(image.as_mut(), row, |first, band| {
        let src = &buf[first * row..first * row + band.len()];
        for (d, p) in band.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let a = p[3] as u32;
            if a < 128 {
                d.fill(0);
                continue;
            }
            for c in 0..3 {
                d[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
            d[3] = ((a + 128) >> 8).min(255) as u8;
        }
    });
}

/// Blurs `image` with `passes` along the rows, then along the columns.
fn blur(image: &mut RgbaImage, passes: &[Pass]) {
    if passes.is_empty() {
        return;
    }
    let mut buf = premultiplied(image);
    blur_buffer(&mut buf, image.width() as usize, passes);
    unpremultiply(&buf, image);
}

fn blurred(image: &RgbaImage, passes: &[Pass]) -> RgbaImage {
    let mut out = image.clone();
    blur(&mut out, passes);
    out
}

/// Blurs premultiplied pixels, `w` per row.
fn blur_buffer(buf: &mut [u16], w: usize, passes: &[Pass]) {
    let row = w * 4;
    par_rows(buf, row, |_, band| {
        let mut scratch = Scratch::default();
        for line in band.chunks_exact_mut(row) {
            filter_line(line, 4, passes, &mut scratch);
        }
    });
    // Columns: a block of them at a time, their pixels side by side.
    par_columns(buf, row, COLUMN_BLOCK * 4, |segments| {
        let lanes = segments[0].len();
        let mut column: Vec<u16> = Vec::with_capacity(segments.len() * lanes);
        for s in segments.iter() {
            column.extend_from_slice(s);
        }
        filter_line(&mut column, lanes, passes, &mut Scratch::default());
        for (s, c) in segments.iter_mut().zip(column.chunks_exact(lanes)) {
            s.copy_from_slice(c);
        }
    });
}

/// Buffers reused from line to line.
#[derive(Default)]
struct Scratch {
    padded: Vec<u16>,
    sums: Vec<u32>,
    acc: Vec<f32>,
}

/// Applies `passes` to a line of elements of `lanes` values each (pixels,
/// or pixels of several columns), the edge elements repeating beyond it.
fn filter_line(line: &mut [u16], lanes: usize, passes: &[Pass], s: &mut Scratch) {
    let n = line.len() / lanes;
    if n == 0 {
        return;
    }
    for pass in passes {
        let reach = pass.reach();
        let pad = &mut s.padded;
        pad.clear();
        for _ in 0..reach {
            pad.extend_from_slice(&line[..lanes]);
        }
        pad.extend_from_slice(line);
        for _ in 0..reach {
            pad.extend_from_slice(&line[(n - 1) * lanes..]);
        }
        match pass {
            Pass::Box { r, w } => {
                let (r, w) = (*r, *w);
                let norm = 1.0 / ((2 * r + 1) as f32 + 2.0 * w);
                s.sums.clear();
                s.sums.resize(lanes, 0);
                // Window around the first element (at `reach` in `pad`).
                for k in reach - r..=reach + r {
                    for (sum, v) in s.sums.iter_mut().zip(&pad[k * lanes..(k + 1) * lanes]) {
                        *sum += *v as u32;
                    }
                }
                for (i, out) in line.chunks_exact_mut(lanes).enumerate() {
                    let p = i + reach;
                    let before = &pad[(p - r - 1) * lanes..(p - r) * lanes];
                    let first = &pad[(p - r) * lanes..(p - r + 1) * lanes];
                    let after = &pad[(p + r + 1) * lanes..(p + r + 2) * lanes];
                    for l in 0..lanes {
                        let edges = before[l] as f32 + after[l] as f32;
                        out[l] = ((s.sums[l] as f32 + w * edges) * norm + 0.5) as u16;
                        s.sums[l] = s.sums[l] + after[l] as u32 - first[l] as u32;
                    }
                }
            }
            Pass::Kernel(weights) => {
                // Tap by tap over the whole line: contiguous, vectorized.
                let len = line.len();
                let center = reach * lanes;
                s.acc.clear();
                s.acc.extend(
                    pad[center..center + len]
                        .iter()
                        .map(|&v| weights[0] * v as f32),
                );
                for (d, &wd) in weights.iter().enumerate().skip(1) {
                    let left = &pad[center - d * lanes..center - d * lanes + len];
                    let right = &pad[center + d * lanes..center + d * lanes + len];
                    for ((a, l), r) in s.acc.iter_mut().zip(left).zip(right) {
                        *a += wd * (*l as f32 + *r as f32);
                    }
                }
                for (o, a) in line.iter_mut().zip(&s.acc) {
                    *o = (a + 0.5) as u16;
                }
            }
        }
    }
}

/// Motion Blur: average along a line of `distance` pixels through each
/// pixel, at `angle`. Lines are followed sample by sample along the axis
/// closest to them (a running sum: fast whatever the distance), sampled
/// between two pixels across it; each pixel blends the two lines on either
/// side of its center.
fn motion_blur(image: &mut RgbaImage, angle: f32, distance: f32) {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let theta = angle.to_radians();
    let (dx, dy) = (theta.cos(), -theta.sin());
    // Major axis: one sample per pixel; across it, the line moves by `t`.
    let x_major = dx.abs() >= dy.abs();
    let (along, t) = if x_major {
        (dx.abs(), dy / dx)
    } else {
        (dy.abs(), dx / dy)
    };
    let span = distance * along;
    if span.is_nan() || span <= 1.0 {
        return;
    }
    let half = (span - 1.0) / 2.0;
    let (r, frac) = (half.floor() as usize, half.fract());
    let norm = 1.0 / span;
    let (major_len, minor_len) = if x_major { (w, h) } else { (h, w) };
    let src = premultiplied(image);
    let sample = |m: i64, n: i64| -> &[u16] {
        let m = m.clamp(0, major_len as i64 - 1) as usize;
        let n = n.clamp(0, minor_len as i64 - 1) as usize;
        let i = if x_major { n * w + m } else { m * w + n };
        &src[i * 4..i * 4 + 4]
    };
    par_rows(image.as_mut(), w * 4, |y0, band| {
        let y1 = y0 + band.len() / (w * 4);
        // The band's pixels in (major, minor) coordinates.
        let ((m0, m1), (n0, n1)) = if x_major {
            ((0, w as i64), (y0 as i64, y1 as i64))
        } else {
            ((y0 as i64, y1 as i64), (0, w as i64))
        };
        let shifts = [m0 as f32 * t, (m1 - 1) as f32 * t];
        let (lo, hi) = (shifts[0].min(shifts[1]), shifts[0].max(shifts[1]));
        let first = (n0 as f32 - 1.0 - hi).floor() as i64 - 1;
        let last = (n1 as f32 + 1.0 - lo).ceil() as i64 + 1;
        let mut samples: Vec<[f32; 4]> = Vec::new();
        let mut prev: Option<(i64, Vec<[f32; 4]>)> = None;
        for j in first..=last {
            // Where line j (minor = j + major × t) is near the band.
            let (a, b) = ((n0 - 1 - j) as f32, (n1 + 1 - j) as f32);
            let (ma, mb) = if t == 0.0 {
                if a <= 0.0 && b > 0.0 {
                    (m0, m1)
                } else {
                    (0, 0)
                }
            } else {
                let (u, v) = if t > 0.0 {
                    (a / t, b / t)
                } else {
                    (b / t, a / t)
                };
                (
                    (u.floor() as i64).saturating_sub(1).max(m0),
                    (v.ceil() as i64).saturating_add(1).min(m1),
                )
            };
            if ma >= mb {
                prev = None;
                continue;
            }
            samples.clear();
            for m in ma - r as i64 - 1..mb + r as i64 + 1 {
                let n = j as f32 + m as f32 * t;
                let base = n.floor();
                let f = n - base;
                let (p, q) = (sample(m, base as i64), sample(m, base as i64 + 1));
                samples.push(std::array::from_fn(|l| {
                    p[l] as f32 * (1.0 - f) + q[l] as f32 * f
                }));
            }
            let mut line = Vec::with_capacity((mb - ma) as usize);
            let mut sum = [0.0f32; 4];
            for s in &samples[1..2 * r + 2] {
                (0..4).for_each(|l| sum[l] += s[l]);
            }
            for i in 0..(mb - ma) as usize {
                let (before, after) = (samples[i], samples[i + 2 * r + 2]);
                line.push(std::array::from_fn(|l| {
                    (sum[l] + frac * (before[l] + after[l])) * norm
                }));
                let leaving = samples[i + 1];
                (0..4).for_each(|l| sum[l] += after[l] - leaving[l]);
            }
            if let Some((pa, pline)) = &prev {
                for m in ma.max(*pa)..mb.min(*pa + pline.len() as i64) {
                    let a = (j - 1) as f32 + m as f32 * t;
                    let n = a.ceil() as i64;
                    if n < n0 || n >= n1 {
                        continue;
                    }
                    let e = n as f32 - a;
                    let (p, q) = (pline[(m - pa) as usize], line[(m - ma) as usize]);
                    let (x, y) = if x_major { (m, n) } else { (n, m) };
                    let at = ((y as usize - y0) * w + x as usize) * 4;
                    let px = std::array::from_fn(|l| p[l] * (1.0 - e) + q[l] * e);
                    band[at..at + 4].copy_from_slice(&unpremultiply_px(px));
                }
            }
            prev = Some((ma, line));
        }
    });
}

/// A pixel from premultiplied values (see [`premultiplied`]).
fn unpremultiply_px(p: [f32; 4]) -> [u8; 4] {
    if p[3] < 128.0 {
        return [0; 4];
    }
    let c = |v: f32| (v * 255.0 / p[3]).round().clamp(0.0, 255.0) as u8;
    [
        c(p[0]),
        c(p[1]),
        c(p[2]),
        (p[3] / 256.0).round().min(255.0) as u8,
    ]
}

/// Replaces each color channel `c` of `image` with `f(c, b)`, `b` that of
/// `other` (alpha is kept).
fn combine(image: &mut RgbaImage, other: &RgbaImage, f: impl Fn(f32, f32) -> f32 + Sync) {
    let row = image.width() as usize * 4;
    let other = other.as_raw();
    par_rows(image.as_mut(), row, |first, band| {
        let o = &other[first * row..first * row + band.len()];
        for (p, q) in band.chunks_exact_mut(4).zip(o.chunks_exact(4)) {
            for c in 0..3 {
                p[c] = f(p[c] as f32, q[c] as f32).round().clamp(0.0, 255.0) as u8;
            }
        }
    });
}

// ------------------------------------------------------------ sharpening

/// Smart Sharpen: unsharp masking against the blur to remove, with small
/// differences (noise) sharpened less, and less in the shadows and
/// highlights if asked.
fn smart_sharpen(image: &mut RgbaImage, s: &SmartSharpen) {
    let mut blur = image.clone();
    match s.remove {
        Remove::Gaussian => self::blur(&mut blur, &gaussian_passes(s.radius)),
        // A defocus: two averages of the radius (closer to a disc).
        Remove::Lens => self::blur(
            &mut blur,
            &[moving_average(s.radius), moving_average(s.radius)],
        ),
        Remove::Motion => motion_blur(&mut blur, s.angle, 2.0 * s.radius),
    }
    // Tones around each pixel, for the fading.
    let tones = |fade: &ToneFade| {
        (fade.fade > 0.0).then(|| {
            let mut l = image.clone();
            for p in l.pixels_mut() {
                let y = luminance(p.0);
                p.0 = [y, y, y, 255];
            }
            self::blur(&mut l, &gaussian_passes(fade.radius));
            l
        })
    };
    let (shadows, highlights) = (tones(&s.shadows), tones(&s.highlights));
    let amount = s.amount / 100.0;
    let noise = (s.reduce_noise / 100.0 * 10.0).powi(2);
    let row = image.width() as usize * 4;
    let blur = blur.as_raw();
    let weight = |tones: &Option<RgbaImage>, fade: &ToneFade, i: usize, dark: bool| {
        let Some(t) = tones else { return 1.0 };
        let width = fade.tonal_width / 100.0 * 128.0;
        let y = t.as_raw()[i] as f32;
        let distance = if dark { y } else { 255.0 - y };
        let inside = if width > 0.0 {
            (1.0 - distance / width).max(0.0)
        } else {
            0.0
        };
        1.0 - fade.fade / 100.0 * inside
    };
    par_rows(image.as_mut(), row, |first, band| {
        for (k, p) in band.chunks_exact_mut(4).enumerate() {
            let i = first * row + k * 4;
            let fade = weight(&shadows, &s.shadows, i, true)
                * weight(&highlights, &s.highlights, i, false);
            for c in 0..3 {
                let v = p[c] as f32;
                let d = v - blur[i + c] as f32;
                let d = if noise > 0.0 {
                    d * d * d / (d * d + noise)
                } else {
                    d
                };
                p[c] = (v + amount * fade * d).round().clamp(0.0, 255.0) as u8;
            }
        }
    });
}

fn luminance(p: [u8; 4]) -> u8 {
    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).round() as u8
}

// ------------------------------------------------------------ tiles

/// Side of the tiles of the neighborhood filters.
const TILE: usize = 256;

/// Pixels of a tile of the image and of `halo` pixels around it (the edge
/// pixels repeating beyond the image), row by row.
struct Tile {
    px: Vec<[u8; 4]>,
    w: usize,
    h: usize,
    halo: usize,
}

impl Tile {
    fn gather(
        src: &RgbaImage,
        (x0, y0): (usize, usize),
        (w, h): (usize, usize),
        halo: usize,
    ) -> Self {
        let (iw, ih) = (src.width() as i64, src.height() as i64);
        let (tw, th) = (w + 2 * halo, h + 2 * halo);
        let raw = src.as_raw();
        let mut px = Vec::with_capacity(tw * th);
        for ty in 0..th {
            let y = (y0 as i64 + ty as i64 - halo as i64).clamp(0, ih - 1) as usize;
            let line = &raw[y * iw as usize * 4..];
            for tx in 0..tw {
                let x = (x0 as i64 + tx as i64 - halo as i64).clamp(0, iw - 1) as usize;
                px.push([
                    line[x * 4],
                    line[x * 4 + 1],
                    line[x * 4 + 2],
                    line[x * 4 + 3],
                ]);
            }
        }
        Self {
            px,
            w: tw,
            h: th,
            halo,
        }
    }

    /// Size of the tile itself (without its halo).
    fn inner(&self) -> (usize, usize) {
        (self.w - 2 * self.halo, self.h - 2 * self.halo)
    }

    /// A value per pixel.
    fn plane(&self, f: impl Fn([u8; 4]) -> f32) -> Vec<f32> {
        self.px.iter().map(|&p| f(p)).collect()
    }

    /// The tile with the colors of (partly) transparent pixels mixed, by
    /// their transparency, with those of the visible pixels around them: the
    /// filters that read straight colors then don't depend on the hidden
    /// colors of fully transparent pixels (black, or anything left by
    /// painting or a file).
    fn visible_colors(&self) -> Tile {
        let mut px = self.px.clone();
        if self.px.iter().all(|p| p[3] == 255) {
            return Tile { px, ..*self };
        }
        let alpha = self.plane(|p| p[3] as f32);
        let mut weights = alpha.clone();
        gauss_plane(&mut weights, self.w, self.h, 3.0);
        for c in 0..3 {
            let mut sums = self.plane(|p| p[c] as f32 * p[3] as f32);
            gauss_plane(&mut sums, self.w, self.h, 3.0);
            for (i, p) in px.iter_mut().enumerate() {
                let a = alpha[i];
                if a < 255.0 {
                    let around = if weights[i] > 1e-3 {
                        sums[i] / weights[i]
                    } else {
                        0.0
                    };
                    let v = (a * p[c] as f32 + (255.0 - a) * around) / 255.0;
                    p[c] = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        Tile { px, ..*self }
    }
}

/// Runs `f` on tiles of `image` (read from a copy of it) and writes the
/// pixels it returns for each tile: its inner part, row by row.
fn by_tiles(image: &mut RgbaImage, halo: usize, f: impl Fn(&Tile) -> Vec<[u8; 4]> + Sync) {
    let src = image.clone();
    let w = image.width() as usize;
    par_rows(image.as_mut(), w * 4, |y0, band| {
        let rows = band.len() / (w * 4);
        let cancel = cancel_flag();
        for x0 in (0..w).step_by(TILE) {
            if cancelled(&cancel) {
                return;
            }
            let tw = TILE.min(w - x0);
            let out = f(&Tile::gather(&src, (x0, y0), (tw, rows), halo));
            for (y, line) in out.chunks_exact(tw).enumerate() {
                let at = (y * w + x0) * 4;
                for (d, p) in band[at..at + tw * 4].chunks_exact_mut(4).zip(line) {
                    d.copy_from_slice(p);
                }
            }
        }
    });
}

/// Average over (2r + 1) × (2r + 1) values of a plane, edges repeated.
fn box_plane(v: &mut [f32], w: usize, h: usize, r: usize) {
    let mut line = Vec::new();
    let mut out = Vec::new();
    for y in 0..h {
        line.clear();
        line.extend_from_slice(&v[y * w..(y + 1) * w]);
        average_1d(&line, r, &mut out);
        v[y * w..(y + 1) * w].copy_from_slice(&out);
    }
    for x in 0..w {
        line.clear();
        line.extend((0..h).map(|y| v[y * w + x]));
        average_1d(&line, r, &mut out);
        for (y, o) in out.iter().enumerate() {
            v[y * w + x] = *o;
        }
    }
}

fn average_1d(src: &[f32], r: usize, out: &mut Vec<f32>) {
    let n = src.len() as i64;
    let at = |i: i64| src[i.clamp(0, n - 1) as usize];
    let norm = 1.0 / (2 * r + 1) as f32;
    let r = r as i64;
    let mut sum: f32 = (-r..=r).map(at).sum();
    out.clear();
    for i in 0..n {
        out.push(sum * norm);
        sum += at(i + r + 1) - at(i - r);
    }
}

/// Gaussian blur of a plane (exact kernel), edges repeated.
fn gauss_plane(v: &mut [f32], w: usize, h: usize, sigma: f32) {
    let k = (3.0 * sigma).ceil().max(1.0) as i64;
    let mut weights: Vec<f32> = (0..=k)
        .map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total = weights[0] + 2.0 * weights[1..].iter().sum::<f32>();
    weights.iter_mut().for_each(|x| *x /= total);
    let conv = |line: &[f32], out: &mut Vec<f32>| {
        let n = line.len() as i64;
        let at = |i: i64| line[i.clamp(0, n - 1) as usize];
        out.clear();
        out.extend((0..n).map(|i| {
            weights[0] * at(i)
                + (1..=k)
                    .map(|d| weights[d as usize] * (at(i - d) + at(i + d)))
                    .sum::<f32>()
        }));
    };
    let (mut line, mut out) = (Vec::new(), Vec::new());
    for y in 0..h {
        line.clear();
        line.extend_from_slice(&v[y * w..(y + 1) * w]);
        conv(&line, &mut out);
        v[y * w..(y + 1) * w].copy_from_slice(&out);
    }
    for x in 0..w {
        line.clear();
        line.extend((0..h).map(|y| v[y * w + x]));
        conv(&line, &mut out);
        for (y, o) in out.iter().enumerate() {
            v[y * w + x] = *o;
        }
    }
}

fn premultiply8(p: [u8; 4]) -> [u8; 4] {
    let a = p[3] as u32;
    let c = |v: u8| ((v as u32 * a + 127) / 255) as u8;
    [c(p[0]), c(p[1]), c(p[2]), p[3]]
}

fn unpremultiply8(p: [u8; 4]) -> [u8; 4] {
    let a = p[3] as u32;
    if a == 0 {
        return [0; 4];
    }
    let c = |v: u8| ((v as u32 * 255 + a / 2) / a).min(255) as u8;
    [c(p[0]), c(p[1]), c(p[2]), p[3]]
}

/// Surface Blur of a tile: each channel averaged over the square of radius
/// `r`, neighbors weighing less the more they differ (nothing beyond 2.5 ×
/// `threshold` levels). Colors are weighted by alpha. Small squares are
/// computed directly, large ones from averages at a few levels of the
/// value, interpolated (constant time whatever the radius).
fn surface_blur_tile(t: &Tile, r: usize, threshold: f32) -> Vec<[u8; 4]> {
    let reach = 2.5 * threshold;
    let (iw, ih) = t.inner();
    let alpha = t.plane(|p| p[3] as f32 / 255.0);
    let ones = vec![1.0; t.px.len()];
    let step = (reach / 3.0).max(255.0 / 48.0);
    let levels = (255.0 / step).ceil() as usize + 1;
    let direct = (2 * r + 1).pow(2) <= 12 * levels;
    let mut out = vec![[0u8; 4]; iw * ih];
    for c in 0..4 {
        let v = t.plane(|p| p[c] as f32);
        let weights = if c < 3 { &alpha } else { &ones };
        let tent = |d: f32| (1.0 - d.abs() / reach).max(0.0);
        let mut result = vec![0.0f32; iw * ih];
        if direct {
            for y in 0..ih {
                for x in 0..iw {
                    let center = v[(y + r) * t.w + x + r];
                    let (mut num, mut den) = (0.0, 0.0);
                    for wy in y..=y + 2 * r {
                        for wx in x..=x + 2 * r {
                            let i = wy * t.w + wx;
                            let k = tent(v[i] - center) * weights[i];
                            num += k * v[i];
                            den += k;
                        }
                    }
                    result[y * iw + x] = if den > 0.0 { num / den } else { center };
                }
            }
        } else {
            let inner = |i: usize| v[(i / iw + r) * t.w + i % iw + r];
            let (lo, hi) = (0..iw * ih).fold((255.0f32, 0.0f32), |(lo, hi), i| {
                (lo.min(inner(i)), hi.max(inner(i)))
            });
            let cancel = cancel_flag();
            for k in 0..levels {
                if cancelled(&cancel) {
                    break;
                }
                let level = k as f32 * step;
                if level < lo - step || level > hi + step {
                    continue;
                }
                let mut den: Vec<f32> = (0..v.len())
                    .map(|i| tent(v[i] - level) * weights[i])
                    .collect();
                let mut num: Vec<f32> = den.iter().zip(&v).map(|(d, x)| d * x).collect();
                box_plane(&mut den, t.w, t.h, r);
                box_plane(&mut num, t.w, t.h, r);
                for (i, out) in result.iter_mut().enumerate() {
                    let value = inner(i);
                    let hat = 1.0 - (value - level).abs() / step;
                    if hat > 0.0 {
                        let j = (i / iw + r) * t.w + i % iw + r;
                        let d = den[j];
                        *out += hat * if d > 1e-6 { num[j] / d } else { value };
                    }
                }
            }
        }
        for (o, v) in out.iter_mut().zip(&result) {
            o[c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Median of each channel (premultiplied) over the square of radius `r`
/// (the tile's halo); with a threshold (Dust & Scratches), only where the
/// pixel differs from it by more than that.
fn median_tile(t: &Tile, r: usize, threshold: Option<u8>) -> Vec<[u8; 4]> {
    let px: Vec<[u8; 4]> = t.px.iter().map(|&p| premultiply8(p)).collect();
    let (iw, ih) = t.inner();
    let k = 2 * r + 1;
    let rank = (k * k / 2) as u32;
    let mut out = vec![[0u8; 4]; iw * ih];
    let mut hist = [0u32; 256];
    let cancel = cancel_flag();
    for c in 0..4 {
        // The window snakes through the tile (right along even rows, left
        // along odd ones): each step swaps one row or column of it.
        hist.fill(0);
        for wy in 0..k {
            for p in &px[wy * t.w..wy * t.w + k] {
                hist[p[c] as usize] += 1;
            }
        }
        // Median: the value with `rank` values below it; `below` counts them.
        let (mut median, mut below) = (0usize, 0u32);
        let swap = |hist: &mut [u32; 256], gone: u8, new: u8, below: &mut u32, median: usize| {
            hist[gone as usize] -= 1;
            hist[new as usize] += 1;
            *below =
                *below - u32::from((gone as usize) < median) + u32::from((new as usize) < median);
        };
        let mut x = 0;
        for y in 0..ih {
            if cancelled(&cancel) {
                return out;
            }
            if y > 0 {
                for wx in x..x + k {
                    let (gone, new) = (px[(y - 1) * t.w + wx][c], px[(y + k - 1) * t.w + wx][c]);
                    swap(&mut hist, gone, new, &mut below, median);
                }
            }
            for step in 0..iw {
                if step > 0 {
                    let (gone_x, new_x) = if y % 2 == 0 {
                        x += 1;
                        (x - 1, x + k - 1)
                    } else {
                        x -= 1;
                        (x + k, x)
                    };
                    for wy in y..y + k {
                        let (gone, new) = (px[wy * t.w + gone_x][c], px[wy * t.w + new_x][c]);
                        swap(&mut hist, gone, new, &mut below, median);
                    }
                }
                while below > rank {
                    median -= 1;
                    below -= hist[median];
                }
                while below + hist[median] <= rank {
                    below += hist[median];
                    median += 1;
                }
                let own = px[(y + r) * t.w + x + r][c];
                let m = median as u8;
                out[y * iw + x][c] = match threshold {
                    Some(t) if own.abs_diff(m) <= t => own,
                    _ => m,
                };
            }
        }
    }
    out.iter_mut().for_each(|p| *p = unpremultiply8(*p));
    out
}

/// Minimum or Maximum of each channel (premultiplied: transparency spreads
/// or shrinks with the colors) over the square or disc of radius `r`.
fn extreme_tile(t: &Tile, r: usize, max: bool, round: bool) -> Vec<[u8; 4]> {
    let px: Vec<[u8; 4]> = t.px.iter().map(|&p| premultiply8(p)).collect();
    let (iw, ih) = t.inner();
    let pick = |a: [u8; 4], b: [u8; 4]| -> [u8; 4] {
        std::array::from_fn(|c| if max { a[c].max(b[c]) } else { a[c].min(b[c]) })
    };
    let mut out = vec![if max { [0; 4] } else { [255; 4] }; iw * ih];
    if round {
        // Rows of the disc: each one over its own half-width.
        let cancel = cancel_flag();
        for dy in -(r as i64)..=r as i64 {
            if cancelled(&cancel) {
                break;
            }
            let half = ((r * r) as f64 - (dy * dy) as f64).sqrt().floor() as usize;
            for y in 0..ih {
                let ty = (y as i64 + r as i64 + dy) as usize;
                let line = &px[ty * t.w + r - half..ty * t.w + r + half + iw];
                for (o, e) in out[y * iw..(y + 1) * iw]
                    .iter_mut()
                    .zip(running_extreme(line, half, max))
                {
                    *o = pick(*o, e);
                }
            }
        }
    } else {
        // Square: rows, then columns.
        let mut rows = Vec::with_capacity(iw * t.h);
        for y in 0..t.h {
            rows.extend(running_extreme(&px[y * t.w..(y + 1) * t.w], r, max));
        }
        let mut column = Vec::with_capacity(t.h);
        for x in 0..iw {
            column.clear();
            column.extend((0..t.h).map(|y| rows[y * iw + x]));
            for (y, e) in running_extreme(&column, r, max).into_iter().enumerate() {
                out[y * iw + x] = e;
            }
        }
    }
    out.iter_mut().for_each(|p| *p = unpremultiply8(*p));
    out
}

/// Extreme of each window of 2r + 1 values of `line` (van Herk / Gil-Werman:
/// three comparisons per value whatever `r`).
fn running_extreme(line: &[[u8; 4]], r: usize, max: bool) -> Vec<[u8; 4]> {
    let k = 2 * r + 1;
    let n = line.len();
    let pick = |a: [u8; 4], b: [u8; 4]| -> [u8; 4] {
        std::array::from_fn(|c| if max { a[c].max(b[c]) } else { a[c].min(b[c]) })
    };
    // From the start of each block of k, and to its end.
    let mut from_start = line.to_vec();
    for i in 1..n {
        if i % k != 0 {
            from_start[i] = pick(from_start[i - 1], line[i]);
        }
    }
    let mut to_end = line.to_vec();
    for i in (0..n.saturating_sub(1)).rev() {
        if (i + 1) % k != 0 {
            to_end[i] = pick(to_end[i + 1], line[i]);
        }
    }
    (0..=n - k)
        .map(|i| pick(to_end[i], from_start[i + k - 1]))
        .collect()
}

/// Emboss: 128 gray plus the difference between the two sides of each
/// pixel, `height` apart in the direction of the light.
fn emboss_tile(t: &Tile, angle: f32, height: f32, amount: f32) -> Vec<[u8; 4]> {
    let t = &t.visible_colors();
    let (iw, ih) = t.inner();
    let theta = angle.to_radians();
    let (ux, uy) = (theta.cos() * height / 2.0, -theta.sin() * height / 2.0);
    let k = amount / 100.0;
    let sample = |x: f32, y: f32, c: usize| {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |x: f32, y: f32| {
            let x = (x as usize).min(t.w - 1);
            let y = (y as usize).min(t.h - 1);
            t.px[y * t.w + x][c] as f32
        };
        let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1.0, y0) * fx;
        let bottom = at(x0, y0 + 1.0) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0) * fx;
        top * (1.0 - fy) + bottom * fy
    };
    let mut out = Vec::with_capacity(iw * ih);
    for y in 0..ih {
        for x in 0..iw {
            let (cx, cy) = ((x + t.halo) as f32, (y + t.halo) as f32);
            let own = t.px[(y + t.halo) * t.w + x + t.halo];
            out.push(std::array::from_fn(|c| {
                if c == 3 {
                    return own[3];
                }
                let lit = sample(cx - ux, cy - uy, c) - sample(cx + ux, cy + uy, c);
                (128.0 + k * lit).round().clamp(0.0, 255.0) as u8
            }));
        }
    }
    out
}

/// Find Edges: white, with the edges of each channel drawn darker the
/// sharper they are.
fn find_edges_tile(t: &Tile) -> Vec<[u8; 4]> {
    let t = &t.visible_colors();
    let (iw, ih) = t.inner();
    let mut out = Vec::with_capacity(iw * ih);
    for y in 1..=ih {
        for x in 1..=iw {
            let v =
                |dx: usize, dy: usize, c: usize| t.px[(y + dy - 1) * t.w + x + dx - 1][c] as f32;
            let own = t.px[y * t.w + x];
            out.push(std::array::from_fn(|c| {
                if c == 3 {
                    return own[3];
                }
                let gx = v(2, 0, c) + 2.0 * v(2, 1, c) + v(2, 2, c)
                    - v(0, 0, c)
                    - 2.0 * v(0, 1, c)
                    - v(0, 2, c);
                let gy = v(0, 2, c) + 2.0 * v(1, 2, c) + v(2, 2, c)
                    - v(0, 0, c)
                    - 2.0 * v(1, 0, c)
                    - v(2, 0, c);
                (255.0 - (gx * gx + gy * gy).sqrt() / 4.0)
                    .round()
                    .clamp(0.0, 255.0) as u8
            }));
        }
    }
    out
}

/// Pixels a Reduce Noise tile reads around itself.
const REDUCE_NOISE_HALO: usize = 20;

/// Reduce Noise: luminance smoothed where it varies little (guided filter:
/// details whose contrast stands above the noise are kept), color noise
/// blurred away, details sharpened back.
fn reduce_noise_tile(t: &Tile, n: &ReduceNoise) -> Vec<[u8; 4]> {
    let t = &t.visible_colors();
    let (w, h) = (t.w, t.h);
    let mut y = t.plane(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32);
    let mut cb: Vec<f32> = t.px.iter().zip(&y).map(|(p, y)| p[2] as f32 - y).collect();
    let mut cr: Vec<f32> = t.px.iter().zip(&y).map(|(p, y)| p[0] as f32 - y).collect();
    if n.strength > 0.0 {
        let eps = (n.strength * 2.5 * (1.0 - 0.85 * n.preserve_details / 100.0)).powi(2);
        let mut mean = y.clone();
        box_plane(&mut mean, w, h, 2);
        let mut square: Vec<f32> = y.iter().map(|v| v * v).collect();
        box_plane(&mut square, w, h, 2);
        let mut a: Vec<f32> = mean
            .iter()
            .zip(&square)
            .map(|(m, s)| {
                let var = (s - m * m).max(0.0);
                var / (var + eps)
            })
            .collect();
        let mut b: Vec<f32> = a.iter().zip(&mean).map(|(a, m)| (1.0 - a) * m).collect();
        box_plane(&mut a, w, h, 2);
        box_plane(&mut b, w, h, 2);
        for ((v, a), b) in y.iter_mut().zip(&a).zip(&b) {
            *v = a * *v + b;
        }
    }
    if n.sharpen_details > 0.0 {
        let mut soft = y.clone();
        gauss_plane(&mut soft, w, h, 1.0);
        let k = 1.5 * n.sharpen_details / 100.0;
        for (v, s) in y.iter_mut().zip(&soft) {
            *v += k * (*v - s);
        }
    }
    if n.color_noise > 0.0 {
        let amount = n.color_noise / 100.0;
        let sigma = 1.0 + 5.0 * amount;
        let mix = (2.0 * amount).min(1.0);
        for plane in [&mut cb, &mut cr] {
            let mut soft = plane.clone();
            gauss_plane(&mut soft, w, h, sigma);
            for (v, s) in plane.iter_mut().zip(&soft) {
                *v += mix * (s - *v);
            }
        }
    }
    let (iw, ih) = t.inner();
    let mut out = Vec::with_capacity(iw * ih);
    for ty in t.halo..t.halo + ih {
        for tx in t.halo..t.halo + iw {
            let i = ty * w + tx;
            let (r, b) = (cr[i] + y[i], cb[i] + y[i]);
            let g = (y[i] - 0.299 * r - 0.114 * b) / 0.587;
            let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
            out.push([c(r), c(g), c(b), t.px[i][3]]);
        }
    }
    out
}

// ------------------------------------------------------------ pixel by pixel

/// Integer hash (lowbias32): the noise of a pixel only depends on its
/// position and the seed.
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Random number in [0, 1) for pixel (x, y), draw `k`.
fn random(seed: u32, x: i64, y: i64, k: u32) -> f32 {
    let h = hash(seed ^ hash(x as u32 ^ hash(y as u32 ^ hash(k.wrapping_add(0x9e37_79b9)))));
    (h >> 8) as f32 / (1 << 24) as f32
}

/// Add Noise: uniform within ± amount % of 255, or Gaussian of the same
/// spread; the same on the three channels if monochromatic.
fn add_noise(image: &mut RgbaImage, n: &AddNoise, origin: (i64, i64)) {
    let w = image.width() as usize;
    let spread = n.amount / 100.0 * 255.0;
    let draw = |x: i64, y: i64, c: u32| {
        if n.gaussian {
            let u = random(n.seed, x, y, 2 * c).max(1e-7);
            let v = random(n.seed, x, y, 2 * c + 1);
            spread / 2.0 * (-2.0 * u.ln()).sqrt() * (std::f32::consts::TAU * v).cos()
        } else {
            spread * (2.0 * random(n.seed, x, y, c) - 1.0)
        }
    };
    par_rows(image.as_mut(), w * 4, |first, band| {
        for (i, p) in band.chunks_exact_mut(4).enumerate() {
            let (x, y) = (origin.0 + (i % w) as i64, origin.1 + (first + i / w) as i64);
            let mono = n.monochromatic.then(|| draw(x, y, 0));
            for (c, v) in p[..3].iter_mut().enumerate() {
                let d = mono.unwrap_or_else(|| draw(x, y, c as u32));
                *v = (*v as f32 + d).round().clamp(0.0, 255.0) as u8;
            }
        }
    });
}

/// Clouds: fractal noise between the foreground and background colors,
/// opaque.
fn clouds(image: &mut RgbaImage, c: &Clouds, origin: (i64, i64)) {
    let w = image.width() as usize;
    par_rows(image.as_mut(), w * 4, |first, band| {
        for (i, p) in band.chunks_exact_mut(4).enumerate() {
            let x = (origin.0 + (i % w) as i64) as f32;
            let y = (origin.1 + (first + i / w) as i64) as f32;
            let t = cloud_value(c.seed, x, y);
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
            p.copy_from_slice(&[
                mix(c.foreground[0], c.background[0]),
                mix(c.foreground[1], c.background[1]),
                mix(c.foreground[2], c.background[2]),
                255,
            ]);
        }
    });
}

/// Octaves of smooth value noise, from 256 px to 2 px, contrast stretched:
/// 0 to 1.
fn cloud_value(seed: u32, x: f32, y: f32) -> f32 {
    let (mut sum, mut total, mut amplitude) = (0.0, 0.0, 1.0);
    for octave in 0..8u32 {
        let period = (256 >> octave) as f32;
        let (u, v) = (x / period, y / period);
        let (i, j) = (u.floor(), v.floor());
        let s = |t: f32| t * t * (3.0 - 2.0 * t);
        let (fx, fy) = (s(u - i), s(v - j));
        let corner = |di: f32, dj: f32| {
            random(
                seed.wrapping_add(octave),
                (i + di) as i64,
                (j + dj) as i64,
                7,
            )
        };
        let top = corner(0.0, 0.0) * (1.0 - fx) + corner(1.0, 0.0) * fx;
        let bottom = corner(0.0, 1.0) * (1.0 - fx) + corner(1.0, 1.0) * fx;
        sum += amplitude * (top * (1.0 - fy) + bottom * fy);
        total += amplitude;
        amplitude *= 0.5;
    }
    ((sum / total - 0.5) * 2.2 + 0.5).clamp(0.0, 1.0)
}

/// Mosaic: squares of `cell` pixels, aligned on the canvas, filled with
/// their average color.
fn mosaic(image: &mut RgbaImage, cell: i64, origin: (i64, i64)) {
    let src = image.clone();
    let (w, h) = (image.width() as i64, image.height() as i64);
    // First cell boundary at or before 0, in image coordinates.
    let start = |o: i64| -o.rem_euclid(cell);
    let raw = src.as_raw();
    par_rows(image.as_mut(), w as usize * 4, |first, band| {
        let (y0, y1) = (first as i64, first as i64 + band.len() as i64 / (w * 4));
        let mut cy = start(origin.1) + (y0 - start(origin.1)).div_euclid(cell) * cell;
        while cy < y1 {
            let (ya, yb) = (cy.max(0), (cy + cell).min(h));
            let mut cx = start(origin.0);
            while cx < w {
                let (xa, xb) = (cx.max(0), (cx + cell).min(w));
                let mut sum = [0u64; 4];
                for y in ya..yb {
                    for x in xa..xb {
                        let i = ((y * w + x) * 4) as usize;
                        let a = raw[i + 3] as u64;
                        for c in 0..3 {
                            sum[c] += raw[i + c] as u64 * a;
                        }
                        sum[3] += a;
                    }
                }
                let n = ((yb - ya) * (xb - xa)) as u64;
                let px = if sum[3] == 0 {
                    [0; 4]
                } else {
                    let c = |s: u64| ((s + sum[3] / 2) / sum[3]) as u8;
                    [
                        c(sum[0]),
                        c(sum[1]),
                        c(sum[2]),
                        ((sum[3] + n / 2) / n) as u8,
                    ]
                };
                for y in ya.max(y0)..yb.min(y1) {
                    let at = (((y - y0) * w + xa) * 4) as usize;
                    for d in band[at..at + (xb - xa) as usize * 4].chunks_exact_mut(4) {
                        d.copy_from_slice(&px);
                    }
                }
                cx += cell;
            }
            cy += cell;
        }
    });
}

/// Offset: the image moved, what it uncovers transparent, repeated from its
/// edges, or wrapped from the other side.
fn offset(image: &mut RgbaImage, o: &Offset) {
    let src = image.clone();
    let (w, h) = (image.width() as i64, image.height() as i64);
    let (dx, dy) = (o.horizontal.round() as i64, o.vertical.round() as i64);
    let raw = src.as_raw();
    par_rows(image.as_mut(), w as usize * 4, |first, band| {
        for (i, d) in band.chunks_exact_mut(4).enumerate() {
            let (x, y) = (i as i64 % w - dx, first as i64 + i as i64 / w - dy);
            let (x, y) = match o.undefined {
                Undefined::WrapAround => (x.rem_euclid(w), y.rem_euclid(h)),
                Undefined::RepeatEdges => (x.clamp(0, w - 1), y.clamp(0, h - 1)),
                Undefined::Transparent if x < 0 || y < 0 || x >= w || y >= h => {
                    d.fill(0);
                    continue;
                }
                Undefined::Transparent => (x, y),
            };
            let s = ((y * w + x) * 4) as usize;
            d.copy_from_slice(&raw[s..s + 4]);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_ops::ClipShape;
    use image::Rgba;

    /// Transparent `w × h` image with one pixel of `color` at (x, y).
    fn impulse(w: u32, h: u32, (x, y): (u32, u32), color: [u8; 4]) -> RgbaImage {
        let mut image = RgbaImage::new(w, h);
        image.put_pixel(x, y, Rgba(color));
        image
    }

    /// Sum of the alpha values, and their variance along x and y around (cx, cy).
    fn spread(image: &RgbaImage, (cx, cy): (f64, f64)) -> (f64, f64, f64) {
        let (mut total, mut vx, mut vy) = (0.0, 0.0, 0.0);
        for (x, y, p) in image.enumerate_pixels() {
            let a = p.0[3] as f64;
            total += a;
            vx += a * (x as f64 - cx).powi(2);
            vy += a * (y as f64 - cy).powi(2);
        }
        (total, vx / total, vy / total)
    }

    #[test]
    fn gaussian_impulse_response_has_the_radius_as_deviation() {
        // A line across a 16-bit buffer: the profile across it is the 1D
        // response, precise enough to measure in the tails.
        let n = 241;
        for radius in [0.6f32, 1.0, 1.5, 3.0, 8.0, 20.0, 37.5] {
            for vertical in [false, true] {
                let mut buf = vec![0u16; n * n * 4];
                for i in 0..n {
                    let (x, y) = if vertical { (n / 2, i) } else { (i, n / 2) };
                    buf[(y * n + x) * 4..][..4].fill(65280);
                }
                blur_buffer(&mut buf, n, &gaussian_passes(radius));
                let profile: Vec<f64> = (0..n)
                    .map(|i| {
                        let (x, y) = if vertical { (i, n / 2) } else { (n / 2, i) };
                        buf[(y * n + x) * 4] as f64
                    })
                    .collect();
                let mass: f64 = profile.iter().sum();
                let c = (n / 2) as f64;
                let variance = profile
                    .iter()
                    .enumerate()
                    .map(|(i, v)| v * (i as f64 - c).powi(2))
                    .sum::<f64>()
                    / mass;
                assert!(
                    (mass - 65280.0).abs() < 65280.0 * 0.003,
                    "radius {radius}: mass {mass}"
                );
                let sd = variance.sqrt();
                assert!(
                    (sd - radius as f64).abs() < radius as f64 * 0.03,
                    "radius {radius}: σ {sd}"
                );
                for k in 1..n / 2 {
                    assert!(
                        profile[n / 2 - k].abs_diff_eq(profile[n / 2 + k]),
                        "symmetric"
                    );
                    assert!(profile[n / 2 + k] <= profile[n / 2 + k - 1], "decreasing");
                }
            }
        }
        // A point: the same in both directions, and the color kept.
        let mut image = impulse(41, 41, (20, 20), [255, 255, 255, 255]);
        Filter::GaussianBlur { radius: 1.5 }.apply(&mut image, (0, 0));
        assert_eq!(image.get_pixel(17, 20), image.get_pixel(20, 23));
        assert_eq!(image.get_pixel(18, 19).0[..3], [255, 255, 255]);
    }

    trait Close {
        fn abs_diff_eq(self, other: Self) -> bool;
    }

    impl Close for f64 {
        fn abs_diff_eq(self, other: f64) -> bool {
            (self - other).abs() <= 1.0
        }
    }

    #[test]
    fn box_passes_have_the_exact_variance() {
        for radius in [2.0f32, 2.7, 5.0, 33.3] {
            let passes = gaussian_passes(radius);
            let variance: f32 = passes
                .iter()
                .map(|p| match p {
                    Pass::Box { r, w } => {
                        let r = *r as f32;
                        (r * (r + 1.0) * (2.0 * r + 1.0) / 3.0 + 2.0 * w * (r + 1.0).powi(2))
                            / (2.0 * r + 1.0 + 2.0 * w)
                    }
                    Pass::Kernel(_) => unreachable!(),
                })
                .sum();
            assert!((variance - radius * radius).abs() < 1e-2 * radius * radius);
        }
    }

    #[test]
    fn blurs_keep_a_uniform_image_up_to_the_edges() {
        let color = [200, 40, 90, 255];
        for filter in [
            Filter::GaussianBlur { radius: 1.2 },
            Filter::GaussianBlur { radius: 30.0 },
            Filter::BoxBlur { radius: 4.5 },
            Filter::MotionBlur {
                angle: 30.0,
                distance: 25.0,
            },
            Filter::MotionBlur {
                angle: 80.0,
                distance: 9.0,
            },
            Filter::SurfaceBlur {
                radius: 3.0,
                threshold: 15.0,
            },
            Filter::SurfaceBlur {
                radius: 12.0,
                threshold: 40.0,
            },
            Filter::Median { radius: 2.0 },
            Filter::Minimum(Morphology {
                radius: 3.0,
                roundness: true,
            }),
            FilterKind::ReduceNoise.initial(),
            FilterKind::SmartSharpen.initial(),
            FilterKind::UnsharpMask.initial(),
            Filter::Mosaic { cell: 7.0 },
        ] {
            let mut image = RgbaImage::from_pixel(70, 50, Rgba(color));
            filter.apply(&mut image, (3, 5));
            let off = image
                .pixels()
                .find(|p| p.0.iter().zip(color).any(|(a, b)| a.abs_diff(b) > 1));
            assert!(off.is_none(), "{filter:?}: {off:?}");
        }
    }

    #[test]
    fn colors_of_transparent_pixels_do_not_bleed() {
        // Red opaque square on transparent black: the blurred edge stays
        // red, only more transparent.
        let mut image = RgbaImage::new(40, 40);
        for y in 10..30 {
            for x in 10..30 {
                image.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        for filter in [
            Filter::GaussianBlur { radius: 4.0 },
            Filter::BoxBlur { radius: 3.0 },
            Filter::MotionBlur {
                angle: 45.0,
                distance: 12.0,
            },
            Filter::Median { radius: 2.0 },
            Filter::Maximum(Morphology {
                radius: 2.0,
                roundness: false,
            }),
        ] {
            let mut out = image.clone();
            filter.apply(&mut out, (0, 0));
            let semi: Vec<_> = out
                .pixels()
                .filter(|p| p.0[3] > 20 && p.0[3] < 235)
                .collect();
            assert!(
                !semi.is_empty() || matches!(filter, Filter::Maximum(_) | Filter::Median { .. })
            );
            for p in out.pixels().filter(|p| p.0[3] > 20) {
                assert!(
                    p.0[0] >= 250 && p.0[1] <= 3 && p.0[2] <= 3,
                    "{filter:?}: {p:?}"
                );
            }
        }
    }

    #[test]
    fn hidden_colors_do_not_change_visible_pixels() {
        // The same visible disc, with black or white hidden colors around.
        let disc = |hidden: u8| {
            RgbaImage::from_fn(40, 40, |x, y| {
                if (x as i32 - 20).pow(2) + (y as i32 - 20).pow(2) < 100 {
                    Rgba([200, 120, 40, 255])
                } else {
                    Rgba([hidden, hidden, hidden, 0])
                }
            })
        };
        for kind in [
            FilterKind::BoxBlur,
            FilterKind::GaussianBlur,
            FilterKind::MotionBlur,
            FilterKind::SurfaceBlur,
            FilterKind::DustAndScratches,
            FilterKind::Median,
            FilterKind::ReduceNoise,
            FilterKind::Mosaic,
            FilterKind::SmartSharpen,
            FilterKind::UnsharpMask,
            FilterKind::Emboss,
            FilterKind::FindEdges,
            FilterKind::HighPass,
            FilterKind::Maximum,
            FilterKind::Minimum,
        ] {
            let filter = kind.initial();
            let (mut a, mut b) = (disc(0), disc(255));
            filter.apply(&mut a, (0, 0));
            filter.apply(&mut b, (0, 0));
            for (p, q) in a.pixels().zip(b.pixels()) {
                if p[3] > 0 || q[3] > 0 {
                    let d = (0..4).map(|c| p[c].abs_diff(q[c])).max().unwrap();
                    assert!(d <= 2, "{kind:?}: {p:?} vs {q:?}");
                }
            }
        }
    }

    #[test]
    fn motion_blur_spreads_along_its_angle_only() {
        for (angle, along) in [(0.0f32, (1, 0)), (90.0, (0, 1)), (45.0, (1, -1))] {
            let mut image = impulse(81, 81, (40, 40), [0, 0, 255, 255]);
            Filter::MotionBlur {
                angle,
                distance: 21.0,
            }
            .apply(&mut image, (0, 0));
            let at = |k: i32| {
                image
                    .get_pixel((40 + k * along.0) as u32, (40 + k * along.1) as u32)
                    .0[3]
            };
            let across = |k: i32| {
                image
                    .get_pixel((40 + k * along.1) as u32, (40 - k * along.0) as u32)
                    .0[3]
            };
            assert!(at(6) > 0 && at(-6) > 0, "angle {angle}");
            assert!(at(6).abs_diff(at(-6)) <= 1, "angle {angle}: symmetric");
            assert!(across(4) == 0, "angle {angle}: nothing across");
            let (total, ..) = spread(&image, (40.0, 40.0));
            assert!((total - 255.0).abs() < 30.0, "angle {angle}: mass {total}");
        }
    }

    #[test]
    fn box_blur_averages_a_square() {
        let mut image = impulse(21, 21, (10, 10), [255, 255, 255, 255]);
        Filter::BoxBlur { radius: 1.0 }.apply(&mut image, (0, 0));
        // 255 / 9 over the 3 × 3 square, nothing beyond.
        assert_eq!(image.get_pixel(9, 11).0[3], 28);
        assert_eq!(image.get_pixel(10, 10).0[3], 28);
        assert_eq!(image.get_pixel(12, 10).0[3], 0);
    }

    #[test]
    fn edges_repeat_beyond_the_image() {
        // Left half black, right half white: blurring keeps the far left
        // black (no transparent or white coming from outside).
        let mut image = RgbaImage::from_fn(40, 10, |x, _| {
            if x < 20 {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        Filter::GaussianBlur { radius: 3.0 }.apply(&mut image, (0, 0));
        assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(39, 9).0, [255, 255, 255, 255]);
        let mid = image.get_pixel(19, 5).0[0];
        assert!(mid > 64 && mid < 191, "{mid}");
    }

    #[test]
    fn surface_blur_smooths_areas_but_keeps_edges() {
        // Noisy dark and light halves.
        let image = RgbaImage::from_fn(60, 30, |x, y| {
            let base = if x < 30 { 60 } else { 190 };
            let n = (hash(x * 31 + y * 7) % 11) as i32 - 5;
            let v = (base + n) as u8;
            Rgba([v, v, v, 255])
        });
        for radius in [2.0, 10.0] {
            let mut out = image.clone();
            Filter::SurfaceBlur {
                radius,
                threshold: 15.0,
            }
            .apply(&mut out, (0, 0));
            // Mean distance to the base level, in the middle of each half.
            let dev = |image: &RgbaImage, x0: u32, base: f32| {
                (0..30)
                    .flat_map(|y| (x0..x0 + 20).map(move |x| (x, y)))
                    .map(|(x, y)| (image.get_pixel(x, y).0[0] as f32 - base).abs())
                    .sum::<f32>()
                    / 600.0
            };
            for (x0, base) in [(3, 60.0), (37, 190.0)] {
                let (before, after) = (dev(&image, x0, base), dev(&out, x0, base));
                assert!(after < before / 2.0, "radius {radius}: {before} → {after}");
            }
            assert!(out.get_pixel(29, 10).0[0] < 70 && out.get_pixel(30, 10).0[0] > 180);
        }
    }

    #[test]
    fn median_removes_specks_and_dust_respects_the_threshold() {
        let mut image = RgbaImage::from_pixel(20, 20, Rgba([100, 100, 100, 255]));
        image.put_pixel(5, 5, Rgba([255, 255, 255, 255]));
        image.put_pixel(12, 12, Rgba([110, 110, 110, 255]));
        let mut median = image.clone();
        Filter::Median { radius: 1.0 }.apply(&mut median, (0, 0));
        assert_eq!(median.get_pixel(5, 5).0, [100, 100, 100, 255]);
        assert_eq!(median.get_pixel(12, 12).0, [100, 100, 100, 255]);
        let mut dust = image.clone();
        Filter::DustAndScratches {
            radius: 1.0,
            threshold: 20.0,
        }
        .apply(&mut dust, (0, 0));
        assert_eq!(dust.get_pixel(5, 5).0, [100, 100, 100, 255]);
        assert_eq!(
            dust.get_pixel(12, 12).0,
            [110, 110, 110, 255],
            "within the threshold"
        );
    }

    #[test]
    fn minimum_and_maximum_follow_the_neighborhood_shape() {
        let dot = impulse(21, 21, (10, 10), [255, 255, 255, 255]);
        let mut square = dot.clone();
        Filter::Maximum(Morphology {
            radius: 3.0,
            roundness: false,
        })
        .apply(&mut square, (0, 0));
        assert_eq!(square.get_pixel(13, 13).0[3], 255);
        assert_eq!(square.get_pixel(14, 10).0[3], 0);
        let mut disc = dot.clone();
        Filter::Maximum(Morphology {
            radius: 3.0,
            roundness: true,
        })
        .apply(&mut disc, (0, 0));
        assert_eq!(disc.get_pixel(13, 10).0[3], 255);
        assert_eq!(disc.get_pixel(13, 13).0[3], 0, "corner outside the disc");
        let mut shrunk = square.clone();
        Filter::Minimum(Morphology {
            radius: 3.0,
            roundness: false,
        })
        .apply(&mut shrunk, (0, 0));
        assert_eq!(shrunk, dot, "minimum undoes maximum on a square");
    }

    #[test]
    fn sharpening_and_high_pass() {
        let step = RgbaImage::from_fn(30, 4, |x, _| {
            if x < 15 {
                Rgba([80, 80, 80, 255])
            } else {
                Rgba([160, 160, 160, 255])
            }
        });
        let mut usm = step.clone();
        FilterKind::UnsharpMask.initial().apply(&mut usm, (0, 0));
        assert!(usm.get_pixel(14, 0).0[0] < 80 && usm.get_pixel(15, 0).0[0] > 160);
        assert_eq!(usm.get_pixel(2, 0).0[0], 80);
        let mut held = step.clone();
        Filter::UnsharpMask {
            amount: 100.0,
            radius: 1.0,
            threshold: 255.0,
        }
        .apply(&mut held, (0, 0));
        assert_eq!(held, step, "nothing differs by the threshold");
        let mut smart = step.clone();
        FilterKind::SmartSharpen.initial().apply(&mut smart, (0, 0));
        assert!(smart.get_pixel(14, 0).0[0] < 80 && smart.get_pixel(15, 0).0[0] > 160);
        let mut high = step.clone();
        Filter::HighPass { radius: 2.0 }.apply(&mut high, (0, 0));
        assert_eq!(high.get_pixel(2, 0).0[0], 128);
        assert!(high.get_pixel(14, 0).0[0] < 128 && high.get_pixel(15, 0).0[0] > 128);
    }

    #[test]
    fn noise_is_anchored_to_the_canvas() {
        let image = RgbaImage::from_pixel(32, 32, Rgba([128, 128, 128, 255]));
        let noise = Filter::AddNoise(AddNoise {
            amount: 20.0,
            gaussian: true,
            monochromatic: true,
            seed: 7,
        });
        let mut whole = image.clone();
        noise.apply(&mut whole, (0, 0));
        let mut part = image::imageops::crop_imm(&image, 10, 10, 8, 8).to_image();
        noise.apply(&mut part, (10, 10));
        assert_eq!(
            part,
            image::imageops::crop_imm(&whole, 10, 10, 8, 8).to_image()
        );
        assert!(
            whole.pixels().all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2]),
            "monochromatic"
        );
        let mean = whole.pixels().map(|p| p.0[0] as f64).sum::<f64>() / 1024.0;
        assert!((mean - 128.0).abs() < 3.0, "{mean}");
        assert!(whole.pixels().any(|p| p.0[0] != 128));
    }

    #[test]
    fn offset_wraps_repeats_or_clears() {
        let image = RgbaImage::from_fn(4, 1, |x, _| Rgba([x as u8 * 10, 0, 0, 255]));
        let shifted = |undefined| {
            let mut out = image.clone();
            Filter::Offset(Offset {
                horizontal: 1.0,
                vertical: 0.0,
                undefined,
            })
            .apply(&mut out, (0, 0));
            out.pixels().map(|p| (p.0[0], p.0[3])).collect::<Vec<_>>()
        };
        assert_eq!(
            shifted(Undefined::WrapAround),
            [(30, 255), (0, 255), (10, 255), (20, 255)]
        );
        assert_eq!(
            shifted(Undefined::RepeatEdges),
            [(0, 255), (0, 255), (10, 255), (20, 255)]
        );
        assert_eq!(
            shifted(Undefined::Transparent),
            [(0, 0), (0, 255), (10, 255), (20, 255)]
        );
    }

    #[test]
    fn mosaic_cells_are_aligned_on_the_canvas() {
        let image = RgbaImage::from_fn(8, 2, |x, _| Rgba([x as u8 * 20, 0, 0, 255]));
        let mut out = image.clone();
        // Image at x = 2 in the canvas: cells start at canvas 0 and 4.
        Filter::Mosaic { cell: 4.0 }.apply(&mut out, (2, 0));
        let reds: Vec<u8> = (0..8).map(|x| out.get_pixel(x, 0).0[0]).collect();
        assert_eq!(reds, [10, 10, 70, 70, 70, 70, 130, 130]);
    }

    #[test]
    fn emboss_find_edges_and_clouds() {
        let image = RgbaImage::from_fn(20, 20, |x, _| {
            if x < 10 {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        let mut emboss = image.clone();
        FilterKind::Emboss.initial().apply(&mut emboss, (0, 0));
        assert_eq!(
            emboss.get_pixel(3, 10).0,
            [128, 128, 128, 255],
            "flat: gray"
        );
        // Lit from the top left: the left edge of the white half is bright.
        assert!(emboss.get_pixel(10, 10).0[0] > 200);
        let mut edges = image.clone();
        Filter::FindEdges.apply(&mut edges, (0, 0));
        assert_eq!(edges.get_pixel(3, 3).0, [255, 255, 255, 255]);
        assert!(edges.get_pixel(10, 3).0[0] < 50);
        let mut clouds = RgbaImage::new(256, 256);
        let c = Clouds {
            foreground: [0, 0, 0],
            background: [255, 255, 255],
            seed: 3,
        };
        Filter::Clouds(c).apply(&mut clouds, (0, 0));
        assert!(clouds.pixels().all(|p| p.0[3] == 255 && p.0[0] == p.0[1]));
        let (lo, hi) = clouds
            .pixels()
            .fold((255, 0), |(lo, hi), p| (lo.min(p.0[0]), hi.max(p.0[0])));
        assert!(hi - lo > 60, "{lo}..{hi}");
    }

    #[test]
    fn target_blends_inside_a_soft_selection_and_reads_around_it() {
        // Left half white, right half black; the selection: the right
        // half's first column at full coverage, the next one at half.
        let image = Arc::new(RgbaImage::from_fn(20, 4, |x, _| {
            if x < 10 {
                Rgba([255, 255, 255, 255])
            } else {
                Rgba([0, 0, 0, 255])
            }
        }));
        let mask =
            image::GrayImage::from_fn(2, 4, |x, _| image::Luma([if x == 0 { 255 } else { 128 }]));
        let target = Target {
            image: Arc::clone(&image),
            clip: Some(Clip {
                rect: [10, 0, 12, 4],
                shape: Some(ClipShape::Mask(Arc::new(mask), (10, 0))),
                inverted: false,
            }),
            offset: (0, 0),
        };
        let filter = Filter::BoxBlur { radius: 2.0 };
        let out = target.process(filter.reach().unwrap(), |i, o| filter.apply(i, o));
        // Blurred with the white beyond the selection.
        assert_eq!(out.get_pixel(10, 1).0[0], 102);
        assert!(out.get_pixel(11, 1).0[0] > 0 && out.get_pixel(11, 1).0[0] < 51);
        assert_eq!(out.get_pixel(9, 1).0, [255, 255, 255, 255]);
        assert_eq!(out.get_pixel(12, 1).0, [0, 0, 0, 255]);
        // A region of it: the same pixels.
        let part = target.process_region([8, 0, 14, 4], filter.reach(), |i, o| filter.apply(i, o));
        assert_eq!(part, image::imageops::crop_imm(&out, 8, 0, 6, 4).to_image());
    }

    /// `cargo test --release gaussian_speed -- --ignored --nocapture`
    #[test]
    #[ignore = "timing, in release"]
    fn gaussian_speed() {
        let mut image = RgbaImage::from_fn(6000, 4000, |x, y| Rgba([x as u8, y as u8, 90, 255]));
        for radius in [1.5, 10.0, 250.0] {
            let start = std::time::Instant::now();
            Filter::GaussianBlur { radius }.apply(&mut image, (0, 0));
            println!("gaussian {radius}: {:?}", start.elapsed());
        }
        for filter in [
            FilterKind::MotionBlur.initial(),
            FilterKind::SurfaceBlur.initial(),
            FilterKind::Median.initial(),
            FilterKind::ReduceNoise.initial(),
            FilterKind::UnsharpMask.initial(),
        ] {
            let start = std::time::Instant::now();
            filter.apply(&mut image, (0, 0));
            println!("{filter:?}: {:?}", start.elapsed());
        }
    }
}
