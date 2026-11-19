//! Uncompressed, big-endian Java Edition NBT.
use std::io::{self, Cursor, Read, Write};

pub type Compound = Vec<(String, Tag)>;

#[derive(Debug, Clone, PartialEq)]
pub enum Tag {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    String(String),
    List(u8, Vec<Tag>),
    Compound(Compound),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl Tag {
    pub fn id(&self) -> u8 {
        match self {
            Self::End => 0,
            Self::Byte(_) => 1,
            Self::Short(_) => 2,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::Float(_) => 5,
            Self::Double(_) => 6,
            Self::ByteArray(_) => 7,
            Self::String(_) => 8,
            Self::List(..) => 9,
            Self::Compound(_) => 10,
            Self::IntArray(_) => 11,
            Self::LongArray(_) => 12,
        }
    }
}

struct Reader<'a>(Cursor<&'a [u8]>);
impl Reader<'_> {
    fn bytes<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let mut bytes = [0; N];
        self.0.read_exact(&mut bytes)?;
        Ok(bytes)
    }
    fn string(&mut self) -> io::Result<String> {
        let len = u16::from_be_bytes(self.bytes()?) as usize;
        let mut bytes = vec![0; len];
        self.0.read_exact(&mut bytes)?;
        decode_mutf8(&bytes)
    }
    fn len(&mut self, width: usize) -> io::Result<usize> {
        let len = i32::from_be_bytes(self.bytes()?);
        let remaining = self.0.get_ref().len() - self.0.position() as usize;
        if len < 0 || len as usize > remaining / width {
            return Err(invalid("invalid NBT collection length"));
        }
        Ok(len as usize)
    }
    fn tag(&mut self, id: u8, depth: usize) -> io::Result<Tag> {
        if depth > 128 {
            return Err(invalid("NBT nesting too deep"));
        }
        Ok(match id {
            0 => Tag::End,
            1 => Tag::Byte(i8::from_be_bytes(self.bytes()?)),
            2 => Tag::Short(i16::from_be_bytes(self.bytes()?)),
            3 => Tag::Int(i32::from_be_bytes(self.bytes()?)),
            4 => Tag::Long(i64::from_be_bytes(self.bytes()?)),
            5 => Tag::Float(f32::from_be_bytes(self.bytes()?)),
            6 => Tag::Double(f64::from_be_bytes(self.bytes()?)),
            7 => {
                let len = self.len(1)?;
                let mut bytes = vec![0; len];
                self.0.read_exact(&mut bytes)?;
                Tag::ByteArray(bytes)
            }
            8 => Tag::String(self.string()?),
            9 => {
                let id = self.bytes::<1>()?[0];
                let len = self.len(1)?;
                if id > 12 || (id == 0 && len != 0) {
                    return Err(invalid("invalid list type"));
                }
                Tag::List(
                    id,
                    (0..len)
                        .map(|_| self.tag(id, depth + 1))
                        .collect::<io::Result<_>>()?,
                )
            }
            10 => {
                let mut tags = Vec::new();
                loop {
                    let id = self.bytes::<1>()?[0];
                    if id == 0 {
                        break;
                    }
                    let name = self.string()?;
                    tags.push((name, self.tag(id, depth + 1)?));
                }
                Tag::Compound(tags)
            }
            11 => {
                let len = self.len(4)?;
                Tag::IntArray(
                    (0..len)
                        .map(|_| Ok(i32::from_be_bytes(self.bytes()?)))
                        .collect::<io::Result<_>>()?,
                )
            }
            12 => {
                let len = self.len(8)?;
                Tag::LongArray(
                    (0..len)
                        .map(|_| Ok(i64::from_be_bytes(self.bytes()?)))
                        .collect::<io::Result<_>>()?,
                )
            }
            _ => return Err(invalid("unknown NBT tag type")),
        })
    }
}

pub fn read(bytes: &[u8]) -> io::Result<(String, Tag)> {
    let mut reader = Reader(Cursor::new(bytes));
    let id = reader.bytes::<1>()?[0];
    if id == 0 {
        return Ok((String::new(), Tag::End));
    }
    let name = reader.string()?;
    let tag = reader.tag(id, 0)?;
    if reader.0.position() as usize != bytes.len() {
        return Err(invalid("trailing NBT data"));
    }
    Ok((name, tag))
}

fn decode_mutf8(bytes: &[u8]) -> io::Result<String> {
    let mut units = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let first = bytes[i];
        let (unit, width) = match first {
            1..=0x7f => (u16::from(first), 1),
            0xc0..=0xdf => {
                let second = *bytes
                    .get(i + 1)
                    .ok_or_else(|| invalid("truncated MUTF-8"))?;
                if second & 0xc0 != 0x80 {
                    return Err(invalid("invalid MUTF-8 continuation"));
                }
                let unit = (u16::from(first & 0x1f) << 6) | u16::from(second & 0x3f);
                if unit < 0x80 && unit != 0 {
                    return Err(invalid("overlong MUTF-8"));
                }
                (unit, 2)
            }
            0xe0..=0xef => {
                let second = *bytes
                    .get(i + 1)
                    .ok_or_else(|| invalid("truncated MUTF-8"))?;
                let third = *bytes
                    .get(i + 2)
                    .ok_or_else(|| invalid("truncated MUTF-8"))?;
                if second & 0xc0 != 0x80 || third & 0xc0 != 0x80 {
                    return Err(invalid("invalid MUTF-8 continuation"));
                }
                let unit = (u16::from(first & 0x0f) << 12)
                    | (u16::from(second & 0x3f) << 6)
                    | u16::from(third & 0x3f);
                if unit < 0x800 {
                    return Err(invalid("overlong MUTF-8"));
                }
                (unit, 3)
            }
            _ => return Err(invalid("invalid MUTF-8 byte")),
        };
        units.push(unit);
        i += width;
    }
    String::from_utf16(&units).map_err(|_| invalid("unpaired MUTF-8 surrogate"))
}

