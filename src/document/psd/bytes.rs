//! Big-endian reading and writing of PSD structures: numbers, Pascal and
//! Unicode strings, length-prefixed blocks, PackBits (RLE) rows.

use super::super::FileError;

pub type Result<T> = std::result::Result<T, FileError>;

/// Reads from a slice; running past its end means a damaged file.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(FileError::Damaged);
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    /// The next `n` bytes as a reader of their own.
    pub fn sub(&mut self, n: usize) -> Result<Reader<'a>> {
        self.bytes(n).map(Reader::new)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().expect("N bytes"))
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_be_bytes)
    }

    pub fn i16(&mut self) -> Result<i16> {
        self.array().map(i16::from_be_bytes)
    }

    pub fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_be_bytes)
    }

    pub fn i32(&mut self) -> Result<i32> {
        self.array().map(i32::from_be_bytes)
    }

    pub fn u64(&mut self) -> Result<u64> {
        self.array().map(u64::from_be_bytes)
    }

    pub fn i64(&mut self) -> Result<i64> {
        self.array().map(i64::from_be_bytes)
    }

    pub fn f64(&mut self) -> Result<f64> {
        self.array().map(f64::from_be_bytes)
    }

    pub fn tag(&mut self) -> Result<[u8; 4]> {
        self.array()
    }

    /// A length: 4 bytes, or 8 in a PSB (`big`).
    pub fn len(&mut self, big: bool) -> Result<usize> {
        let n = if big {
            self.u64()?
        } else {
            u64::from(self.u32()?)
        };
        usize::try_from(n).map_err(|_| FileError::Damaged)
    }

    /// Pascal string (length byte, then the bytes), the whole padded to a
    /// multiple of `pad`.
    pub fn pascal(&mut self, pad: usize) -> Result<&'a [u8]> {
        let n = self.u8()? as usize;
        let text = self.bytes(n)?;
        let total = 1 + n;
        self.skip((pad - total % pad) % pad)?;
        Ok(text)
    }

    /// Unicode string: a count of UTF-16 units, then them (a final NUL is
    /// dropped).
    pub fn unicode(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        let raw = self.bytes(n.checked_mul(2).ok_or(FileError::Damaged)?)?;
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16_lossy(&units);
        Ok(text.trim_end_matches('\0').to_owned())
    }
}

/// Builds a file in memory.
#[derive(Default)]
pub struct Writer {
    pub data: Vec<u8>,
}

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.data.push(v);
    }

    pub fn u16(&mut self, v: u16) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn i16(&mut self, v: i16) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.data.extend_from_slice(b);
    }

    pub fn zeros(&mut self, n: usize) {
        self.data.resize(self.data.len() + n, 0);
    }

    /// Pascal string of `text` (at most 255 bytes), padded to a multiple of
    /// `pad`.
    pub fn pascal(&mut self, text: &[u8], pad: usize) {
        let text = &text[..text.len().min(255)];
        self.u8(text.len() as u8);
        self.bytes(text);
        let total = 1 + text.len();
        self.zeros((pad - total % pad) % pad);
    }

    pub fn unicode(&mut self, text: &str) {
        let units: Vec<u16> = text.encode_utf16().collect();
        self.u32(units.len() as u32);
        for u in units {
            self.u16(u);
        }
    }

    /// A 4-byte length, then what `f` writes, padded with zeros to a
    /// multiple of `pad` (the padding counted in the length).
    pub fn block(&mut self, pad: usize, f: impl FnOnce(&mut Writer)) {
        let at = self.data.len();
        self.u32(0);
        f(self);
        let len = self.data.len() - at - 4;
        self.zeros((pad - len % pad) % pad);
        let len = (self.data.len() - at - 4) as u32;
        self.data[at..at + 4].copy_from_slice(&len.to_be_bytes());
    }

    /// Tagged block `key` ("8BIM" signature), its data padded to 4 bytes.
    pub fn tagged(&mut self, key: &[u8; 4], f: impl FnOnce(&mut Writer)) {
        self.bytes(b"8BIM");
        self.bytes(key);
        self.block(4, f);
    }
}

