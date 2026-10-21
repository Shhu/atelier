//! DNG files: the raw data (uncompressed, lossless JPEG, Deflate or lossy
//! JPEG; Bayer mosaic or linear RGB), linearized and scaled between black
//! and white levels, demosaicked, with the camera's color matrices and
//! white balance as shot, per the DNG specification.

use std::io::Read;

use image::metadata::Orientation;

use super::demosaic::malvar;
use super::ljpeg;
use super::tiff::{Ifd, Tiff};
use crate::camera_raw::source::{Calibration, Camera, Source};
use crate::camera_raw::white::{self, Mat3};

/// Why a DNG could not be decoded.
#[derive(Debug, PartialEq, Eq)]
pub enum DngError {
    /// A variant this decoder does not handle (its preview may do).
    Unsupported,
    Damaged,
}

use DngError::{Damaged, Unsupported};

// Tags.
const NEW_SUBFILE_TYPE: u16 = 254;
const WIDTH: u16 = 256;
const HEIGHT: u16 = 257;
const BITS: u16 = 258;
const COMPRESSION: u16 = 259;
const PHOTOMETRIC: u16 = 262;
const STRIP_OFFSETS: u16 = 273;
const ORIENTATION: u16 = 274;
const SAMPLES: u16 = 277;
const ROWS_PER_STRIP: u16 = 278;
const STRIP_COUNTS: u16 = 279;
const PLANAR: u16 = 284;
const PREDICTOR: u16 = 317;
const TILE_WIDTH: u16 = 322;
const TILE_HEIGHT: u16 = 323;
const TILE_OFFSETS: u16 = 324;
const TILE_COUNTS: u16 = 325;
const SAMPLE_FORMAT: u16 = 339;
const CFA_REPEAT: u16 = 33421;
const CFA_PATTERN: u16 = 33422;
const DNG_VERSION: u16 = 50706;
const CFA_PLANE_COLOR: u16 = 50710;
const CFA_LAYOUT: u16 = 50711;
const LINEARIZATION: u16 = 50712;
const BLACK_REPEAT: u16 = 50713;
const BLACK_LEVEL: u16 = 50714;
const BLACK_DELTA_H: u16 = 50715;
const BLACK_DELTA_V: u16 = 50716;
const WHITE_LEVEL: u16 = 50717;
const CROP_ORIGIN: u16 = 50719;
const CROP_SIZE: u16 = 50720;
const COLOR_MATRIX: [u16; 2] = [50721, 50722];
const CAMERA_CALIBRATION: [u16; 2] = [50723, 50724];
const ANALOG_BALANCE: u16 = 50727;
const AS_SHOT_NEUTRAL: u16 = 50728;
const AS_SHOT_WHITE_XY: u16 = 50729;
const BASELINE_EXPOSURE: u16 = 50730;
const ILLUMINANT: [u16; 2] = [50778, 50779];
const ACTIVE_AREA: u16 = 50829;
const FORWARD_MATRIX: [u16; 2] = [50964, 50965];
const OPCODE_LIST_2: u16 = 51009;

const CFA: u32 = 32803;
const LINEAR_RAW: u32 = 34892;

/// Whether `data` is a DNG file.
pub fn is_dng(data: &[u8]) -> bool {
    Tiff::new(data).is_some_and(|(tiff, first)| {
        tiff.ifd(first)
            .is_some_and(|(ifd0, _)| ifd0.get(DNG_VERSION).is_some())
    })
}

