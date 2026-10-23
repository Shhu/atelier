//! Lossless JPEG (ITU T.81, process 14), the compression of most DNG
//! files: Huffman-coded differences to a predictor.

/// Huffman table: for the next 16 bits, the code's length and its value.
struct Table {
    lookup: Vec<(u8, u8)>,
}

impl Table {
    fn new(counts: &[u8; 16], values: &[u8]) -> Option<Table> {
        let mut lookup = vec![(0u8, 0u8); 1 << 16];
        let mut code = 0u32;
        let mut k = 0;
        for (i, &n) in counts.iter().enumerate() {
            let len = i as u32 + 1;
            for _ in 0..n {
                let value = *values.get(k)?;
                k += 1;
                let first = (code << (16 - len)) as usize;
                let span = 1usize << (16 - len);
                lookup
                    .get_mut(first..first + span)?
                    .fill((len as u8, value));
                code += 1;
            }
            code <<= 1;
        }
        Some(Table { lookup })
    }
}

/// Bits of the entropy-coded data, its stuffed bytes removed; zeros past
/// a marker.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    count: u32,
    marker: bool,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits {
            data,
            pos: 0,
            acc: 0,
            count: 0,
            marker: false,
        }
    }

    fn fill(&mut self) {
        while self.count <= 56 {
            let mut byte = 0;
            if !self.marker && self.pos < self.data.len() {
                byte = self.data[self.pos];
                if byte == 0xFF {
                    if self.data.get(self.pos + 1) == Some(&0) {
                        self.pos += 2;
                    } else {
                        self.marker = true;
                        byte = 0;
                    }
                } else {
                    self.pos += 1;
                }
            }
            self.acc |= (byte as u64) << (56 - self.count);
            self.count += 8;
        }
    }

    fn take(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.count < n {
            self.fill();
        }
        let v = (self.acc >> (64 - n)) as u32;
        self.acc <<= n;
        self.count -= n;
        v
    }

    fn huffman(&mut self, table: &Table) -> Option<u32> {
        if self.count < 16 {
            self.fill();
        }
        let (len, value) = table.lookup[(self.acc >> 48) as usize];
        if len == 0 {
            return None;
        }
        self.acc <<= len;
        self.count -= len as u32;
        Some(value as u32)
    }

    /// Past the restart marker that ends this interval.
    fn restart(&mut self) {
        let mut at = self.pos;
        while at + 1 < self.data.len()
            && !(self.data[at] == 0xFF && (0xD0..=0xD7).contains(&self.data[at + 1]))
        {
            at += 1;
        }
        self.pos = (at + 2).min(self.data.len());
        self.acc = 0;
        self.count = 0;
        self.marker = false;
    }
}

fn be16(data: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]) as usize)
}

/// Decodes a lossless JPEG stream: its samples, the components of each
/// pixel together, rows from the top; `None` if it is not one, is
/// damaged, or holds more than `limit` samples.
pub fn decode(data: &[u8], limit: usize) -> Option<Vec<u16>> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut tables: [Option<Table>; 4] = [None, None, None, None];
    let (mut precision, mut width, mut height, mut components) = (0u32, 0, 0, 0);
    let mut restart_interval = 0usize;
    let mut at = 2;
    loop {
        while *data.get(at)? != 0xFF {
            at += 1;
        }
        let marker = *data.get(at + 1)?;
        at += 2;
        if marker == 0xFF || marker == 0xD8 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        let len = be16(data, at)?;
        let segment = data.get(at + 2..at + len)?;
        match marker {
            0xC4 => {
                let mut s = segment;
                while s.len() >= 17 {
                    let id = (s[0] & 0x0F) as usize;
                    let counts: [u8; 16] = s[1..17].try_into().ok()?;
                    let n: usize = counts.iter().map(|&c| c as usize).sum();
                    let values = s.get(17..17 + n)?;
                    *tables.get_mut(id)? = Some(Table::new(&counts, values)?);
                    s = &s[17 + n..];
                }
            }
            0xC3 => {
                precision = *segment.first()? as u32;
                height = be16(segment, 1)?;
                width = be16(segment, 3)?;
                components = *segment.get(5)? as usize;
            }
            // Other frame types: not lossless.
            0xC0..=0xC2 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            0xDD => restart_interval = be16(segment, 0)?,
            0xDA => {
                let ns = *segment.first()? as usize;
                let mut selected = Vec::with_capacity(ns);
                for i in 0..ns {
                    let id = (segment.get(2 + 2 * i)? >> 4) as usize;
                    selected.push(tables.get(id)?.as_ref()?);
                }
                let predictor = *segment.get(1 + 2 * ns)? as u32;
                let transform = (*segment.get(3 + 2 * ns)? & 0x0F) as u32;
                if ns != components || components == 0 || width == 0 || height == 0 {
                    return None;
                }
                if width * height * components > limit {
                    return None;
                }
                let scan = data.get(at + len..)?;
                return scan_samples(
                    scan,
                    &selected,
                    (width, height, components),
                    precision,
                    predictor,
                    transform,
                    restart_interval,
                );
            }
            0xD9 => return None,
            _ => {}
        }
        at += len;
    }
}

