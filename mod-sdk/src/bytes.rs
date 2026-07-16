//! A tiny little-endian byte codec for mod-local records (world/section KV
//! values, client storage blobs): [`ByteWriter`] pushes, [`ByteReader`] reads
//! back with `Option` results — `None` = truncated/malformed input, so decode
//! loops stop (or default) instead of panicking. Wire layout is exactly the
//! pushed bytes in order; nothing is implicit.

/// Append-only little-endian record builder. `finish()` yields the bytes.
#[derive(Default)]
pub struct ByteWriter(Vec<u8>);

impl ByteWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        ByteWriter(Vec::with_capacity(capacity))
    }

    pub fn u16(&mut self, v: u16) {
        self.0.extend(v.to_le_bytes());
    }

    pub fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.0.extend(v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.0.extend(v.to_le_bytes());
    }

    pub fn i32x3(&mut self, v: [i32; 3]) {
        for c in v {
            self.i32(c);
        }
    }

    /// Raw bytes, no prefix — fixed-width fields the reader takes back with
    /// [`ByteReader::take`].
    pub fn raw(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    /// u16-length-prefixed bytes (truncated at `u16::MAX`); read back with
    /// [`ByteReader::blob`].
    pub fn blob(&mut self, bytes: &[u8]) {
        let len = bytes.len().min(u16::MAX as usize);
        self.u16(len as u16);
        self.0.extend_from_slice(&bytes[..len]);
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}

/// Cursor over an encoded record; every read advances past what it consumed.
pub struct ByteReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> ByteReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        ByteReader { bytes, at: 0 }
    }

    /// The next `n` raw bytes.
    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let value = self.bytes.get(self.at..self.at + n)?;
        self.at += n;
        Some(value)
    }

    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    pub fn i32x3(&mut self) -> Option<[i32; 3]> {
        Some([self.i32()?, self.i32()?, self.i32()?])
    }

    /// u16-length-prefixed bytes pushed by [`ByteWriter::blob`].
    pub fn blob(&mut self) -> Option<&'a [u8]> {
        let len = self.u16()? as usize;
        self.take(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reader mirrors the writer field for field, and truncation reads
    /// `None` instead of panicking — the contract every mod codec leans on.
    #[test]
    fn roundtrip_and_truncation() {
        let mut w = ByteWriter::new();
        w.u32(7);
        w.i32x3([-1, 2, i32::MIN]);
        w.raw(&[9, 8, 7]);
        w.blob(b"name");
        w.f32(0.25);
        w.u16(65535);
        let bytes = w.finish();

        let mut r = ByteReader::new(&bytes);
        assert_eq!(r.u32(), Some(7));
        assert_eq!(r.i32x3(), Some([-1, 2, i32::MIN]));
        assert_eq!(r.take(3), Some(&[9, 8, 7][..]));
        assert_eq!(r.blob(), Some(&b"name"[..]));
        assert_eq!(r.f32(), Some(0.25));
        assert_eq!(r.u16(), Some(65535));
        assert_eq!(r.take(1), None);

        for cut in 0..bytes.len() {
            let mut r = ByteReader::new(&bytes[..cut]);
            // Whatever prefix decodes, the first read past the cut is None.
            while r.u32().is_some() {}
            assert!(r.take(4).is_none());
        }
    }
}