/// The developed-to-be pixels of a DNG, the right way up.
pub fn decode(data: &[u8]) -> Result<Source, DngError> {
    let (tiff, first) = Tiff::new(data).ok_or(Damaged)?;
    let ifds = tiff.all_ifds(first);
    let ifd0 = ifds.first().ok_or(Damaged)?;
    if ifd0.get(DNG_VERSION).is_none() {
        return Err(Unsupported);
    }
    let number = |ifd: &Ifd, tag| tiff.number(ifd, tag);
    let raw = ifds
        .iter()
        .filter(|ifd| {
            number(ifd, NEW_SUBFILE_TYPE).unwrap_or(0.0) == 0.0
                && matches!(
                    number(ifd, PHOTOMETRIC).map(|v| v as u32),
                    Some(CFA | LINEAR_RAW)
                )
        })
        .max_by_key(|ifd| {
            (number(ifd, WIDTH).unwrap_or(0.0) * number(ifd, HEIGHT).unwrap_or(0.0)) as u64
        })
        .ok_or(Unsupported)?;
    // Color tags live in IFD 0 (some writers put them with the data).
    let color_number = |tag| number(ifd0, tag).or_else(|| number(raw, tag));
    let color_numbers = |tag| tiff.numbers(ifd0, tag).or_else(|| tiff.numbers(raw, tag));

    let w = number(raw, WIDTH).ok_or(Damaged)? as usize;
    let h = number(raw, HEIGHT).ok_or(Damaged)? as usize;
    let spp = number(raw, SAMPLES).unwrap_or(1.0) as usize;
    let photometric = number(raw, PHOTOMETRIC).unwrap_or(0.0) as u32;
    let bits = number(raw, BITS).unwrap_or(16.0) as u32;
    crate::document::check_size(w as u32, h as u32).map_err(|_| Unsupported)?;
    if number(raw, SAMPLE_FORMAT).unwrap_or(1.0) != 1.0
        || number(raw, PLANAR).unwrap_or(1.0) != 1.0
        || !(1..=16).contains(&bits)
        || (photometric == CFA && spp != 1)
        || (photometric == LINEAR_RAW && spp != 3)
    {
        return Err(Unsupported);
    }
    let mut samples = read_samples(&tiff, raw, w, h, spp, bits)?;

    // Linearization, black and white levels.
    let mut white_max = ((1u64 << bits) - 1) as f64;
    if let Some(table) = tiff.numbers(raw, LINEARIZATION).filter(|t| !t.is_empty()) {
        let table: Vec<u16> = table
            .iter()
            .map(|&v| v.clamp(0.0, 65535.0) as u16)
            .collect();
        white_max = *table.iter().max().unwrap_or(&0) as f64;
        for s in &mut samples {
            *s = table[(*s as usize).min(table.len() - 1)];
        }
    }
    let area = tiff
        .numbers(raw, ACTIVE_AREA)
        .filter(|a| a.len() == 4)
        .map_or([0, 0, h, w], |a| {
            [a[0], a[1], a[2], a[3]].map(|v| v as usize)
        });
    let [top, left, bottom, right] = area;
    if top >= bottom || left >= right || bottom > h || right > w {
        return Err(Damaged);
    }
    let (aw, ah) = (right - left, bottom - top);
    let repeat = tiff
        .numbers(raw, BLACK_REPEAT)
        .filter(|r| r.len() == 2 && r[0] >= 1.0 && r[1] >= 1.0)
        .map_or((1, 1), |r| (r[0] as usize, r[1] as usize));
    let blacks = tiff.numbers(raw, BLACK_LEVEL).unwrap_or_default();
    let delta_h = tiff.numbers(raw, BLACK_DELTA_H).unwrap_or_default();
    let delta_v = tiff.numbers(raw, BLACK_DELTA_V).unwrap_or_default();
    let whites = tiff.numbers(raw, WHITE_LEVEL).unwrap_or_default();
    let black = |y: usize, x: usize, s: usize| {
        let i = ((y % repeat.0) * repeat.1 + x % repeat.1) * spp + s;
        blacks.get(i).or(blacks.first()).copied().unwrap_or(0.0)
            + delta_h.get(x).copied().unwrap_or(0.0)
            + delta_v.get(y).copied().unwrap_or(0.0)
    };
    let white_of = |s: usize| {
        whites
            .get(s)
            .or(whites.first())
            .copied()
            .unwrap_or(white_max)
    };
    // Levels between black (0) and white (1), then the corrections of
    // OpcodeList2 (lens shading of phones, the tone map of lossy DNGs).
    let mut levels: Vec<f32> = (0..aw * ah * spp)
        .map(|i| {
            let (s, x, y) = (i % spp, (i / spp) % aw, i / spp / aw);
            let v = samples[((top + y) * w + left + x) * spp + s] as f64;
            let b = black(y, x, s);
            (((v - b) / (white_of(s) - b).max(1.0)) as f32).clamp(0.0, 1.0)
        })
        .collect();
    if let Some(e) = raw.get(OPCODE_LIST_2) {
        let list: Vec<u8> = tiff.values(e).iter().map(|&v| v as u8).collect();
        apply_opcodes(&list, &mut levels, aw, ah, spp)?;
    }

    let camera = camera(&color_number, &color_numbers)?;
    let max = camera.as_shot.iter().copied().fold(1e-9, f64::max);
    let gains = camera.as_shot.map(|n| (max / n.max(1e-9)) as f32);
    let mut rgb = if photometric == CFA {
        let pattern = cfa_pattern(&tiff, raw)?;
        let mosaic: Vec<f32> = (0..aw * ah)
            .map(|i| {
                let (x, y) = (i % aw, i / aw);
                levels[i] * gains[pattern[(y % 2) * 2 + x % 2] as usize]
            })
            .collect();
        malvar(&mosaic, aw, ah, pattern)
    } else {
        (0..aw * ah)
            .map(|i| std::array::from_fn(|c| levels[i * 3 + c] * gains[c]))
            .collect()
    };
    // Clipped highlights: white rather than magenta.
    for p in &mut rgb {
        if (0..3).any(|c| p[c] >= gains[c] * 0.999) {
            *p = p.map(|v| v.min(1.0));
        }
    }

    let source = Source {
        width: aw as u32,
        height: ah as u32,
        rgb,
        alpha: None,
        camera: Some(std::sync::Arc::new(camera)),
    };
    let crop = match (tiff.numbers(raw, CROP_ORIGIN), tiff.numbers(raw, CROP_SIZE)) {
        (Some(o), Some(s)) if o.len() == 2 && s.len() == 2 => {
            let (x0, y0) = ((o[0] as usize).min(aw - 1), (o[1] as usize).min(ah - 1));
            let (x1, y1) = ((x0 + s[0] as usize).min(aw), (y0 + s[1] as usize).min(ah));
            [
                x0 as u32,
                y0 as u32,
                x1.max(x0 + 1) as u32,
                y1.max(y0 + 1) as u32,
            ]
        }
        _ => [0, 0, aw as u32, ah as u32],
    };
    let source = if crop == [0, 0, aw as u32, ah as u32] {
        source
    } else {
        source.crop(crop)
    };
    let orientation = number(ifd0, ORIENTATION)
        .and_then(|v| Orientation::from_exif(v as u8))
        .unwrap_or(Orientation::NoTransforms);
    Ok(source.oriented(orientation))
}

