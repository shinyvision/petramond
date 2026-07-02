//! Per-mob (de)serialization for the saved mobs stored inside a chunk's save
//! record.
//!
//! Mobs persist with their owning chunk: a passive owl saved into its chunk as it
//! unloads reappears when the chunk loads again, exactly like a dropped item-stack.
//! So this is a helper for the chunk codec rather than a standalone file format — a
//! chunk record appends a length-prefixed list of these after its
//! block/biome/water/entity/furnace/chest/torch data (see `save::codec`). Only a mob's
//! persisted projection is stored (species, position, facing); a reloaded mob resumes
//! with a fresh brain.

use crate::mathh::Vec3;
use crate::mob::{self, SavedMob};
use crate::save::codec::{put_f32, put_u16, put_u32, put_u8, Reader};

/// Bytes per serialized mob: kind(1) + pos(12) + yaw(4) + shear_regrow(4). A v1
/// section record predates the shear-regrow field and stores 17-byte mobs (see
/// [`get_mobs`]).
const MOB_BYTES: usize = 21;

/// Append a `u16`-length-prefixed list of saved mobs to `buf`. The count is capped at
/// `u16::MAX` (a chunk never holds anywhere near that many mobs).
pub fn put_mobs(buf: &mut Vec<u8>, mobs: &[SavedMob]) {
    let n = mobs.len().min(u16::MAX as usize);
    buf.reserve(2 + n * MOB_BYTES);
    put_u16(buf, n as u16);
    for m in &mobs[..n] {
        put_u8(buf, m.kind as u8);
        put_f32(buf, m.pos.x);
        put_f32(buf, m.pos.y);
        put_f32(buf, m.pos.z);
        put_f32(buf, m.yaw);
        put_u32(buf, m.shear_regrow);
    }
}

/// Read a list of saved mobs written by [`put_mobs`]; `None` on truncated input. An
/// unknown species id falls back to the registry default via [`mob::from_id`], matching
/// how block/item ids decode. The per-mob layout is fixed-width, so it is versioned by
/// the enclosing section record: `record_version` `1` mobs have no shear-regrow field
/// (they reload fully coated), `2+` do.
pub fn get_mobs(r: &mut Reader, record_version: u8) -> Option<Vec<SavedMob>> {
    let n = r.u16()? as usize;
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let kind = mob::from_id(r.u8()?);
        let pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let yaw = r.f32()?;
        let shear_regrow = if record_version >= 2 { r.u32()? } else { 0 };
        out.push(SavedMob {
            kind,
            pos,
            yaw,
            shear_regrow,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::Mob;

    #[test]
    fn mobs_roundtrip_through_a_buffer() {
        let a = SavedMob {
            kind: Mob::Owl,
            pos: Vec3::new(1.0, 64.0, 2.0),
            yaw: 1.5,
            shear_regrow: 0,
        };
        let b = SavedMob {
            kind: Mob::Sheep,
            pos: Vec3::new(-3.0, 70.0, 8.0),
            yaw: -0.25,
            shear_regrow: 4321,
        };
        let mut buf = Vec::new();
        put_mobs(&mut buf, &[a, b]);

        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r, 2).expect("decodes");
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0], a,
            "species, position and facing survive the round-trip"
        );
        assert_eq!(got[1], b, "the shear-regrow counter survives too");
    }

    #[test]
    fn v1_records_decode_without_a_shear_field() {
        // A v1 section record stored 17-byte mobs (no shear-regrow). Hand-write that
        // old layout and decode it as record version 1: the mob reloads fully coated.
        let mut buf = Vec::new();
        put_u16(&mut buf, 1);
        put_u8(&mut buf, Mob::Sheep as u8);
        for v in [1.0f32, 64.0, 2.0, 0.5] {
            crate::save::codec::put_f32(&mut buf, v);
        }
        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r, 1).expect("v1 layout decodes");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, Mob::Sheep);
        assert_eq!(got[0].shear_regrow, 0, "an old record reloads coated");
    }

    #[test]
    fn empty_list_roundtrips() {
        let mut buf = Vec::new();
        put_mobs(&mut buf, &[]);
        let mut r = Reader::new(&buf);
        assert!(get_mobs(&mut r, 2).expect("decodes").is_empty());
    }

    #[test]
    fn truncated_input_is_none() {
        // Claims one mob but provides no body.
        let mut buf = Vec::new();
        put_u16(&mut buf, 1);
        let mut r = Reader::new(&buf);
        assert!(get_mobs(&mut r, 2).is_none());
    }
}
