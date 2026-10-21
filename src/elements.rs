//! Finding the "elements" of an image: groups of nearby visible pixels.

use image::RgbaImage;

/// Result of the grouping.
pub struct Groups {
    /// For each pixel: 0 if transparent, else the group number (starting at 1).
    pub labels: Vec<u32>,
    /// Box (x, y, width, height) of the visible pixels of each group;
    /// group `n` is at `boxes[n - 1]`.
    pub boxes: Vec<[u32; 4]>,
}

/// Groups the visible pixels (alpha > 0): two pixels are in the same group
/// if they are linked by a chain of visible pixels separated by at most `gap`
/// transparent pixels (0 = they must touch, diagonals included).
pub fn group_pixels(image: &RgbaImage, gap: u32) -> Groups {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let mask: Vec<bool> = image.pixels().map(|p| p.0[3] > 0).collect();

    // Grow each pixel by ⌈gap/2⌉: two pixels separated by `gap` empty
    // pixels then touch, and the connected components of this grown mask
    // define the groups.
    let radius = gap.div_ceil(2) as usize;
    let grown = if radius == 0 {
        mask.clone()
    } else {
        dilate(&mask, w, h, radius)
    };

    let mut grown_labels = vec![0u32; w * h];
    let mut count = 0;
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !grown[start] || grown_labels[start] != 0 {
            continue;
        }
        count += 1;
        grown_labels[start] = count;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let j = ny * w + nx;
                    if grown[j] && grown_labels[j] == 0 {
                        grown_labels[j] = count;
                        stack.push(j);
                    }
                }
            }
        }
    }

    // Each visible pixel takes the group of its grown area.
    let mut labels = vec![0u32; w * h];
    let mut bounds = vec![[u32::MAX, u32::MAX, 0, 0]; count as usize];
    for i in 0..w * h {
        if mask[i] {
            let l = grown_labels[i];
            labels[i] = l;
            let b = &mut bounds[(l - 1) as usize];
            let (x, y) = ((i % w) as u32, (i / w) as u32);
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x);
            b[3] = b[3].max(y);
        }
    }
    let boxes = bounds
        .into_iter()
        .map(|[x0, y0, x1, y1]| [x0, y0, x1 - x0 + 1, y1 - y0 + 1])
        .collect();
    Groups { labels, boxes }
}

/// Square dilation of radius `r` (separable: rows then columns).
fn dilate(mask: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    // True if at least one element of the window [i - r, i + r] is true.
    fn pass(src: &[bool], n: usize, r: usize, get: impl Fn(usize) -> usize, out: &mut [bool]) {
        let mut prefix = vec![0u32; n + 1];
        for i in 0..n {
            prefix[i + 1] = prefix[i] + src[get(i)] as u32;
        }
        for i in 0..n {
            let (a, b) = (i.saturating_sub(r), (i + r + 1).min(n));
            out[get(i)] = prefix[b] > prefix[a];
        }
    }

    let mut rows = vec![false; w * h];
    for y in 0..h {
        pass(mask, w, r, |x| y * w + x, &mut rows);
    }
    let mut out = vec![false; w * h];
    for x in 0..w {
        pass(&rows, h, r, |y| y * w + x, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn image_with(w: u32, h: u32, pixels: &[(u32, u32)]) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for &(x, y) in pixels {
            img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
        }
        img
    }

    #[test]
    fn separate_blobs_make_separate_groups() {
        let img = image_with(20, 5, &[(1, 1), (2, 1), (15, 3)]);
        let g = group_pixels(&img, 0);
        assert_eq!(g.boxes, vec![[1, 1, 2, 1], [15, 3, 1, 1]]);
        assert_eq!(g.labels[20 + 2], 1);
        assert_eq!(g.labels[3 * 20 + 15], 2);
        assert_eq!(g.labels[0], 0);
    }

    #[test]
    fn diagonal_pixels_touch() {
        let img = image_with(5, 5, &[(1, 1), (2, 2), (3, 3)]);
        assert_eq!(group_pixels(&img, 0).boxes.len(), 1);
    }

    #[test]
    fn gap_merges_close_groups_exactly() {
        // 3 empty pixels between the two points.
        let img = image_with(10, 1, &[(1, 0), (5, 0)]);
        assert_eq!(group_pixels(&img, 2).boxes.len(), 2);
        assert_eq!(group_pixels(&img, 3).boxes, vec![[1, 0, 5, 1]]);
    }

    #[test]
    fn gap_works_vertically_too() {
        let img = image_with(1, 10, &[(0, 0), (0, 4)]);
        assert_eq!(group_pixels(&img, 2).boxes.len(), 2);
        assert_eq!(group_pixels(&img, 3).boxes.len(), 1);
    }

    #[test]
    fn transparent_image_has_no_group() {
        let g = group_pixels(&RgbaImage::new(4, 4), 5);
        assert!(g.boxes.is_empty());
        assert!(g.labels.iter().all(|&l| l == 0));
    }
}
