//! "Select Subject" without AI: GrabCut (Rother et al., 2004).
//!
//! The image border and transparent pixels are background; the rest is
//! first assumed to be the subject. Then we alternate: color models
//! (Gaussian mixtures) of the subject and the background, then a graph cut
//! that picks the most likely side for each pixel while keeping a boundary
//! that follows the image's edges.

use image::{GrayImage, Luma, RgbaImage, imageops::FilterType};

/// Side of the downscaled image the computation runs on (the result is then
/// upscaled to the original size).
const WORK_SIZE: u32 = 480;
/// Maximum side of the image on which the edge is refined.
const REFINE_SIZE: u32 = 2048;
/// Subject smaller than this fraction of the image: nothing clear was found.
const MIN_SUBJECT: f32 = 0.005;
/// Components per Gaussian mixture.
const COMPONENTS: usize = 5;
const ITERATIONS: usize = 4;
/// Weight of the boundary smoothness (value from the paper).
const GAMMA: f32 = 50.0;
const INFINITE: f32 = 1e9;

/// Coverage (0..255) of the main subject of `image`, at its size.
pub fn select_subject(image: &RgbaImage) -> GrayImage {
    let (w0, h0) = image.dimensions();
    let scale = (WORK_SIZE as f32 / w0.max(h0) as f32).min(1.0);
    let (w, h) = (
        ((w0 as f32 * scale).round() as u32).max(1),
        ((h0 as f32 * scale).round() as u32).max(1),
    );
    let small = if scale < 1.0 {
        image::imageops::resize(image, w, h, FilterType::Triangle)
    } else {
        image.clone()
    };
    let (w, h) = (w as usize, h as usize);
    let colors: Vec<[f32; 3]> = small
        .pixels()
        .map(|p| [p.0[0] as f32, p.0[1] as f32, p.0[2] as f32])
        .collect();

    // Certain background: the border and transparency.
    let border = ((w.min(h) as f32 * 0.04).round() as usize).max(1);
    let fixed_bg: Vec<bool> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let edge = x < border || y < border || x >= w - border || y >= h - border;
            edge || small.as_raw()[i * 4 + 3] < 128
        })
        .collect();
    let mut fg: Vec<bool> = fixed_bg.iter().map(|&b| !b).collect();
    if fg.iter().all(|&f| !f) {
        return GrayImage::new(w0, h0);
    }

    let links = Links::new(&colors, w, h);
    for _ in 0..ITERATIONS {
        let fg_model = Gmm::fit(&colors, &fg, true);
        let bg_model = Gmm::fit(&colors, &fg, false);
        let (Some(fg_model), Some(bg_model)) = (fg_model, bg_model) else {
            break;
        };
        let mut graph = Graph::new(w * h);
        for i in 0..w * h {
            if fixed_bg[i] {
                graph.set_terminals(i, 0.0, INFINITE);
            } else {
                // Cost of labelling "background" on the source side, "subject" on the sink side.
                let c = colors[i];
                graph.set_terminals(i, bg_model.cost(c), fg_model.cost(c));
            }
        }
        links.add_to(&mut graph);
        graph.max_flow();
        let changed = (0..w * h)
            .filter(|&i| !fixed_bg[i])
            .fold(false, |changed, i| {
                let side = graph.on_source_side(i);
                let differs = side != fg[i];
                fg[i] = side;
                changed || differs
            });
        if !changed {
            break;
        }
    }
    let largest = keep_main_regions(&mut fg, w, h);
    if (largest as f32) < (w * h) as f32 * MIN_SUBJECT {
        return GrayImage::new(w0, h0);
    }

    let mask = GrayImage::from_fn(w as u32, h as u32, |x, y| {
        Luma([if fg[y as usize * w + x as usize] {
            255
        } else {
            0
        }])
    });
    // Edge refined on a larger image: the upscaled mask follows the image's
    // edges thanks to a guided filter.
    let fine = (REFINE_SIZE as f32 / w0.max(h0) as f32).min(1.0);
    let (rw, rh) = (
        ((w0 as f32 * fine).round() as u32).max(1),
        ((h0 as f32 * fine).round() as u32).max(1),
    );
    let guide = if fine < 1.0 {
        image::imageops::resize(image, rw, rh, FilterType::Triangle)
    } else {
        image.clone()
    };
    let coarse = image::imageops::resize(&mask, rw, rh, FilterType::Triangle);
    let radius = ((rw as f32 / w as f32) * 1.5).round().max(2.0) as usize;
    let refined = refine_edges(&guide, &coarse, radius);
    if (rw, rh) == (w0, h0) {
        refined
    } else {
        image::imageops::resize(&refined, w0, h0, FilterType::Triangle)
    }
}

