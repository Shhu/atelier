//! Vector masks (and shape layers' outlines), rasterized into masks: Atelier
//! has no vector masks of its own.

use image::GrayImage;
use zeno::{Command, Fill, Mask, Vector};

use super::bytes::{Reader, Result};

/// A subpath: closed or not, its knots as (preceding control, anchor,
/// leaving control) points in canvas pixels.
struct Subpath {
    closed: bool,
    knots: Vec<[Vector; 3]>,
}

/// Path components: an operation (0 exclude, 1 combine, 2 subtract, 3
/// intersect) and the subpaths filled together by the even-odd rule.
type Component = (i16, Vec<Subpath>);

/// Coverage over the canvas of the vector mask `data` ("vmsk" / "vsms"
/// block), `None` if it is disabled.
pub fn rasterize(data: &[u8], (w, h): (u32, u32)) -> Result<Option<GrayImage>> {
    let mut r = Reader::new(data);
    r.u32()?;
    let flags = r.u32()?;
    if flags & 4 != 0 {
        return Ok(None);
    }
    let mut components: Vec<Component> = Vec::new();
    let mut reveal = false;
    // 8.24 fixed point, fractions of the canvas size.
    let fixed = |v: i32, size: u32| v as f32 / (1 << 24) as f32 * size as f32;
    while r.remaining() >= 26 {
        let mut rec = r.sub(26)?;
        match rec.u16()? {
            selector @ (0 | 3) => {
                rec.u16()?;
                let op = rec.i16()?;
                let path = Subpath {
                    closed: selector == 0,
                    knots: Vec::new(),
                };
                match components.last_mut() {
                    Some((_, paths)) if op == -1 => paths.push(path),
                    _ => components.push((op.max(0), vec![path])),
                }
            }
            1 | 2 | 4 | 5 => {
                let mut point = || -> Result<Vector> {
                    let y = rec.i32()?;
                    let x = rec.i32()?;
                    Ok(Vector::new(fixed(x, w), fixed(y, h)))
                };
                let knot = [point()?, point()?, point()?];
                if let Some(path) = components.last_mut().and_then(|c| c.1.last_mut()) {
                    path.knots.push(knot);
                }
            }
            8 => reveal = rec.u16()? != 0,
            _ => {}
        }
    }
    let n = w as usize * h as usize;
    let mut mask = vec![
        if reveal && components.is_empty() {
            1.0f32
        } else {
            0.0
        };
        n
    ];
    for (i, (op, paths)) in components.iter().enumerate() {
        let plane = fill(paths, (w, h));
        // Subtracting or intersecting first: from everything.
        if i == 0 && matches!(op, 2 | 3) {
            mask.iter_mut().for_each(|m| *m = 1.0 - *m);
        }
        for (m, &p) in mask.iter_mut().zip(&plane) {
            let p = p as f32 / 255.0;
            *m = match op {
                0 => *m + p - 2.0 * *m * p,
                2 => (*m - p).max(0.0),
                3 => *m * p,
                _ => *m + p - *m * p,
            };
        }
    }
    let invert = flags & 1 != 0;
    let bytes = mask
        .into_iter()
        .map(|m| {
            let m = if invert { 1.0 - m } else { m };
            (m.clamp(0.0, 1.0) * 255.0).round() as u8
        })
        .collect();
    Ok(GrayImage::from_raw(w, h, bytes))
}

/// The subpaths filled by the even-odd rule over the canvas.
fn fill(paths: &[Subpath], (w, h): (u32, u32)) -> Vec<u8> {
    let mut commands = Vec::new();
    for path in paths.iter().filter(|p| p.knots.len() >= 2) {
        let k = &path.knots;
        commands.push(Command::MoveTo(k[0][1]));
        for pair in k.windows(2) {
            commands.push(Command::CurveTo(pair[0][2], pair[1][0], pair[1][1]));
        }
        if path.closed {
            let (last, first) = (&k[k.len() - 1], &k[0]);
            commands.push(Command::CurveTo(last[2], first[0], first[1]));
            commands.push(Command::Close);
        }
    }
    if commands.is_empty() {
        return vec![0; w as usize * h as usize];
    }
    Mask::new(&commands[..])
        .style(Fill::EvenOdd)
        .size(w, h)
        .render()
        .0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vector mask block: a closed square path, its corners in fractions
    /// of the canvas.
    fn square(op: i16, [x0, y0, x1, y1]: [f64; 4], flags: u32) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&3u32.to_be_bytes());
        out.extend_from_slice(&flags.to_be_bytes());
        let record = |out: &mut Vec<u8>, selector: u16, body: &[u8]| {
            out.extend_from_slice(&selector.to_be_bytes());
            let mut b = body.to_vec();
            b.resize(24, 0);
            out.extend_from_slice(&b);
        };
        record(&mut out, 6, &[]);
        let mut len = 4u16.to_be_bytes().to_vec();
        len.extend_from_slice(&op.to_be_bytes());
        record(&mut out, 0, &len);
        let fix = |v: f64| ((v * (1 << 24) as f64) as i32).to_be_bytes();
        for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
            let mut knot = Vec::new();
            for _ in 0..3 {
                knot.extend_from_slice(&fix(y));
                knot.extend_from_slice(&fix(x));
            }
            record(&mut out, 2, &knot);
        }
        out
    }

    #[test]
    fn square_paths_fill_their_area() {
        let block = square(1, [0.25, 0.25, 0.75, 0.75], 0);
        let m = rasterize(&block, (8, 8)).unwrap().unwrap();
        assert_eq!(m.get_pixel(4, 4).0[0], 255);
        assert_eq!(m.get_pixel(0, 0).0[0], 0);
        assert_eq!(m.get_pixel(1, 4).0[0], 0);
        // Inverted; disabled.
        let inverted = rasterize(&square(1, [0.25, 0.25, 0.75, 0.75], 1), (8, 8)).unwrap();
        assert_eq!(inverted.unwrap().get_pixel(0, 0).0[0], 255);
        assert!(
            rasterize(&square(1, [0.0, 0.0, 1.0, 1.0], 4), (8, 8))
                .unwrap()
                .is_none()
        );
        // Subtracting first: from everything.
        let hole = rasterize(&square(2, [0.25, 0.25, 0.75, 0.75], 0), (8, 8))
            .unwrap()
            .unwrap();
        assert_eq!(
            (hole.get_pixel(4, 4).0[0], hole.get_pixel(0, 0).0[0]),
            (0, 255)
        );
    }
}