fn scan_samples(
    scan: &[u8],
    tables: &[&Table],
    (width, height, n): (usize, usize, usize),
    precision: u32,
    predictor: u32,
    transform: u32,
    restart_interval: usize,
) -> Option<Vec<u16>> {
    let mut bits = Bits::new(scan);
    let mut samples = vec![0u16; width * height * n];
    let row = width * n;
    let initial = 1i32 << (precision.saturating_sub(transform + 1));
    let mut since_restart = 0usize;
    let mut fresh = true;
    let mut first_line = 0;
    for y in 0..height {
        for x in 0..width {
            if restart_interval > 0 && since_restart == restart_interval {
                bits.restart();
                since_restart = 0;
                fresh = true;
                first_line = y;
            }
            for (c, table) in tables.iter().enumerate() {
                let ssss = bits.huffman(table)?;
                let diff = match ssss {
                    0 => 0,
                    16 => 32768,
                    s if s < 16 => {
                        let v = bits.take(s) as i32;
                        if v < 1 << (s - 1) {
                            v - (1 << s) + 1
                        } else {
                            v
                        }
                    }
                    _ => return None,
                };
                let i = y * row + x * n + c;
                let at = |k: usize| samples[k] as i32;
                // After a start or restart: the first sample, then the
                // line goes from the left, the next ones from above.
                let prediction = if fresh {
                    initial
                } else if y == first_line {
                    at(i - n)
                } else if x == 0 {
                    at(i - row)
                } else {
                    let (ra, rb, rc) = (at(i - n), at(i - row), at(i - row - n));
                    match predictor {
                        2 => rb,
                        3 => rc,
                        4 => ra + rb - rc,
                        5 => ra + ((rb - rc) >> 1),
                        6 => rb + ((ra - rc) >> 1),
                        7 => (ra + rb) >> 1,
                        _ => ra,
                    }
                };
                samples[i] = ((prediction + diff) & 0xFFFF) as u16;
            }
            fresh = false;
            since_restart += 1;
        }
    }
    if transform > 0 {
        samples.iter_mut().for_each(|s| *s <<= transform);
    }
    Some(samples)
}

#[cfg(test)]
pub mod tests {
    /// A lossless JPEG of `samples` (`width` × `height` × `n`, predictor
    /// 1), with a simple Huffman table (lengths 0 to 16, 4-bit codes, then
    /// 5 bits for 15 and 16).
    pub fn encode(
        samples: &[u16],
        width: usize,
        height: usize,
        n: usize,
        precision: u8,
    ) -> Vec<u8> {
        // Code lengths: 15 categories of 4 bits (0000..1110), 2 of 5 bits.
        let mut counts = [0u8; 16];
        counts[3] = 15;
        counts[4] = 2;
        let values: Vec<u8> = (0..=16).collect();
        let code_of = |s: u32| -> (u32, u32) {
            if s < 15 {
                (s, 4)
            } else {
                (0b11110 + (s - 15), 5)
            }
        };
        let mut out = vec![0xFF, 0xD8];
        // DHT.
        out.extend([0xFF, 0xC4]);
        let len = 2 + 17 + values.len();
        out.extend((len as u16).to_be_bytes());
        out.push(0x00);
        out.extend(counts);
        out.extend(&values);
        // SOF3.
        out.extend([0xFF, 0xC3]);
        out.extend(((8 + 3 * n) as u16).to_be_bytes());
        out.push(precision);
        out.extend((height as u16).to_be_bytes());
        out.extend((width as u16).to_be_bytes());
        out.push(n as u8);
        for c in 0..n {
            out.extend([c as u8, 0x11, 0]);
        }
        // SOS: predictor 1.
        out.extend([0xFF, 0xDA]);
        out.extend(((6 + 2 * n) as u16).to_be_bytes());
        out.push(n as u8);
        for c in 0..n {
            out.extend([c as u8, 0x00]);
        }
        out.extend([1, 0, 0]);
        // Entropy-coded data.
        let mut bits = Writer::default();
        let row = width * n;
        for y in 0..height {
            for x in 0..width {
                for c in 0..n {
                    let i = y * row + x * n + c;
                    let prediction = if x == 0 && y == 0 {
                        1i32 << (precision - 1)
                    } else if x == 0 {
                        samples[i - row] as i32
                    } else {
                        samples[i - n] as i32
                    };
                    let mut diff = samples[i] as i32 - prediction;
                    if diff > 32767 {
                        diff -= 65536;
                    } else if diff < -32768 {
                        diff += 65536;
                    }
                    let s = if diff == 0 {
                        0
                    } else {
                        32 - diff.unsigned_abs().leading_zeros()
                    };
                    let (code, len) = code_of(s);
                    bits.put(code, len);
                    if s > 0 && s < 16 {
                        let v = if diff < 0 { diff + (1 << s) - 1 } else { diff };
                        bits.put(v as u32, s);
                    }
                }
            }
        }
        if bits.count > 0 {
            bits.put(0xFF, 8 - bits.count);
        }
        out.extend(bits.data);
        out.extend([0xFF, 0xD9]);
        out
    }

    /// Bits written most significant first, 0xFF bytes stuffed.
    #[derive(Default)]
    struct Writer {
        acc: u64,
        count: u32,
        data: Vec<u8>,
    }

    impl Writer {
        fn put(&mut self, v: u32, len: u32) {
            self.acc = (self.acc << len) | (v as u64 & ((1u64 << len) - 1));
            self.count += len;
            while self.count >= 8 {
                let byte = (self.acc >> (self.count - 8)) as u8;
                self.data.push(byte);
                if byte == 0xFF {
                    self.data.push(0);
                }
                self.count -= 8;
            }
        }
    }

    #[test]
    fn decodes_what_the_test_encoder_wrote() {
        let (w, h, n) = (13, 7, 2);
        let samples: Vec<u16> = (0..w * h * n).map(|i| ((i * 7919) % 4096) as u16).collect();
        let jpeg = encode(&samples, w, h, n, 12);
        assert_eq!(
            super::decode(&jpeg, samples.len()).expect("decoded"),
            samples
        );
        // A frame larger than the tile it is for: refused, not decoded.
        assert!(super::decode(&jpeg, samples.len() - 1).is_none());
    }
}