/// Guided filter (He et al.): the mask locally takes the form of an affine
/// function of the image's luminance, which realigns its edge onto the
/// image edges. Then a tightened ramp for a soft but crisp edge.
fn refine_edges(guide: &RgbaImage, mask: &GrayImage, radius: usize) -> GrayImage {
    const EPS: f32 = 1e-3;
    let (w, h) = (mask.width() as usize, mask.height() as usize);
    let i: Vec<f32> = guide
        .pixels()
        .map(|p| (0.299 * p.0[0] as f32 + 0.587 * p.0[1] as f32 + 0.114 * p.0[2] as f32) / 255.0)
        .collect();
    let p: Vec<f32> = mask.pixels().map(|m| m.0[0] as f32 / 255.0).collect();
    let product = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).collect::<Vec<_>>();
    let mean_i = box_mean(&i, w, h, radius);
    let mean_p = box_mean(&p, w, h, radius);
    let corr_ip = box_mean(&product(&i, &p), w, h, radius);
    let corr_ii = box_mean(&product(&i, &i), w, h, radius);
    let mut a = vec![0.0f32; w * h];
    let mut b = vec![0.0f32; w * h];
    for k in 0..w * h {
        let var = corr_ii[k] - mean_i[k] * mean_i[k];
        let cov = corr_ip[k] - mean_i[k] * mean_p[k];
        a[k] = cov / (var + EPS);
        b[k] = mean_p[k] - a[k] * mean_i[k];
    }
    let (mean_a, mean_b) = (box_mean(&a, w, h, radius), box_mean(&b, w, h, radius));
    GrayImage::from_fn(w as u32, h as u32, |x, y| {
        let k = y as usize * w + x as usize;
        let q = mean_a[k] * i[k] + mean_b[k];
        Luma([(((q - 0.5) * 1.6 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8])
    })
}

/// Mean over a square of side 2 `radius` + 1 (clamped at the borders), via
/// an integral image.
fn box_mean(src: &[f32], w: usize, h: usize, radius: usize) -> Vec<f32> {
    let mut integral = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += src[y * w + x] as f64;
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(radius), (y + radius + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(radius), (x + radius + 1).min(w));
            let sum = integral[y1 * (w + 1) + x1]
                - integral[y0 * (w + 1) + x1]
                - integral[y1 * (w + 1) + x0]
                + integral[y0 * (w + 1) + x0];
            out[y * w + x] = (sum / ((x1 - x0) * (y1 - y0)) as f64) as f32;
        }
    }
    out
}

/// Keeps only the subject regions of at least 5% of the largest one (removes
/// isolated islands). Returns the size of the largest.
fn keep_main_regions(fg: &mut [bool], w: usize, h: usize) -> usize {
    let mut label = vec![0u32; w * h];
    let mut sizes = vec![0usize];
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !fg[start] || label[start] != 0 {
            continue;
        }
        let id = sizes.len() as u32;
        let mut size = 0;
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = (i % w, i / w);
            let neighbours = [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ];
            for j in neighbours.into_iter().flatten() {
                if fg[j] && label[j] == 0 {
                    label[j] = id;
                    stack.push(j);
                }
            }
        }
        sizes.push(size);
    }
    let largest = sizes.iter().copied().max().unwrap_or(0);
    for i in 0..w * h {
        if fg[i] && sizes[label[i] as usize] * 20 < largest {
            fg[i] = false;
        }
    }
    largest
}

/// Links between neighbours (8-connectivity): strong between close colors,
/// weak across an edge, so that the cut follows the edges.
struct Links {
    /// (pixel, neighbour, weight), each pair only once.
    pairs: Vec<(u32, u32, f32)>,
}