/// Applies an opcode list (big-endian, DNG 1.3) to `data` (w × h × spp
/// levels from 0 to 1): MapTable, MapPolynomial and GainMap; the other
/// opcodes are refused unless optional.
fn apply_opcodes(
    list: &[u8],
    data: &mut [f32],
    w: usize,
    h: usize,
    spp: usize,
) -> Result<(), DngError> {
    let u32_at = |b: &[u8], at: usize| -> Result<u32, DngError> {
        Ok(u32::from_be_bytes(
            b.get(at..at + 4)
                .ok_or(Damaged)?
                .try_into()
                .map_err(|_| Damaged)?,
        ))
    };
    let f64_at = |b: &[u8], at: usize| -> Result<f64, DngError> {
        Ok(f64::from_be_bytes(
            b.get(at..at + 8)
                .ok_or(Damaged)?
                .try_into()
                .map_err(|_| Damaged)?,
        ))
    };
    let count = u32_at(list, 0)?;
    let mut at = 4;
    for _ in 0..count {
        let (id, flags, len) = (
            u32_at(list, at)?,
            u32_at(list, at + 8)?,
            u32_at(list, at + 12)? as usize,
        );
        let params = list.get(at + 16..at + 16 + len).ok_or(Damaged)?;
        at += 16 + len;
        if !matches!(id, 6 | 8 | 9) {
            // Optional: may be skipped.
            if flags & 1 == 1 {
                continue;
            }
            return Err(Unsupported);
        }
        // The area: top, left, bottom, right, first plane, planes, row
        // and column pitches.
        let area: Vec<usize> = (0..8)
            .map(|k| u32_at(params, 4 * k).map(|v| v as usize))
            .collect::<Result<_, _>>()?;
        let [
            top,
            left,
            bottom,
            right,
            plane,
            planes,
            row_pitch,
            col_pitch,
        ] = area[..]
        else {
            return Err(Damaged);
        };
        let rows = (top..bottom.min(h)).step_by(row_pitch.max(1));
        let cols = (left..right.min(w)).step_by(col_pitch.max(1));
        let channels = plane..(plane + planes).min(spp);
        let mut each = |f: &dyn Fn(f32, usize, usize, usize) -> f32| {
            for y in rows.clone() {
                for x in cols.clone() {
                    for c in channels.clone() {
                        let v = &mut data[(y * w + x) * spp + c];
                        *v = f(*v, y, x, c - plane);
                    }
                }
            }
        };
        match id {
            6 => {
                let size = u32_at(params, 32)? as usize;
                let table: Vec<f32> = (0..size)
                    .map(|i| {
                        let b = params.get(36 + 2 * i..38 + 2 * i).ok_or(Damaged)?;
                        Ok(u16::from_be_bytes([b[0], b[1]]) as f32 / 65535.0)
                    })
                    .collect::<Result<_, DngError>>()?;
                if table.is_empty() {
                    return Err(Damaged);
                }
                each(&|v, _, _, _| table[((v * 65535.0).round() as usize).min(table.len() - 1)]);
            }
            8 => {
                let degree = u32_at(params, 32)? as usize;
                let c: Vec<f64> = (0..=degree.min(8))
                    .map(|i| f64_at(params, 36 + 8 * i))
                    .collect::<Result<_, _>>()?;
                each(&|v, _, _, _| {
                    let x = v as f64;
                    let y = c.iter().rev().fold(0.0, |acc, k| acc * x + k);
                    y.clamp(0.0, 1.0) as f32
                });
            }
            _ => {
                let (points_v, points_h) =
                    (u32_at(params, 32)? as usize, u32_at(params, 36)? as usize);
                let (spacing_v, spacing_h) = (f64_at(params, 40)?, f64_at(params, 48)?);
                let (origin_v, origin_h) = (f64_at(params, 56)?, f64_at(params, 64)?);
                let map_planes = u32_at(params, 72)? as usize;
                let n = points_v * points_h * map_planes;
                if n == 0 || spacing_v <= 0.0 || spacing_h <= 0.0 {
                    return Err(Damaged);
                }
                let gains: Vec<f32> = (0..n)
                    .map(|i| u32_at(params, 76 + 4 * i).map(f32::from_bits))
                    .collect::<Result<_, _>>()?;
                let gain = |y: usize, x: usize, p: usize| {
                    let at = |pos: usize, len: usize, origin: f64, spacing: f64, points: usize| {
                        let t = (((pos as f64 + 0.5) / len as f64 - origin) / spacing)
                            .clamp(0.0, (points - 1) as f64);
                        let i = (t as usize).min(points.saturating_sub(2));
                        (i, (t - i as f64) as f32)
                    };
                    let (i, fy) = at(y, h, origin_v, spacing_v, points_v);
                    let (j, fx) = at(x, w, origin_h, spacing_h, points_h);
                    let p = p.min(map_planes - 1);
                    let g = |r: usize, c: usize| {
                        gains[(r.min(points_v - 1) * points_h + c.min(points_h - 1)) * map_planes
                            + p]
                    };
                    let top = g(i, j) + (g(i, j + 1) - g(i, j)) * fx;
                    let bottom = g(i + 1, j) + (g(i + 1, j + 1) - g(i + 1, j)) * fx;
                    top + (bottom - top) * fy
                };
                each(&|v, y, x, p| (v * gain(y, x, p)).clamp(0.0, 1.0));
            }
        }
    }
    Ok(())
}

