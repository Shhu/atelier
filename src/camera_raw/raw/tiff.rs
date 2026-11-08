//! Just enough TIFF to read RAW files: image file directories, their
//! entries' values, sub-IFDs. Also the TIFF-like headers of ORF and RW2.

/// A TIFF file in memory.
pub struct Tiff<'a> {
    pub data: &'a [u8],
    /// Little-endian ("II").
    pub little: bool,
}

/// An entry: tag, type, number of values and where they are.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub tag: u16,
    kind: u16,
    count: usize,
    at: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Ifd {
    pub entries: Vec<Entry>,
}

impl Ifd {
    pub fn get(&self, tag: u16) -> Option<Entry> {
        self.entries.iter().find(|e| e.tag == tag).copied()
    }
}

/// Directories read from a file at most: a DNG has a handful.
const MAX_IFDS: usize = 64;

/// Size of a value of each TIFF type.
fn type_size(kind: u16) -> usize {
    match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 | 16 | 17 | 18 => 8,
        _ => 0,
    }
}

impl<'a> Tiff<'a> {
    /// The file and the offset of its first directory, if it is a TIFF (or
    /// an ORF / RW2, which only change the magic number).
    pub fn new(data: &'a [u8]) -> Option<(Tiff<'a>, usize)> {
        let little = match data.get(..2)? {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let tiff = Tiff { data, little };
        let magic = tiff.u16(2)?;
        if ![42, 0x4f52, 0x5352, 0x55].contains(&magic) {
            return None;
        }
        let first = tiff.u32(4)? as usize;
        Some((tiff, first))
    }

    pub fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.data.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if self.little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    pub fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.data.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if self.little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    /// The directory at `at` and the offset of the next one (0: none).
    pub fn ifd(&self, at: usize) -> Option<(Ifd, usize)> {
        let n = self.u16(at)? as usize;
        // Entries past the end of the file are not there.
        if at.checked_add(2 + 12 * n)? > self.data.len() {
            return None;
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let e = at + 2 + 12 * i;
            let (tag, kind, count) = (self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)? as usize);
            let size = type_size(kind).saturating_mul(count);
            let value = if size <= 4 {
                e + 8
            } else {
                self.u32(e + 8)? as usize
            };
            if size > 0
                && value
                    .checked_add(size)
                    .is_some_and(|end| end <= self.data.len())
            {
                entries.push(Entry {
                    tag,
                    kind,
                    count,
                    at: value,
                });
            }
        }
        let next = self.u32(at + 2 + 12 * n).unwrap_or(0) as usize;
        Some((Ifd { entries }, next))
    }

    /// The directories chained from `first`, their offsets added to
    /// `seen`.
    fn chain(&self, first: usize, seen: &mut Vec<usize>) -> Vec<Ifd> {
        let mut out = Vec::new();
        let mut at = first;
        while at != 0 && !seen.contains(&at) && out.len() < 16 {
            seen.push(at);
            let Some((ifd, next)) = self.ifd(at) else {
                break;
            };
            out.push(ifd);
            at = next;
        }
        out
    }

    /// The directories of the file: the chain, and their sub-IFDs (two
    /// levels deep), each read once and [`MAX_IFDS`] at most (a directory
    /// listing itself many times would multiply them).
    pub fn all_ifds(&self, first: usize) -> Vec<Ifd> {
        let mut seen = Vec::new();
        let mut out = self.chain(first, &mut seen);
        let mut i = 0;
        let mut depth_end = out.len();
        let mut depth = 0;
        while i < out.len() && depth < 2 {
            if let Some(e) = out[i].get(330) {
                for offset in self.values(e) {
                    if !(0.0..=f64::from(u32::MAX)).contains(&offset) {
                        continue;
                    }
                    let at = offset as usize;
                    if out.len() >= MAX_IFDS || seen.contains(&at) {
                        continue;
                    }
                    seen.push(at);
                    if let Some((sub, _)) = self.ifd(at) {
                        out.push(sub);
                    }
                }
            }
            i += 1;
            if i == depth_end {
                depth += 1;
                depth_end = out.len();
            }
        }
        out
    }

    /// The values of an entry, as numbers.
    pub fn values(&self, e: Entry) -> Vec<f64> {
        let size = type_size(e.kind);
        (0..e.count.min(1 << 20))
            .filter_map(|i| {
                let at = e.at + i * size;
                Some(match e.kind {
                    1 | 7 => *self.data.get(at)? as f64,
                    6 => *self.data.get(at)? as i8 as f64,
                    3 => self.u16(at)? as f64,
                    8 => self.u16(at)? as i16 as f64,
                    4 | 13 => self.u32(at)? as f64,
                    9 => self.u32(at)? as i32 as f64,
                    5 => self.u32(at)? as f64 / (self.u32(at + 4)?.max(1)) as f64,
                    10 => {
                        let d = self.u32(at + 4)? as i32;
                        self.u32(at)? as i32 as f64 / if d == 0 { 1.0 } else { d as f64 }
                    }
                    11 => f32::from_bits(self.u32(at)?) as f64,
                    12 => {
                        let (a, b) = (self.u32(at)? as u64, self.u32(at + 4)? as u64);
                        let bits = if self.little {
                            b << 32 | a
                        } else {
                            a << 32 | b
                        };
                        f64::from_bits(bits)
                    }
                    _ => return None,
                })
            })
            .collect()
    }

    /// First value of `tag` in `ifd`.
    pub fn number(&self, ifd: &Ifd, tag: u16) -> Option<f64> {
        ifd.get(tag).and_then(|e| self.values(e).first().copied())
    }

    pub fn numbers(&self, ifd: &Ifd, tag: u16) -> Option<Vec<f64>> {
        ifd.get(tag).map(|e| self.values(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory listing itself as its sub-IFD many times (fuzzing: a
    /// 12 KB file made 9 million directories): each read once; offsets
    /// past any file are skipped.
    #[test]
    fn sub_ifds_are_read_once() {
        let n = 3000u32;
        let mut file = b"II*\0\x08\0\0\0".to_vec();
        file.extend(2u16.to_le_bytes());
        // SubIFDs: n offsets, all to this directory.
        file.extend(330u16.to_le_bytes());
        file.extend(4u16.to_le_bytes());
        file.extend(n.to_le_bytes());
        file.extend(38u32.to_le_bytes());
        // SubIFDs again, as a huge double.
        file.extend(330u16.to_le_bytes());
        file.extend(12u16.to_le_bytes());
        file.extend(1u32.to_le_bytes());
        file.extend((38 + 4 * n).to_le_bytes());
        file.extend(0u32.to_le_bytes());
        for _ in 0..n {
            file.extend(8u32.to_le_bytes());
        }
        file.extend(1e300f64.to_le_bytes());
        let (tiff, first) = Tiff::new(&file).unwrap();
        let ifds = tiff.all_ifds(first);
        assert_eq!(ifds.len(), 1);
        assert_eq!(tiff.u16(usize::MAX), None);
        assert_eq!(tiff.u32(usize::MAX - 1), None);
    }
}