impl Links {
    fn new(colors: &[[f32; 3]], w: usize, h: usize) -> Self {
        let offsets: [(isize, isize, f32); 4] = [
            (1, 0, 1.0),
            (0, 1, 1.0),
            (1, 1, std::f32::consts::FRAC_1_SQRT_2),
            (-1, 1, std::f32::consts::FRAC_1_SQRT_2),
        ];
        let dist2 = |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>();
        let mut pairs = Vec::with_capacity(w * h * 4);
        let mut total = 0.0f64;
        for y in 0..h {
            for x in 0..w {
                for &(dx, dy, _) in &offsets {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let (i, j) = (y * w + x, ny as usize * w + nx as usize);
                    let d = dist2(colors[i], colors[j]);
                    total += d as f64;
                    pairs.push((i as u32, j as u32, d));
                }
            }
        }
        let mean = (total / pairs.len().max(1) as f64) as f32;
        let beta = if mean > 0.0 { 1.0 / (2.0 * mean) } else { 0.0 };
        let mut k = 0;
        for y in 0..h {
            for x in 0..w {
                for &(dx, dy, weight) in &offsets {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if nx < 0 || nx >= w as isize || ny >= h as isize {
                        continue;
                    }
                    let d = pairs[k].2;
                    pairs[k].2 = GAMMA * weight * (-beta * d).exp();
                    k += 1;
                }
            }
        }
        Self { pairs }
    }

    fn add_to(&self, graph: &mut Graph) {
        graph.set_edges(&self.pairs);
    }
}

/// Gaussian mixture (full covariance) over RGB colors.
struct Gmm {
    /// (log of weight − ½ log det, mean, inverse of the covariance).
    components: Vec<(f32, [f32; 3], [[f32; 3]; 3])>,
}

impl Gmm {
    /// Fits a mixture on the pixels whose `fg` equals `side` (k-means then
    /// one Gaussian per cluster). `None` if there are no pixels.
    fn fit(colors: &[[f32; 3]], fg: &[bool], side: bool) -> Option<Gmm> {
        let samples: Vec<[f32; 3]> = colors
            .iter()
            .zip(fg)
            .filter(|(_, f)| **f == side)
            .map(|(c, _)| *c)
            .collect();
        if samples.is_empty() {
            return None;
        }
        let k = COMPONENTS.min(samples.len());
        // Initial centers spread in order of brightness.
        let mut sorted: Vec<&[f32; 3]> = samples.iter().collect();
        sorted.sort_by(|a, b| (a[0] + a[1] + a[2]).total_cmp(&(b[0] + b[1] + b[2])));
        let mut centers: Vec<[f32; 3]> = (0..k)
            .map(|c| *sorted[(c * 2 + 1) * sorted.len() / (2 * k)])
            .collect();
        let mut assign = vec![0usize; samples.len()];
        for _ in 0..6 {
            for (a, s) in assign.iter_mut().zip(&samples) {
                *a = nearest(&centers, *s);
            }
            let mut sums = vec![([0.0f64; 3], 0usize); k];
            for (a, s) in assign.iter().zip(&samples) {
                for (sum, v) in sums[*a].0.iter_mut().zip(s) {
                    *sum += *v as f64;
                }
                sums[*a].1 += 1;
            }
            for (c, (sum, n)) in centers.iter_mut().zip(&sums) {
                if *n > 0 {
                    *c = sum.map(|v| (v / *n as f64) as f32);
                }
            }
        }
        let total = samples.len() as f32;
        let components = (0..k)
            .filter_map(|c| {
                let members: Vec<&[f32; 3]> = assign
                    .iter()
                    .zip(&samples)
                    .filter(|(a, _)| **a == c)
                    .map(|(_, s)| s)
                    .collect();
                if members.is_empty() {
                    return None;
                }
                let n = members.len() as f32;
                let mean = centers[c];
                let mut cov = [[0.0f32; 3]; 3];
                for s in &members {
                    for i in 0..3 {
                        for j in 0..3 {
                            cov[i][j] += (s[i] - mean[i]) * (s[j] - mean[j]) / n;
                        }
                    }
                }
                // A little noise to stay invertible (flat colors).
                for (i, row) in cov.iter_mut().enumerate() {
                    row[i] += 4.0;
                }
                let (det, inv) = invert(cov)?;
                Some(((n / total).ln() - 0.5 * det.ln(), mean, inv))
            })
            .collect();
        Some(Gmm { components })
    }