/// The colors of a 2 × 2 Bayer pattern (0 red, 1 green, 2 blue).
fn cfa_pattern(tiff: &Tiff, raw: &Ifd) -> Result<[u8; 4], DngError> {
    let repeat = tiff.numbers(raw, CFA_REPEAT).unwrap_or_default();
    let pattern = tiff.numbers(raw, CFA_PATTERN).unwrap_or_default();
    let planes = tiff
        .numbers(raw, CFA_PLANE_COLOR)
        .unwrap_or_else(|| vec![0.0, 1.0, 2.0]);
    if repeat != [2.0, 2.0]
        || pattern.len() != 4
        || tiff.number(raw, CFA_LAYOUT).unwrap_or(1.0) != 1.0
    {
        return Err(Unsupported);
    }
    let mut out = [0u8; 4];
    for (o, &p) in out.iter_mut().zip(&pattern) {
        let color = *planes.get(p as usize).ok_or(Damaged)?;
        if !(0.0..=2.0).contains(&color) {
            return Err(Unsupported);
        }
        *o = color as u8;
    }
    let mut sorted = out;
    sorted.sort();
    if sorted != [0, 1, 1, 2] {
        return Err(Unsupported);
    }
    Ok(out)
}

/// Temperature of an EXIF light source (the calibration illuminants).
fn illuminant_temperature(code: u32) -> f64 {
    match code {
        17 => 2856.0,
        2 | 14 => 4150.0,
        3 => 2850.0,
        10 => 6500.0,
        11 => 7500.0,
        12 => 6430.0,
        13 => 5000.0,
        15 => 3450.0,
        18 => 4874.0,
        19 => 6774.0,
        20 => 5503.0,
        21 => 6504.0,
        22 => 7504.0,
        23 => 5003.0,
        24 => 3200.0,
        _ => 5500.0,
    }
}