fn string(out: &mut Vec<u8>, value: &str) -> io::Result<()> {
    let mut bytes = Vec::new();
    for unit in value.encode_utf16() {
        match unit {
            1..=0x7f => bytes.push(unit as u8),
            0..=0x7ff => {
                bytes.push(0xc0 | (unit >> 6) as u8);
                bytes.push(0x80 | (unit & 0x3f) as u8);
            }
            _ => {
                bytes.push(0xe0 | (unit >> 12) as u8);
                bytes.push(0x80 | ((unit >> 6) & 0x3f) as u8);
                bytes.push(0x80 | (unit & 0x3f) as u8);
            }
        }
    }
    let len = u16::try_from(bytes.len()).map_err(|_| invalid("NBT string too long"))?;
    out.write_all(&len.to_be_bytes())?;
    out.write_all(&bytes)
}
fn length(out: &mut Vec<u8>, len: usize) -> io::Result<()> {
    out.write_all(
        &i32::try_from(len)
            .map_err(|_| invalid("NBT collection too long"))?
            .to_be_bytes(),
    )
}
fn payload(out: &mut Vec<u8>, tag: &Tag, depth: usize) -> io::Result<()> {
    if depth > 128 {
        return Err(invalid("NBT nesting too deep"));
    }
    match tag {
        Tag::End => (),
        Tag::Byte(v) => out.extend(v.to_be_bytes()),
        Tag::Short(v) => out.extend(v.to_be_bytes()),
        Tag::Int(v) => out.extend(v.to_be_bytes()),
        Tag::Long(v) => out.extend(v.to_be_bytes()),
        Tag::Float(v) => out.extend(v.to_be_bytes()),
        Tag::Double(v) => out.extend(v.to_be_bytes()),
        Tag::String(v) => string(out, v)?,
        Tag::ByteArray(v) => {
            length(out, v.len())?;
            out.extend(v);
        }
        Tag::List(id, values) => {
            if *id > 12 || (*id == 0 && !values.is_empty()) || values.iter().any(|v| v.id() != *id)
            {
                return Err(invalid("invalid NBT list"));
            }
            out.push(*id);
            length(out, values.len())?;
            for value in values {
                payload(out, value, depth + 1)?;
            }
        }
        Tag::Compound(values) => {
            for (name, value) in values {
                if value.id() == 0 {
                    return Err(invalid("named End tag"));
                }
                out.push(value.id());
                string(out, name)?;
                payload(out, value, depth + 1)?;
            }
            out.push(0);
        }
        Tag::IntArray(values) => {
            length(out, values.len())?;
            for value in values {
                out.extend(value.to_be_bytes());
            }
        }
        Tag::LongArray(values) => {
            length(out, values.len())?;
            for value in values {
                out.extend(value.to_be_bytes());
            }
        }
    }
    Ok(())
}
pub fn write(name: &str, tag: &Tag) -> io::Result<Vec<u8>> {
    let mut out = vec![tag.id()];
    if tag.id() != 0 {
        string(&mut out, name)?;
        payload(&mut out, tag, 0)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modified_utf8() {
        let name = "Server 😀\0";
        let tag = Tag::String("NUL\0 and emoji 🚀".into());
        assert_eq!(
            read(&write(name, &tag).unwrap()).unwrap(),
            (name.into(), tag)
        );
        assert_eq!(
            read(b"\x08\x00\x00\x00\x06\xed\xa0\xbd\xed\xb8\x80").unwrap(),
            (String::new(), Tag::String("😀".into()))
        );
        assert_eq!(
            write("", &Tag::String("\0".into())).unwrap(),
            b"\x08\x00\x00\x00\x02\xc0\x80"
        );
        assert!(write("", &Tag::String("😀".repeat(10923))).is_err());
        assert!(write("", &Tag::String("\0".repeat(32768))).is_err());
        for bytes in [
            &b"\xc1\x81"[..],
            &b"\xe0\x80\x80"[..],
            &b"\xed\xa0\xbd"[..],
            &b"\xf0\x9f\x98\x80"[..],
            &b"\xc0"[..],
            &b"\xc0\x41"[..],
            &b"\x00"[..],
        ] {
            assert!(decode_mutf8(bytes).is_err());
        }
    }
    #[test]
    fn every_type() {
        let tags = vec![
            Tag::End,
            Tag::Byte(-1),
            Tag::Short(-234),
            Tag::Int(-123456),
            Tag::Long(i64::MIN),
            Tag::Float(1.25),
            Tag::Double(-2.5),
            Tag::ByteArray(vec![0, 255]),
            Tag::String("héllo".into()),
            Tag::List(3, vec![Tag::Int(42)]),
            Tag::Compound(vec![("x".into(), Tag::Byte(1))]),
            Tag::IntArray(vec![i32::MIN, 2]),
            Tag::LongArray(vec![i64::MAX, -2]),
            Tag::List(0, vec![]),
        ];
        for tag in tags {
            assert_eq!(read(&write("", &tag).unwrap()).unwrap(), ("".into(), tag));
        }
    }
    #[test]
    fn invalid_input() {
        for bytes in [
            vec![],
            vec![13, 0, 0],
            vec![8, 0, 0, 0, 1, 255],
            vec![7, 0, 0, 255, 255, 255, 255],
        ] {
            assert!(read(&bytes).is_err());
        }
        assert!(write("", &Tag::List(1, vec![Tag::Int(0)])).is_err());
        assert!(write("", &Tag::String("a".repeat(65536))).is_err());
    }
}
