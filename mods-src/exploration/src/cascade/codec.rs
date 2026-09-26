//! The memo encoding of a settled cell's outcome.

use mod_sdk::{ByteReader, ByteWriter};

use super::{Feature, Kind};

/// A settled cell's key in the shared memo (the host scopes it to this mod
/// and the world seed).
pub fn memo_key(ours: u8, (lx, ly, lz): (i32, i32, i32)) -> Vec<u8> {
    let mut w = ByteWriter::with_capacity(14);
    w.raw(&[b'c', ours]);
    w.i32x3([lx, ly, lz]);
    w.finish()
}

impl Feature {
    /// The memo value of a settled cell: `None` = no cascade here.
    pub fn encode(feature: Option<&Feature>) -> Vec<u8> {
        let Some(f) = feature else {
            return vec![0];
        };
        let mut w = ByteWriter::with_capacity(Self::encoded_len(
            f.writes.len(),
            f.reserves.len(),
            f.suppressed.len(),
        ));
        w.raw(&[1]);
        w.u32(f.writes.len() as u32);
        for &(p, kind) in &f.writes {
            w.i32x3(p);
            w.raw(&[match kind {
                Kind::Water => 0,
                Kind::Silt => 1,
                Kind::Air => 2,
            }]);
        }
        w.u32(f.reserves.len() as u32);
        for &p in &f.reserves {
            w.i32x3(p);
        }
        w.u32(f.suppressed.len() as u32);
        for &p in &f.suppressed {
            w.i32x3(p);
        }
        w.finish()
    }

    /// The exact [`encode`](Self::encode)d size of a feature with these list
    /// lengths: a tag and three counts, 13 bytes per write, 12 per cell.
    pub fn encoded_len(writes: usize, reserves: usize, suppressed: usize) -> usize {
        13 + 13 * writes + 12 * (reserves + suppressed)
    }

    /// Outer `None` = malformed (recompute); inner `None` = no cascade.
    pub fn decode(bytes: &[u8]) -> Option<Option<Feature>> {
        let mut r = ByteReader::new(bytes);
        match r.take(1)? {
            [0] => return Some(None),
            [1] => {}
            _ => return None,
        }
        let mut writes = Vec::new();
        for _ in 0..r.u32()? {
            let p = r.i32x3()?;
            let kind = match r.take(1)? {
                [0] => Kind::Water,
                [1] => Kind::Silt,
                [2] => Kind::Air,
                _ => return None,
            };
            writes.push((p, kind));
        }
        let mut reserves = Vec::new();
        for _ in 0..r.u32()? {
            reserves.push(r.i32x3()?);
        }
        let mut suppressed = Vec::new();
        for _ in 0..r.u32()? {
            suppressed.push(r.i32x3()?);
        }
        Some(Some(Feature {
            writes,
            reserves,
            suppressed,
            #[cfg(test)]
            wet: Vec::new(),
        }))
    }
}