fn matrix(values: &[f64]) -> Option<Mat3> {
    (values.len() == 9).then(|| std::array::from_fn(|i| std::array::from_fn(|j| values[i * 3 + j])))
}

/// The camera's color: calibrations, analog balance, white as shot.
fn camera(
    number: &dyn Fn(u16) -> Option<f64>,
    numbers: &dyn Fn(u16) -> Option<Vec<f64>>,
) -> Result<Camera, DngError> {
    let mut calibrations = Vec::new();
    for k in 0..2 {
        let Some(values) = numbers(COLOR_MATRIX[k]) else {
            continue;
        };
        // Three colors only (not the four-color cameras).
        let color_matrix = matrix(&values).ok_or(Unsupported)?;
        calibrations.push(Calibration {
            temperature: illuminant_temperature(number(ILLUMINANT[k]).unwrap_or(0.0) as u32),
            color_matrix,
            forward_matrix: numbers(FORWARD_MATRIX[k]).and_then(|v| matrix(&v)),
            camera_calibration: numbers(CAMERA_CALIBRATION[k])
                .and_then(|v| matrix(&v))
                .unwrap_or(white::IDENTITY),
        });
    }
    if calibrations.is_empty() {
        return Err(Unsupported);
    }
    let analog_balance = numbers(ANALOG_BALANCE)
        .filter(|v| v.len() == 3)
        .map_or([1.0; 3], |v| [v[0], v[1], v[2]]);
    let mut camera = Camera {
        calibrations,
        analog_balance,
        as_shot: [1.0; 3],
        baseline_exposure: number(BASELINE_EXPOSURE).unwrap_or(0.0) as f32,
    };
    camera.as_shot = match (numbers(AS_SHOT_NEUTRAL), numbers(AS_SHOT_WHITE_XY)) {
        (Some(n), _) if n.len() == 3 && n.iter().all(|&v| v > 0.0) => [n[0], n[1], n[2]],
        (_, Some(xy)) if xy.len() == 2 => camera.neutral_of([xy[0], xy[1]]),
        _ => camera.neutral_of(white::D65),
    };
    Ok(camera)
}

