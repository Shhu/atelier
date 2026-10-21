//! Action descriptors: the key / value structures of the newer PSD blocks
//! (layer styles, fill layers, some adjustments, text).

use super::bytes::{Reader, Result, Writer};
use crate::document::FileError;

/// A key or class id: a 4-character code, or a longer string id.
pub type Key = Vec<u8>;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Object(Descriptor),
    List(Vec<Value>),
    Double(f64),
    /// Unit ("#Prc", "#Pxl", "#Ang"…) and value.
    Unit([u8; 4], f64),
    Text(String),
    /// Enumeration type and value.
    Enum(Key, Key),
    Integer(i32),
    Large(i64),
    Bool(bool),
    Class(Key),
    /// What Atelier doesn't use (references, raw data, aliases…), skipped.
    Other,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Descriptor {
    pub class: Key,
    pub items: Vec<(Key, Value)>,
}

/// Depth limit of nested descriptors (damaged or hostile files).
const MAX_DEPTH: usize = 32;

impl Descriptor {
    pub fn new(class: &[u8]) -> Self {
        Self {
            class: class.to_vec(),
            items: Vec::new(),
        }
    }

    pub fn with(mut self, key: &[u8], value: Value) -> Self {
        self.items.push((key.to_vec(), value));
        self
    }