    /// −log of a color's likelihood (membership cost).
    fn cost(&self, c: [f32; 3]) -> f32 {
        // Log-sum-exp over the components.
        let logs: Vec<f32> = self
            .components
            .iter()
            .map(|(w, mean, inv)| {
                let d = [c[0] - mean[0], c[1] - mean[1], c[2] - mean[2]];
                let mut q = 0.0;
                for i in 0..3 {
                    for j in 0..3 {
                        q += d[i] * inv[i][j] * d[j];
                    }
                }
                w - 0.5 * q
            })
            .collect();
        let max = logs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let sum: f32 = logs.iter().map(|l| (l - max).exp()).sum();
        (-(max + sum.ln())).clamp(0.0, 1e4)
    }
}

fn nearest(centers: &[[f32; 3]], s: [f32; 3]) -> usize {
    let d = |c: &[f32; 3]| (0..3).map(|i| (c[i] - s[i]).powi(2)).sum::<f32>();
    (0..centers.len())
        .min_by(|&a, &b| d(&centers[a]).total_cmp(&d(&centers[b])))
        .unwrap_or(0)
}

/// Determinant and inverse of a 3 × 3 matrix.
fn invert(m: [[f32; 3]; 3]) -> Option<(f32, [[f32; 3]; 3])> {
    let c =
        |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    let det = m[0][0] * c(1, 1, 2, 2) - m[0][1] * c(1, 0, 2, 2) + m[0][2] * c(1, 0, 2, 1);
    if det.abs() < 1e-6 {
        return None;
    }
    let inv = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ]
    .map(|row| row.map(|v| v / det));
    Some((det, inv))
}

const NONE: u32 = u32::MAX;
const TERMINAL: u32 = u32::MAX - 1;
const FREE: u8 = 0;
const SOURCE: u8 = 1;
const SINK: u8 = 2;

/// Graph for the minimum cut, solved by the Boykov-Kolmogorov algorithm
/// (two search trees reused from one augmentation to the next), very
/// efficient on image grids.
struct Graph {
    /// Arcs of each node: `first[i]..first[i + 1]` (compact format).
    first: Vec<u32>,
    head: Vec<u32>,
    /// Opposite arc (same link, in the other direction).
    sister: Vec<u32>,
    /// Residual capacity.
    cap: Vec<f32>,
    /// Residual capacity to the source (> 0) or the sink (< 0).
    terminal: Vec<f32>,
    tree: Vec<u8>,
    /// Arc to the parent in the tree, `TERMINAL` at the root, `NONE` otherwise.
    parent: Vec<u32>,
    /// Timestamp and distance to the root (faster adoption).
    stamp: Vec<u32>,
    dist: Vec<u32>,
    active: Vec<bool>,
}

impl Graph {
    fn new(n: usize) -> Self {
        Self {
            first: vec![0; n + 1],
            head: Vec::new(),
            sister: Vec::new(),
            cap: Vec::new(),
            terminal: vec![0.0; n],
            tree: vec![FREE; n],
            parent: vec![NONE; n],
            stamp: vec![0; n],
            dist: vec![0; n],
            active: vec![false; n],
        }
    }

    /// Capacities to the source and to the sink of node `i`.
    fn set_terminals(&mut self, i: usize, source: f32, sink: f32) {
        self.terminal[i] = source - sink;
    }

    /// Undirected links (i, j, weight), set once and for all.
    fn set_edges(&mut self, pairs: &[(u32, u32, f32)]) {
        let n = self.terminal.len();
        let mut degree = vec![0u32; n + 1];
        for &(i, j, _) in pairs {
            degree[i as usize] += 1;
            degree[j as usize] += 1;
        }
        let mut first = vec![0u32; n + 1];
        for i in 0..n {
            first[i + 1] = first[i] + degree[i];
        }
        let m = first[n] as usize;
        let (mut head, mut sister, mut cap) = (vec![0u32; m], vec![0u32; m], vec![0.0f32; m]);
        let mut next = first.clone();
        for &(i, j, w) in pairs {
            let (a, b) = (next[i as usize], next[j as usize]);
            next[i as usize] += 1;
            next[j as usize] += 1;
            (head[a as usize], head[b as usize]) = (j, i);
            (sister[a as usize], sister[b as usize]) = (b, a);
            (cap[a as usize], cap[b as usize]) = (w, w);
        }
        (self.first, self.head, self.sister, self.cap) = (first, head, sister, cap);
    }

    fn arcs(&self, i: usize) -> std::ops::Range<usize> {
        self.first[i] as usize..self.first[i + 1] as usize
    }

