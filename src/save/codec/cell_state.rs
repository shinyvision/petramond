//! The unified cell-state list's records: `[len: u8][id_mask: u8][len bytes]`
//! per cell, where each id-masked byte pair is a BLOCK id stored as the
//! world's disk id — the only interpretation this codec ever applies to
//! state bytes.

use petramond_world::block::{ShapeState, SHAPE_STATE_MAX};
use petramond_world::section::CellMap;

use super::{get_indexed, put_u16, put_u8, Reader};
use crate::save::palette::Palette;

/// One cell's record to write.
pub(super) enum Stored<'a> {
    /// A live cell's state; its ids map to disk ids on the way out.
    Live(&'a ShapeState),
    /// A kept cell's record, written back byte for byte.
    Kept(&'a [u8]),
}

impl Stored<'_> {
    pub(super) fn put(&self, buf: &mut Vec<u8>, pal: &Palette) {
        match self {
            Stored::Kept(record) => buf.extend_from_slice(record),
            Stored::Live(state) => {
                let bytes = state.bytes();
                put_u8(buf, bytes.len() as u8);
                put_u8(buf, state.id_mask());
                let mut i = 0;
                while i < bytes.len() {
                    if state.id_mask() & (1 << i) != 0 && i + 1 < bytes.len() {
                        put_u16(buf, pal.block_to_disk(state.id_at(i)));
                        i += 2;
                    } else {
                        put_u8(buf, bytes[i]);
                        i += 1;
                    }
                }
            }
        }
    }
}

/// Read the list as stored: each cell's whole record, untranslated.
pub(super) fn get_stored(r: &mut Reader) -> Option<CellMap<Vec<u8>>> {
    get_indexed(r, |r| {
        let len = r.u8()?;
        let id_mask = r.u8()?;
        if usize::from(len) > SHAPE_STATE_MAX {
            return None;
        }
        let bytes = r.bytes(usize::from(len))?;
        let mut record = Vec::with_capacity(2 + bytes.len());
        record.extend_from_slice(&[len, id_mask]);
        record.extend_from_slice(bytes);
        Some(record)
    })
}

/// A stored record (from [`get_stored`], so well-formed) as live state, its
/// id-masked pairs mapped back through `pal`.
pub(super) fn translate(record: &[u8], pal: &Palette) -> ShapeState {
    let (len, id_mask, stored) = (usize::from(record[0]), record[1], &record[2..]);
    let mut bytes = [0u8; SHAPE_STATE_MAX];
    let mut i = 0;
    while i < len {
        if id_mask & (1 << i) != 0 && i + 1 < len {
            let disk = u16::from_le_bytes([stored[i], stored[i + 1]]);
            let [lo, hi] = ShapeState::id_bytes(pal.block_from_disk(disk));
            bytes[i] = lo;
            bytes[i + 1] = hi;
            i += 2;
        } else {
            bytes[i] = stored[i];
            i += 1;
        }
    }
    ShapeState::with_ids(&bytes[..len], id_mask)
}