/// Decodes a PackBits row into `out`, which it must fill exactly.
pub fn unpack_row(mut src: &[u8], out: &mut [u8]) -> Result<()> {
    let mut i = 0;
    while i < out.len() {
        let (&n, rest) = src.split_first().ok_or(FileError::Damaged)?;
        src = rest;
        let n = n as i8;
        if n >= 0 {
            let len = n as usize + 1;
            if len > src.len() || i + len > out.len() {
                return Err(FileError::Damaged);
            }
            out[i..i + len].copy_from_slice(&src[..len]);
            src = &src[len..];
            i += len;
        } else if n != -128 {
            let len = 1 - n as isize;
            let len = len as usize;
            let (&v, rest) = src.split_first().ok_or(FileError::Damaged)?;
            src = rest;
            if i + len > out.len() {
                return Err(FileError::Damaged);
            }
            out[i..i + len].fill(v);
            i += len;
        }
    }
    Ok(())
}

/// Encodes a row with PackBits (runs of 3 or more identical bytes are
/// repeated, the rest copied literally, 128 bytes at most per packet).
pub fn pack_row(row: &[u8], out: &mut Vec<u8>) {
    let mut i = 0;
    let n = row.len();
    while i < n {
        let mut run = 1;
        while i + run < n && run < 128 && row[i + run] == row[i] {
            run += 1;
        }
        if run >= 3 {
            out.push((1 - run as isize) as i8 as u8);
            out.push(row[i]);
            i += run;
            continue;
        }
        // Literal bytes until the next run of 3.
        let start = i;
        while i < n && i - start < 128 {
            if i + 2 < n && row[i] == row[i + 1] && row[i] == row[i + 2] {
                break;
            }
            i += 1;
        }
        out.push((i - start - 1) as u8);
        out.extend_from_slice(&row[start..i]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits_round_trip() {
        let rows: [&[u8]; 6] = [
            &[],
            &[7],
            &[1, 2, 3, 4, 5],
            &[9; 300],
            &[1, 1, 2, 2, 2, 2, 3, 4, 4, 4, 5],
            &[0, 0, 1, 0, 0, 1, 0, 0, 1],
        ];
        for row in rows {
            let mut packed = Vec::new();
            pack_row(row, &mut packed);
            let mut out = vec![0; row.len()];
            unpack_row(&packed, &mut out).unwrap();
            assert_eq!(out, row);
        }
        // A run takes two bytes, whatever its length up to 128.
        let mut packed = Vec::new();
        pack_row(&[5; 128], &mut packed);
        assert_eq!(packed, [129, 5]);
        // Spec example (Apple's PackBits documentation).
        let mut out = [0; 24];
        let src = [
            0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xFD, 0xAA, 0x03, 0x80, 0x00, 0x2A, 0x22, 0xF7,
            0xAA,
        ];
        unpack_row(&src, &mut out).unwrap();
        assert_eq!(out[..3], [0xAA; 3]);
        assert_eq!(out[15..], [0xAA; 9]);
        // Damaged: the row is not filled.
        assert!(unpack_row(&[0x05, 1], &mut [0; 6]).is_err());
    }

    #[test]
    fn strings_and_blocks() {
        let mut w = Writer::default();
        w.pascal(b"abc", 4);
        w.unicode("é€");
        w.block(4, |w| w.u8(1));
        let mut r = Reader::new(&w.data);
        assert_eq!(r.pascal(4).unwrap(), b"abc");
        assert_eq!(r.unicode().unwrap(), "é€");
        assert_eq!(r.u32().unwrap(), 4);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.remaining(), 3);
        assert!(r.bytes(4).is_err());
    }
}