    fn on_source_side(&self, i: usize) -> bool {
        self.tree[i] == SOURCE
    }

    fn max_flow(&mut self) {
        let n = self.terminal.len();
        let mut queue = std::collections::VecDeque::new();
        for i in 0..n {
            if self.terminal[i] != 0.0 {
                self.tree[i] = if self.terminal[i] > 0.0 { SOURCE } else { SINK };
                self.parent[i] = TERMINAL;
                self.active[i] = true;
                queue.push_back(i);
            }
        }
        let mut orphans = Vec::new();
        let mut time = 0u32;
        loop {
            // Growth: until the two trees touch.
            let mut meeting = None;
            while let Some(&p) = queue.front() {
                if self.tree[p] == FREE {
                    self.active[p] = false;
                    queue.pop_front();
                    continue;
                }
                let from_source = self.tree[p] == SOURCE;
                for a in self.arcs(p) {
                    let residual = if from_source {
                        self.cap[a]
                    } else {
                        self.cap[self.sister[a] as usize]
                    };
                    if residual <= 0.0 {
                        continue;
                    }
                    let q = self.head[a] as usize;
                    if self.tree[q] == FREE {
                        self.tree[q] = self.tree[p];
                        self.parent[q] = self.sister[a];
                        self.stamp[q] = self.stamp[p];
                        self.dist[q] = self.dist[p] + 1;
                        if !self.active[q] {
                            self.active[q] = true;
                            queue.push_back(q);
                        }
                    } else if self.tree[q] != self.tree[p] {
                        // Arc from the source side to the sink side.
                        meeting = Some(if from_source {
                            a
                        } else {
                            self.sister[a] as usize
                        });
                        break;
                    }
                }
                if meeting.is_some() {
                    break;
                }
                self.active[p] = false;
                queue.pop_front();
            }
            let Some(middle) = meeting else { break };

            // Augmentation along the path found.
            time += 1;
            self.augment(middle, &mut orphans);

            // Adoption of orphans.
            while let Some(o) = orphans.pop() {
                self.adopt(o, time, &mut queue, &mut orphans);
            }
        }
    }

    fn augment(&mut self, middle: usize, orphans: &mut Vec<usize>) {
        let s_start = self.head[self.sister[middle] as usize] as usize;
        let t_start = self.head[middle] as usize;
        // Bottleneck.
        let mut flow = self.cap[middle];
        let mut i = s_start;
        while self.parent[i] != TERMINAL {
            let a = self.parent[i] as usize;
            flow = flow.min(self.cap[self.sister[a] as usize]);
            i = self.head[a] as usize;
        }
        flow = flow.min(self.terminal[i]);
        let mut i = t_start;
        while self.parent[i] != TERMINAL {
            let a = self.parent[i] as usize;
            flow = flow.min(self.cap[a]);
            i = self.head[a] as usize;
        }
        flow = flow.min(-self.terminal[i]);

        let sister = self.sister[middle] as usize;
        self.cap[middle] -= flow;
        self.cap[sister] += flow;
        // Source side: the flow goes down from the root to the meeting point.
        let mut i = s_start;
        while self.parent[i] != TERMINAL {
            let a = self.parent[i] as usize;
            let down = self.sister[a] as usize;
            self.cap[down] -= flow;
            self.cap[a] += flow;
            let next = self.head[a] as usize;
            if self.cap[down] <= 1e-6 {
                self.cap[down] = 0.0;
                self.parent[i] = NONE;
                orphans.push(i);
            }
            i = next;
        }
        self.terminal[i] -= flow;
        if self.terminal[i] <= 1e-6 {
            self.terminal[i] = 0.0;
            self.parent[i] = NONE;
            orphans.push(i);
        }
        // Sink side: the flow goes back up to the root.
        let mut i = t_start;
        while self.parent[i] != TERMINAL {
            let a = self.parent[i] as usize;
            self.cap[a] -= flow;
            self.cap[self.sister[a] as usize] += flow;
            let next = self.head[a] as usize;
            if self.cap[a] <= 1e-6 {
                self.cap[a] = 0.0;
                self.parent[i] = NONE;
                orphans.push(i);
            }
            i = next;
        }
        self.terminal[i] += flow;
        if self.terminal[i] >= -1e-6 {
            self.terminal[i] = 0.0;
            self.parent[i] = NONE;
            orphans.push(i);
        }
    }

