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
use crate::save::codec::{Reader, Writer};

/// Bytes per serialized mob: kind(1) + pos(12) + yaw(4).
const MOB_BYTES: usize = 17;

/// Append a `u16`-length-prefixed list of saved mobs to `buf`. The count is capped at
/// `u16::MAX` (a chunk never holds anywhere near that many mobs).
pub fn put_mobs(buf: &mut Vec<u8>, mobs: &[SavedMob]) {
    let n = mobs.len().min(u16::MAX as usize);
    buf.reserve(2 + n * MOB_BYTES);
    buf.put_u16(n as u16);
    for m in &mobs[..n] {
        buf.put_u8(m.kind as u8);
        buf.put_f32(m.pos.x);
        buf.put_f32(m.pos.y);
        buf.put_f32(m.pos.z);
        buf.put_f32(m.yaw);
    }
}

/// Read a list of saved mobs written by [`put_mobs`]; `None` on truncated input. An
/// unknown species id falls back to the registry default via [`mob::from_id`], matching
/// how block/item ids decode.
pub fn get_mobs(r: &mut Reader) -> Option<Vec<SavedMob>> {
    let n = r.u16()? as usize;
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let kind = mob::from_id(r.u8()?);
        let pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let yaw = r.f32()?;
        out.push(SavedMob { kind, pos, yaw });
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
        };
        let b = SavedMob {
            kind: Mob::Owl,
            pos: Vec3::new(-3.0, 70.0, 8.0),
            yaw: -0.25,
        };
        let mut buf = Vec::new();
        put_mobs(&mut buf, &[a, b]);

        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r).expect("decodes");
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0], a,
            "species, position and facing survive the round-trip"
        );
        assert_eq!(got[1], b);
    }

    #[test]
    fn empty_list_roundtrips() {
        let mut buf = Vec::new();
        put_mobs(&mut buf, &[]);
        let mut r = Reader::new(&buf);
        assert!(get_mobs(&mut r).expect("decodes").is_empty());
    }

    #[test]
    fn truncated_input_is_none() {
        // Claims one mob but provides no body.
        let mut buf = Vec::new();
        buf.put_u16(1);
        let mut r = Reader::new(&buf);
        assert!(get_mobs(&mut r).is_none());
    }
}