/// All the samples of the raw image (w × h × spp), from its strips or
/// tiles.
fn read_samples(
    tiff: &Tiff,
    raw: &Ifd,
    w: usize,
    h: usize,
    spp: usize,
    bits: u32,
) -> Result<Vec<u16>, DngError> {
    let number = |tag| tiff.number(raw, tag);
    let compression = number(COMPRESSION).unwrap_or(1.0) as u32;
    let predictor = number(PREDICTOR).unwrap_or(1.0) as u32;
    let (tw, th, offsets, counts) = match (number(TILE_WIDTH), number(TILE_HEIGHT)) {
        (Some(tw), Some(th)) => (
            tw as usize,
            th as usize,
            tiff.numbers(raw, TILE_OFFSETS).ok_or(Damaged)?,
            tiff.numbers(raw, TILE_COUNTS).ok_or(Damaged)?,
        ),
        _ => (
            w,
            number(ROWS_PER_STRIP).map_or(h, |r| (r as usize).clamp(1, h)),
            tiff.numbers(raw, STRIP_OFFSETS).ok_or(Damaged)?,
            tiff.numbers(raw, STRIP_COUNTS).ok_or(Damaged)?,
        ),
    };
    if tw == 0 || th == 0 {
        return Err(Damaged);
    }
    let across = w.div_ceil(tw);
    let tiles = across * h.div_ceil(th);
    if offsets.len() < tiles || counts.len() < tiles {
        return Err(Damaged);
    }
    let chunk = |i: usize| -> Result<&[u8], DngError> {
        let (at, len) = (offsets[i] as usize, counts[i] as usize);
        tiff.data.get(at..at.saturating_add(len)).ok_or(Damaged)
    };
    let decode_tile = |i: usize| -> Result<Vec<u16>, DngError> {
        let data = chunk(i)?;
        let n = tw * th * spp;
        let mut out = match compression {
            1 => unpack(data, tw * spp, th, bits, tiff.little),
            8 => {
                let mut bytes = Vec::new();
                flate2::read::ZlibDecoder::new(data)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Damaged)?;
                unpack(&bytes, tw * spp, th, bits, tiff.little)
            }
            7 => ljpeg::decode(data).ok_or(Damaged)?,
            34892 => {
                let image = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)
                    .map_err(|_| Damaged)?;
                let image = image.to_rgb8();
                let (jw, jh) = (image.width() as usize, image.height() as usize);
                // Rows of the tile, from the JPEG's (smaller at the edges).
                let mut out = vec![0u16; n];
                for y in 0..jh.min(th) {
                    for x in 0..jw.min(tw) {
                        let p = image.get_pixel(x as u32, y as u32).0;
                        for c in 0..spp {
                            out[(y * tw + x) * spp + c] = p[c.min(2)] as u16;
                        }
                    }
                }
                out
            }
            _ => return Err(Unsupported),
        };
        out.resize(n, 0);
        if predictor == 2 {
            for row in out.chunks_exact_mut(tw * spp) {
                for x in spp..row.len() {
                    row[x] = row[x].wrapping_add(row[x - spp]);
                }
            }
        } else if predictor != 1 {
            return Err(Unsupported);
        }
        Ok(out)
    };
    let decoded: Vec<Result<Vec<u16>, DngError>> = {
        let threads = crate::pixel_ops::threads().clamp(1, tiles.max(1));
        let per = tiles.div_ceil(threads);
        std::thread::scope(|s| {
            let handles: Vec<_> = (0..threads)
                .map(|t| {
                    let decode_tile = &decode_tile;
                    s.spawn(move || {
                        (t * per..((t + 1) * per).min(tiles))
                            .map(decode_tile)
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    };
    let mut samples = vec![0u16; w * h * spp];
    for (i, tile) in decoded.into_iter().enumerate() {
        let tile = tile?;
        let (x0, y0) = ((i % across) * tw, (i / across) * th);
        for y in 0..th.min(h - y0) {
            let n = tw.min(w - x0) * spp;
            let from = &tile[y * tw * spp..][..n];
            samples[((y0 + y) * w + x0) * spp..][..n].copy_from_slice(from);
        }
    }
    Ok(samples)
}

/// Uncompressed samples: 8 or 16 bits (in the file's byte order), or
/// packed most significant bit first, each row starting on a byte.
fn unpack(data: &[u8], per_row: usize, rows: usize, bits: u32, little: bool) -> Vec<u16> {
    let mut out = Vec::with_capacity(per_row * rows);
    match bits {
        8 => out.extend(data.iter().take(per_row * rows).map(|&b| b as u16)),
        16 => out.extend(data.chunks_exact(2).take(per_row * rows).map(|b| {
            if little {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            }
        })),
        _ => {
            let row_bytes = (per_row * bits as usize).div_ceil(8);
            for row in data.chunks(row_bytes).take(rows) {
                let (mut acc, mut n) = (0u32, 0u32);
                let mut bytes = row.iter();
                for _ in 0..per_row {
                    while n < bits {
                        acc = (acc << 8) | *bytes.next().unwrap_or(&0) as u32;
                        n += 8;
                    }
                    out.push(((acc >> (n - bits)) & ((1 << bits) - 1)) as u16);
                    n -= bits;
                }
            }
        }
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A tiny DNG writer for the tests: one IFD 0 with the color tags and
    /// a sub-IFD with the raw data.
    pub struct TestDng {
        pub width: u32,
        pub height: u32,
        /// Bayer samples (or RGB for linear raw).
        pub samples: Vec<u16>,
        pub linear: bool,
        pub lossless_jpeg: bool,
        pub black: u16,
        pub white: u16,
        pub pattern: [u8; 4],
        pub color_matrix: [f64; 9],
        pub as_shot: [f64; 3],
        pub orientation: u16,
    }

    impl TestDng {
        pub fn bytes(&self) -> Vec<u8> {
            // Entries: (tag, type, values as u32 or rational pairs).
            enum V {
                Short(Vec<u16>),
                Long(Vec<u32>),
                Byte(Vec<u8>),
                SRational(Vec<f64>),
                Rational(Vec<f64>),
            }
            let spp = if self.linear { 3 } else { 1 };
            let strip: Vec<u8> = if self.lossless_jpeg {
                // Two samples per JPEG pixel, as DNG writers do.
                let (w, h) = (self.width as usize, self.height as usize);
                ljpeg::tests::encode(&self.samples, w * spp / 2, h, 2, 16)
            } else {
                self.samples.iter().flat_map(|v| v.to_le_bytes()).collect()
            };
            let ifd0 = vec![
                (NEW_SUBFILE_TYPE, V::Long(vec![1])),
                (ORIENTATION, V::Short(vec![self.orientation])),
                (330, V::Long(vec![0])), // sub-IFD offset, patched below
                (DNG_VERSION, V::Byte(vec![1, 4, 0, 0])),
                (COLOR_MATRIX[0], V::SRational(self.color_matrix.to_vec())),
                (AS_SHOT_NEUTRAL, V::Rational(self.as_shot.to_vec())),
                (ILLUMINANT[0], V::Short(vec![21])),
            ];
            let mut raw = vec![
                (NEW_SUBFILE_TYPE, V::Long(vec![0])),
                (WIDTH, V::Long(vec![self.width])),
                (HEIGHT, V::Long(vec![self.height])),
                (BITS, V::Short(vec![16; spp])),
                (
                    COMPRESSION,
                    V::Short(vec![if self.lossless_jpeg { 7 } else { 1 }]),
                ),
                (
                    PHOTOMETRIC,
                    V::Short(vec![if self.linear { 34892 } else { 32803 }]),
                ),
                (STRIP_OFFSETS, V::Long(vec![0])), // patched below
                (SAMPLES, V::Short(vec![spp as u16])),
                (ROWS_PER_STRIP, V::Long(vec![self.height])),
                (STRIP_COUNTS, V::Long(vec![strip.len() as u32])),
                (BLACK_LEVEL, V::Short(vec![self.black])),
                (WHITE_LEVEL, V::Short(vec![self.white])),
            ];
            if !self.linear {
                raw.push((CFA_REPEAT, V::Short(vec![2, 2])));
                raw.push((CFA_PATTERN, V::Byte(self.pattern.to_vec())));
            }
            raw.sort_by_key(|e| e.0);
            let mut out = b"II*\0\x08\0\0\0".to_vec();
            let write_ifd = |out: &mut Vec<u8>, entries: &[(u16, V)]| -> Vec<(u16, usize)> {
                let start = out.len();
                let data_start = start + 2 + entries.len() * 12 + 4;
                let mut data: Vec<u8> = Vec::new();
                let mut places = Vec::new();
                out.extend((entries.len() as u16).to_le_bytes());
                for (tag, v) in entries {
                    let (kind, count, bytes): (u16, usize, Vec<u8>) = match v {
                        V::Short(s) => {
                            (3, s.len(), s.iter().flat_map(|x| x.to_le_bytes()).collect())
                        }
                        V::Long(s) => {
                            (4, s.len(), s.iter().flat_map(|x| x.to_le_bytes()).collect())
                        }
                        V::Byte(s) => (1, s.len(), s.clone()),
                        V::SRational(s) => (
                            10,
                            s.len(),
                            s.iter()
                                .flat_map(|x| {
                                    let n = (x * 10000.0).round() as i32;
                                    [n.to_le_bytes(), 10000i32.to_le_bytes()].concat()
                                })
                                .collect(),
                        ),
                        V::Rational(s) => (
                            5,
                            s.len(),
                            s.iter()
                                .flat_map(|x| {
                                    let n = (x * 10000.0).round() as u32;
                                    [n.to_le_bytes(), 10000u32.to_le_bytes()].concat()
                                })
                                .collect(),
                        ),
                    };
                    out.extend(tag.to_le_bytes());
                    out.extend(kind.to_le_bytes());
                    out.extend((count as u32).to_le_bytes());
                    if bytes.len() <= 4 {
                        places.push((*tag, out.len()));
                        let mut inline = bytes.clone();
                        inline.resize(4, 0);
                        out.extend(inline);
                    } else {
                        let at = data_start + data.len();
                        out.extend((at as u32).to_le_bytes());
                        data.extend(&bytes);
                        if data.len() % 2 == 1 {
                            data.push(0);
                        }
                    }
                }
                out.extend(0u32.to_le_bytes());
                out.extend(data);
                places
            };
            let places0 = write_ifd(&mut out, &ifd0);
            let sub_at = out.len();
            let places1 = write_ifd(&mut out, &raw);
            let strip_at = out.len();
            out.extend(&strip);
            let patch = |out: &mut Vec<u8>, places: &[(u16, usize)], tag: u16, v: usize| {
                let at = places.iter().find(|p| p.0 == tag).unwrap().1;
                out[at..at + 4].copy_from_slice(&(v as u32).to_le_bytes());
            };
            patch(&mut out, &places0, 330, sub_at);
            patch(&mut out, &places1, STRIP_OFFSETS, strip_at);
            out
        }
    }

    /// XYZ (D65) to linear sRGB: a "camera" seeing sRGB.
    pub const SRGB_CAMERA: [f64; 9] = [
        3.2404542, -1.5371385, -0.4985314, -0.9692660, 1.8760108, 0.0415560, 0.0556434, -0.2040259,
        1.0572252,
    ];

    /// A gray card filling the frame, lit by a warm light: raw values
    /// proportional to `as_shot`.
    fn gray_card(lossless_jpeg: bool, linear: bool) -> (TestDng, [f64; 3]) {
        let (w, h) = (16u32, 12u32);
        let neutral = [1.0, 0.8, 0.55];
        let level = |c: usize| (1024.0 + neutral[c] * 0.4 * 15000.0) as u16;
        let pattern = [0, 1, 1, 2];
        let samples: Vec<u16> = if linear {
            (0..w * h)
                .flat_map(|_| [level(0), level(1), level(2)])
                .collect()
        } else {
            (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    level(pattern[((y % 2) * 2 + x % 2) as usize] as usize)
                })
                .collect()
        };
        let dng = TestDng {
            width: w,
            height: h,
            samples,
            linear,
            lossless_jpeg,
            black: 1024,
            white: 16024,
            pattern,
            color_matrix: SRGB_CAMERA,
            as_shot: [0.5, 1.0, 0.8],
            orientation: 1,
        };
        (dng, neutral)
    }

    #[test]
    fn a_tiny_dng_decodes_in_every_variant() {
        for (lossless, linear) in [(false, false), (true, false), (false, true)] {
            let (dng, neutral) = gray_card(lossless, linear);
            let source = decode(&dng.bytes()).expect("decoded");
            assert_eq!((source.width, source.height), (16, 12));
            // Levels between black and white, multiplied by the white
            // balance as shot (1 / neutral, the largest at 1).
            let gains = [2.0, 1.0, 1.25];
            let p = source.rgb[5 * 16 + 7];
            for c in 0..3 {
                let expected = (neutral[c] * 0.4 * gains[c]) as f32;
                assert!((p[c] - expected).abs() < 2e-3, "{lossless} {linear}: {p:?}");
            }
            let camera = source.camera.as_ref().unwrap();
            assert_eq!(camera.as_shot, [0.5, 1.0, 0.8]);
        }
    }

    #[test]
    fn opcodes_map_the_levels_and_correct_the_shading() {
        let area = |plane: u32| {
            [0u32, 0, 4, 4, plane, 1, 1, 1]
                .map(u32::to_be_bytes)
                .concat()
        };
        let op = |id: u32, params: Vec<u8>| {
            [
                &id.to_be_bytes()[..],
                &[0, 1, 3, 0],
                &0u32.to_be_bytes(),
                &(params.len() as u32).to_be_bytes(),
                &params,
            ]
            .concat()
        };
        // MapPolynomial on plane 0: y = x²; a gain map (2 × 2 points,
        // ×1 on the left, ×2 on the right) on plane 1.
        let mut poly = area(0);
        poly.extend(2u32.to_be_bytes());
        for c in [0.0f64, 0.0, 1.0] {
            poly.extend(c.to_be_bytes());
        }
        let mut gain = area(1);
        gain.extend(2u32.to_be_bytes());
        gain.extend(2u32.to_be_bytes());
        for v in [1.0f64, 1.0, 0.0, 0.0] {
            gain.extend(v.to_be_bytes());
        }
        gain.extend(1u32.to_be_bytes());
        for g in [1.0f32, 2.0, 1.0, 2.0] {
            gain.extend(g.to_bits().to_be_bytes());
        }
        let list = [&2u32.to_be_bytes()[..], &op(8, poly), &op(9, gain)].concat();
        let mut data = vec![0.4f32; 4 * 4 * 2];
        apply_opcodes(&list, &mut data, 4, 4, 2).unwrap();
        assert!((data[0] - 0.16).abs() < 1e-6, "squared");
        let (left, right) = (data[1], data[(3 * 4 + 3) * 2 + 1]);
        assert!(left < right && right > 0.7, "{left} {right}");
        // A required opcode this decoder does not know: unsupported.
        let unknown = [&1u32.to_be_bytes()[..], &op(42, area(0))].concat();
        assert_eq!(
            apply_opcodes(&unknown, &mut data, 4, 4, 2),
            Err(Unsupported)
        );
    }

    #[test]
    fn orientation_and_damage() {
        let (mut dng, _) = gray_card(false, false);
        dng.orientation = 6;
        let source = decode(&dng.bytes()).unwrap();
        assert_eq!((source.width, source.height), (12, 16));
        let bytes = dng.bytes();
        assert_eq!(decode(&bytes[..bytes.len() - 10]).unwrap_err(), Damaged);
        assert!(is_dng(&bytes));
        assert!(!is_dng(b"II*\0\x08\0\0\0\0\0"));
    }
}