    pub fn get(&self, key: &[u8]) -> Option<&Value> {
        self.items.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// A number, whatever its type (double, unit float, integer).
    pub fn num(&self, key: &[u8]) -> Option<f64> {
        match self.get(key)? {
            Value::Double(v) | Value::Unit(_, v) => Some(*v),
            Value::Integer(v) => Some(*v as f64),
            Value::Large(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn bool(&self, key: &[u8]) -> Option<bool> {
        match self.get(key)? {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Value of an enumeration.
    pub fn enumerated(&self, key: &[u8]) -> Option<&[u8]> {
        match self.get(key)? {
            Value::Enum(_, v) => Some(v),
            _ => None,
        }
    }

    pub fn object(&self, key: &[u8]) -> Option<&Descriptor> {
        match self.get(key)? {
            Value::Object(d) => Some(d),
            _ => None,
        }
    }

    pub fn list(&self, key: &[u8]) -> Option<&[Value]> {
        match self.get(key)? {
            Value::List(l) => Some(l),
            _ => None,
        }
    }

    /// Reads a descriptor (its name, class id and items).
    pub fn read(r: &mut Reader) -> Result<Self> {
        read_descriptor(r, 0)
    }

    pub fn write(&self, w: &mut Writer) {
        w.unicode("");
        write_key(w, &self.class);
        w.u32(self.items.len() as u32);
        for (key, value) in &self.items {
            write_key(w, key);
            write_value(w, value);
        }
    }
}

fn read_key(r: &mut Reader) -> Result<Key> {
    let n = r.u32()? as usize;
    Ok(r.bytes(if n == 0 { 4 } else { n })?.to_vec())
}

/// 4-character codes are written as such, longer ids as strings.
fn write_key(w: &mut Writer, key: &[u8]) {
    w.u32(if key.len() == 4 { 0 } else { key.len() as u32 });
    w.bytes(key);
}

fn read_descriptor(r: &mut Reader, depth: usize) -> Result<Descriptor> {
    if depth > MAX_DEPTH {
        return Err(FileError::Damaged);
    }
    r.unicode()?;
    let class = read_key(r)?;
    let count = r.u32()? as usize;
    let mut items = Vec::with_capacity(count.min(256));
    for _ in 0..count {
        let key = read_key(r)?;
        let kind = r.tag()?;
        items.push((key, read_value(r, &kind, depth)?));
    }
    Ok(Descriptor { class, items })
}

fn read_value(r: &mut Reader, kind: &[u8; 4], depth: usize) -> Result<Value> {
    Ok(match kind {
        b"Objc" | b"GlbO" => Value::Object(read_descriptor(r, depth + 1)?),
        b"VlLs" => {
            let n = r.u32()? as usize;
            let mut items = Vec::with_capacity(n.min(256));
            for _ in 0..n {
                let kind = r.tag()?;
                items.push(read_value(r, &kind, depth + 1)?);
            }
            Value::List(items)
        }
        b"doub" => Value::Double(r.f64()?),
        b"UntF" => {
            let unit = r.tag()?;
            Value::Unit(unit, r.f64()?)
        }
        b"TEXT" => Value::Text(r.unicode()?),
        b"enum" => {
            let t = read_key(r)?;
            Value::Enum(t, read_key(r)?)
        }
        b"long" => Value::Integer(r.i32()?),
        b"comp" => Value::Large(r.i64()?),
        b"bool" => Value::Bool(r.u8()? != 0),
        b"type" | b"GlbC" => {
            r.unicode()?;
            Value::Class(read_key(r)?)
        }
        b"alis" | b"tdta" | b"Pth " => {
            let n = r.u32()? as usize;
            r.skip(n)?;
            Value::Other
        }
        b"obj " => {
            let n = r.u32()? as usize;
            for _ in 0..n {
                read_reference_item(r)?;
            }
            Value::Other
        }
        b"ObAr" => {
            r.u32()?;
            read_descriptor_items_as_floats(r)?;
            Value::Other
        }
        b"UnFl" => {
            r.tag()?;
            let n = r.u32()? as usize;
            r.skip(n.checked_mul(8).ok_or(FileError::Damaged)?)?;
            Value::Other
        }
        _ => return Err(FileError::Damaged),
    })
}

/// An object array's body: like a descriptor, its items unit float lists.
fn read_descriptor_items_as_floats(r: &mut Reader) -> Result<()> {
    r.unicode()?;
    read_key(r)?;
    let count = r.u32()? as usize;
    for _ in 0..count {
        read_key(r)?;
        let kind = r.tag()?;
        read_value(r, &kind, MAX_DEPTH)?;
    }
    Ok(())
}

fn read_reference_item(r: &mut Reader) -> Result<()> {
    match &r.tag()? {
        b"prop" => {
            r.unicode()?;
            read_key(r)?;
            read_key(r)?;
        }
        b"Clss" => {
            r.unicode()?;
            read_key(r)?;
        }
        b"Enmr" => {
            r.unicode()?;
            read_key(r)?;
            read_key(r)?;
            read_key(r)?;
        }
        b"rele" => {
            r.unicode()?;
            read_key(r)?;
            r.u32()?;
        }
        b"Idnt" | b"indx" => {
            r.u32()?;
        }
        b"name" => {
            r.unicode()?;
            read_key(r)?;
            r.unicode()?;
        }
        _ => return Err(FileError::Damaged),
    }
    Ok(())
}

fn write_value(w: &mut Writer, value: &Value) {
    match value {
        Value::Object(d) => {
            w.bytes(b"Objc");
            d.write(w);
        }
        Value::List(items) => {
            w.bytes(b"VlLs");
            w.u32(items.len() as u32);
            for v in items {
                write_value(w, v);
            }
        }
        Value::Double(v) => {
            w.bytes(b"doub");
            w.f64(*v);
        }
        Value::Unit(unit, v) => {
            w.bytes(b"UntF");
            w.bytes(unit);
            w.f64(*v);
        }
        Value::Text(t) => {
            w.bytes(b"TEXT");
            w.unicode(t);
        }
        Value::Enum(t, v) => {
            w.bytes(b"enum");
            write_key(w, t);
            write_key(w, v);
        }
        Value::Integer(v) => {
            w.bytes(b"long");
            w.i32(*v);
        }
        Value::Large(v) => {
            w.bytes(b"comp");
            w.data.extend_from_slice(&v.to_be_bytes());
        }
        Value::Bool(b) => {
            w.bytes(b"bool");
            w.u8(u8::from(*b));
        }
        Value::Class(c) => {
            w.bytes(b"type");
            w.unicode("");
            write_key(w, c);
        }
        Value::Other => unreachable!("only read"),
    }
}

/// Shorthands for building descriptors.
pub fn percent(v: f64) -> Value {
    Value::Unit(*b"#Prc", v)
}

pub fn pixels(v: f64) -> Value {
    Value::Unit(*b"#Pxl", v)
}

pub fn angle(v: f64) -> Value {
    Value::Unit(*b"#Ang", v)
}

pub fn enumerated(kind: &[u8], value: &[u8]) -> Value {
    Value::Enum(kind.to_vec(), value.to_vec())
}

/// An RGB color (0..255 components), as Photoshop writes them.
pub fn rgb([r, g, b]: [u8; 3]) -> Value {
    Value::Object(
        Descriptor::new(b"RGBC")
            .with(b"Rd  ", Value::Double(r as f64))
            .with(b"Grn ", Value::Double(g as f64))
            .with(b"Bl  ", Value::Double(b as f64)),
    )
}

/// An RGB color of a descriptor (RGB, or gray, HSB, CMYK and Lab
/// converted), components 0..255.
pub fn color_of(d: &Descriptor) -> Option<[u8; 3]> {
    use crate::color::{Hsb, hsb_to_rgb, lab_to_rgb, to_u8};
    let to8 = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    match d.class.as_slice() {
        // Newer files may give 0..1 floats instead.
        b"RGBC" => match (d.num(b"Rd  "), d.num(b"Grn "), d.num(b"Bl  ")) {
            (Some(r), Some(g), Some(b)) => Some([to8(r), to8(g), to8(b)]),
            _ => Some(
                [&b"redFloat"[..], b"greenFloat", b"blueFloat"]
                    .map(|k| to8(d.num(k).unwrap_or(0.0) * 255.0)),
            ),
        },
        b"Grsc" => Some([to8(255.0 - d.num(b"Gry ")? * 2.55); 3]),
        b"HSBC" => Some(to_u8(hsb_to_rgb(Hsb {
            h: d.num(b"H   ")? as f32,
            s: d.num(b"Strt")? as f32 / 100.0,
            b: d.num(b"Brgh")? as f32 / 100.0,
        }))),
        b"CMYC" => {
            let [c, m, y, k] =
                [b"Cyn ", b"Mgnt", b"Ylw ", b"Blck"].map(|key| d.num(key).unwrap_or(0.0) / 100.0);
            let f = |x: f64| to8(255.0 * (1.0 - x) * (1.0 - k));
            Some([f(c), f(m), f(y)])
        }
        b"LbCl" => {
            let lab = [b"Lmnc", b"A   ", b"B   "].map(|k| d.num(k).unwrap_or(0.0) as f32);
            Some(to_u8(lab_to_rgb(lab)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_round_trip() {
        let d = Descriptor::new(b"null")
            .with(b"Scl ", percent(100.0))
            .with(b"masterFXSwitch", Value::Bool(true))
            .with(b"Md  ", enumerated(b"BlnM", b"Mltp"))
            .with(b"Clr ", rgb([10, 20, 30]))
            .with(
                b"Crv ",
                Value::List(vec![Value::Integer(3), Value::Text("é".into())]),
            )
            .with(b"Nm  ", Value::Text("Linear".into()));
        let mut w = Writer::default();
        d.write(&mut w);
        let back = Descriptor::read(&mut Reader::new(&w.data)).unwrap();
        assert_eq!(back, d);
        assert_eq!(back.num(b"Scl "), Some(100.0));
        assert_eq!(back.enumerated(b"Md  "), Some(&b"Mltp"[..]));
        assert_eq!(color_of(back.object(b"Clr ").unwrap()), Some([10, 20, 30]));
        // Truncated: an error, not a panic.
        assert!(Descriptor::read(&mut Reader::new(&w.data[..w.data.len() - 3])).is_err());
    }
}
