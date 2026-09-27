use std::collections::BTreeMap;
use std::io::{self, Read, Write};

pub struct Reader<'a> {
    bytes: &'a [u8],
    off: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, off: 0 }
    }
    pub fn offset(&self) -> usize {
        self.off
    }
    pub fn is_at_end(&self) -> bool {
        self.off == self.bytes.len()
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.off.checked_add(n)?;
        let s = self.bytes.get(self.off..end)?;
        self.off = end;
        Some(s)
    }
    fn arr<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N)?.try_into().ok()
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.arr::<1>()?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.arr()?))
    }
    pub fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.arr()?))
    }
    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.arr()?))
    }
    pub fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.arr()?))
    }
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        self.take(n)
    }
}

pub fn put_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

pub fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn put_i64(buf: &mut Vec<u8>, v: i64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn put_f32(buf: &mut Vec<u8>, v: f32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn put_f64(buf: &mut Vec<u8>, v: f64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

pub fn read_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut bytes = [0u8; 2];
    r.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

pub fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut bytes = [0u8; 4];
    r.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

pub fn write_u16(w: &mut impl Write, v: u16) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

pub fn write_u32(w: &mut impl Write, v: u32) -> io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

pub fn put_kv_map(buf: &mut Vec<u8>, map: &BTreeMap<String, Vec<u8>>) {
    let entries: Vec<(&String, &Vec<u8>)> = map
        .iter()
        .filter(|(k, _)| k.len() <= u16::MAX as usize)
        .take(u16::MAX as usize)
        .collect();
    put_u16(buf, entries.len() as u16);
    for (k, v) in entries {
        put_u16(buf, k.len() as u16);
        buf.extend_from_slice(k.as_bytes());
        put_u32(buf, v.len() as u32);
        buf.extend_from_slice(v);
    }
}

pub fn get_kv_map(r: &mut Reader) -> Option<BTreeMap<String, Vec<u8>>> {
    let n = r.u16()? as usize;
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let klen = r.u16()? as usize;
        let key = std::str::from_utf8(r.bytes(klen)?).ok()?.to_owned();
        let vlen = r.u32()? as usize;
        out.insert(key, r.bytes(vlen)?.to_vec());
    }
    Some(out)
}

pub fn deflate(payload: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = e.write_all(payload);
    e.finish().unwrap_or_default()
}

pub fn inflate(blob: &[u8]) -> Option<Vec<u8>> {
    let mut d = flate2::read::ZlibDecoder::new(blob);
    let mut out = Vec::new();
    d.read_to_end(&mut out).ok()?;
    Some(out)
}

pub fn put_indexed<T>(
    buf: &mut Vec<u8>,
    map: &BTreeMap<u16, T>,
    rec_bytes: usize,
    mut body: impl FnMut(&mut Vec<u8>, &T),
) {
    let n = map.len().min(u16::MAX as usize);
    buf.reserve(2 + n * rec_bytes);
    put_u16(buf, n as u16);
    for (idx, rec) in map.iter().take(n) {
        put_u16(buf, *idx);
        body(buf, rec);
    }
}

pub fn get_indexed<T>(
    r: &mut Reader,
    mut body: impl FnMut(&mut Reader) -> Option<T>,
) -> Option<BTreeMap<u16, T>> {
    let n = r.u16()? as usize;
    let mut out = BTreeMap::new();
    for _ in 0..n {
        let idx = r.u16()?;
        out.insert(idx, body(r)?);
    }
    Some(out)
}