    /// Distance of `j` to the root if its ancestor path is intact.
    fn root_distance(&mut self, j: usize, time: u32) -> Option<u32> {
        let mut d = 0;
        let mut k = j;
        loop {
            if self.stamp[k] == time {
                d += self.dist[k];
                break;
            }
            let a = self.parent[k];
            d += 1;
            if a == TERMINAL {
                self.stamp[k] = time;
                self.dist[k] = 1;
                break;
            }
            if a == NONE {
                return None;
            }
            k = self.head[a as usize] as usize;
        }
        // Remembers the distances along the path walked.
        let mut k = j;
        let mut dk = d;
        while self.stamp[k] != time {
            self.stamp[k] = time;
            self.dist[k] = dk;
            dk -= 1;
            k = self.head[self.parent[k] as usize] as usize;
        }
        Some(d)
    }

    fn adopt(
        &mut self,
        o: usize,
        time: u32,
        queue: &mut std::collections::VecDeque<usize>,
        orphans: &mut Vec<usize>,
    ) {
        let source = self.tree[o] == SOURCE;
        let mut best: Option<(usize, u32)> = None;
        for a in self.arcs(o) {
            let q = self.head[a] as usize;
            if self.tree[q] != self.tree[o] {
                continue;
            }
            let residual = if source {
                self.cap[self.sister[a] as usize]
            } else {
                self.cap[a]
            };
            if residual <= 0.0 {
                continue;
            }
            if let Some(d) = self.root_distance(q, time)
                && best.is_none_or(|(_, bd)| d < bd)
            {
                best = Some((a, d));
            }
        }
        if let Some((a, d)) = best {
            self.parent[o] = a as u32;
            self.stamp[o] = time;
            self.dist[o] = d + 1;
            return;
        }
        // No new parent: the node becomes free again.
        for a in self.arcs(o) {
            let q = self.head[a] as usize;
            if self.tree[q] != self.tree[o] {
                continue;
            }
            let residual = if source {
                self.cap[self.sister[a] as usize]
            } else {
                self.cap[a]
            };
            if residual > 0.0 && !self.active[q] {
                self.active[q] = true;
                queue.push_back(q);
            }
            let pa = self.parent[q];
            if pa != NONE && pa != TERMINAL && self.head[pa as usize] as usize == o {
                self.parent[q] = NONE;
                orphans.push(q);
            }
        }
        self.tree[o] = FREE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_flow_small_graph() {
        // Source → 0 (3), 0 → 1 (2), 1 → sink (3): cut on the link.
        let mut g = Graph::new(2);
        g.set_terminals(0, 3.0, 0.0);
        g.set_terminals(1, 0.0, 3.0);
        g.set_edges(&[(0, 1, 2.0)]);
        g.max_flow();
        assert!(g.on_source_side(0) && !g.on_source_side(1));
        // Strong link: both on the side most costly to cut.
        let mut g = Graph::new(2);
        g.set_terminals(0, 3.0, 0.0);
        g.set_terminals(1, 0.0, 1.0);
        g.set_edges(&[(0, 1, 10.0)]);
        g.max_flow();
        assert!(g.on_source_side(0) && g.on_source_side(1));
    }

    #[test]
    fn finds_a_disc_on_a_plain_background() {
        let mut img = RgbaImage::from_pixel(120, 90, image::Rgba([230, 235, 240, 255]));
        for (x, y, p) in img.enumerate_pixels_mut() {
            let (dx, dy) = (x as f32 - 60.0, y as f32 - 45.0);
            if dx * dx + dy * dy < 25.0 * 25.0 {
                // Slightly textured subject.
                let v = if (x / 3 + y / 3) % 2 == 0 { 40 } else { 70 };
                p.0 = [200, v, v, 255];
            }
        }
        let mask = select_subject(&img);
        assert_eq!(mask.dimensions(), (120, 90));
        assert_eq!(mask.get_pixel(60, 45).0[0], 255);
        assert_eq!(mask.get_pixel(5, 5).0[0], 0);
        assert_eq!(mask.get_pixel(60, 45 + 32).0[0], 0);
        let selected = mask.pixels().filter(|p| p.0[0] >= 128).count() as f32;
        let disc = std::f32::consts::PI * 25.0 * 25.0;
        assert!((selected / disc - 1.0).abs() < 0.1, "{selected} vs {disc}");
    }
}
