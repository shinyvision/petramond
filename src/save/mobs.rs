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
//!
//! Species ids are stored as the SAVE's ids (see [`super::palette`]) so mod packs
//! renumbering the mob registry can't corrupt old worlds. A disk id whose palette
//! name this build doesn't know is SKIPPED with a warning — there is no "air mob"
//! to degrade to, and respawning a wrong species would corrupt the world.

use std::collections::BTreeMap;

use crate::mathh::Vec3;
use crate::mob::{Mob, SavedMob};
use crate::save::codec::{get_kv_map, put_f32, put_kv_map, put_u16, put_u32, put_u8, Reader};

/// Fixed bytes per serialized mob: kind(1) + pos(12) + yaw(4) + shear_regrow(4);
/// a section-record-v3 mob appends its variable-length mod KV map after them
/// (a v3 mob with no KV is these bytes + the map's 2-byte zero count). A v1
/// section record predates the shear-regrow field and stores 17-byte mobs; v2
/// predates the KV map (see [`get_mobs`]).
const MOB_FIXED_BYTES: usize = 21;

/// Append a `u16`-length-prefixed list of saved mobs to `buf`. The count is capped at
/// `u16::MAX` (a chunk never holds anywhere near that many mobs). A species the
/// active palette has no disk pin for (its mod is disabled for this world) is
/// skipped with a warning: there is no air-mob sentinel, and writing another
/// species' disk id would corrupt the record.
pub fn put_mobs(buf: &mut Vec<u8>, mobs: &[SavedMob]) {
    let pal = super::palette::active();
    let capped = &mobs[..mobs.len().min(u16::MAX as usize)];
    let saved: Vec<(&SavedMob, u8)> = capped
        .iter()
        .filter_map(|m| match pal.mob_to_disk(m.kind.id()) {
            Some(disk) => Some((m, disk)),
            None => {
                log::warn!(
                    "mob {:?} at {:?} has no save-palette pin (disabled mod?); not persisted",
                    m.kind,
                    m.pos
                );
                None
            }
        })
        .collect();
    buf.reserve(2 + saved.len() * (MOB_FIXED_BYTES + 2));
    put_u16(buf, saved.len() as u16);
    for (m, disk) in saved {
        put_u8(buf, disk);
        put_f32(buf, m.pos.x);
        put_f32(buf, m.pos.y);
        put_f32(buf, m.pos.z);
        put_f32(buf, m.yaw);
        put_u32(buf, m.shear_regrow);
        put_kv_map(buf, &m.kv);
    }
}

/// Read a list of saved mobs written by [`put_mobs`]; `None` on truncated input. A
/// disk species the palette (or registry) doesn't know is skipped with a warning —
/// its record bytes are still consumed, so the rest of the list stays intact.
/// The per-mob layout is versioned by the enclosing section record:
/// `record_version` `1` mobs have no shear-regrow field (they reload fully coated),
/// `2` no mod KV map (they reload with none), `3+` both.
pub fn get_mobs(r: &mut Reader, record_version: u8) -> Option<Vec<SavedMob>> {
    let pal = super::palette::active();
    let registered = crate::mob::defs().len();
    let n = r.u16()? as usize;
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let disk = r.u8()?;
        let pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let yaw = r.f32()?;
        let shear_regrow = if record_version >= 2 { r.u32()? } else { 0 };
        let kv = if record_version >= 3 {
            get_kv_map(r)?
        } else {
            BTreeMap::new()
        };
        // Resolve the species AFTER consuming the record bytes, so a skip can't
        // desync the reader.
        let kind = pal
            .mob_from_disk(disk)
            .filter(|&id| (id as usize) < registered);
        let Some(id) = kind else {
            log::warn!(
                "saved mob with unknown species (disk id {disk}) at {pos:?} skipped — \
                 was this world last played on a newer or modded build?"
            );
            continue;
        };
        out.push(SavedMob {
            kind: Mob(id),
            pos,
            yaw,
            shear_regrow,
            kv,
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
            kv: BTreeMap::new(),
        };
        let b = SavedMob {
            kind: Mob::Sheep,
            pos: Vec3::new(-3.0, 70.0, 8.0),
            yaw: -0.25,
            shear_regrow: 4321,
            kv: BTreeMap::from([
                ("zombies:target".to_owned(), vec![1, 2, 3]),
                ("othermod:tag".to_owned(), Vec::new()),
            ]),
        };
        let mut buf = Vec::new();
        put_mobs(&mut buf, &[a.clone(), b.clone()]);

        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r, 3).expect("decodes");
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0], a,
            "species, position and facing survive the round-trip"
        );
        assert_eq!(got[1], b, "the shear-regrow counter and mod KV survive too");
    }

    #[test]
    fn v1_records_decode_without_a_shear_field() {
        // A v1 section record stored 17-byte mobs (no shear-regrow). Hand-write that
        // old layout and decode it as record version 1: the mob reloads fully coated.
        let mut buf = Vec::new();
        put_u16(&mut buf, 1);
        put_u8(&mut buf, Mob::Sheep.id());
        for v in [1.0f32, 64.0, 2.0, 0.5] {
            crate::save::codec::put_f32(&mut buf, v);
        }
        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r, 1).expect("v1 layout decodes");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, Mob::Sheep);
        assert_eq!(got[0].shear_regrow, 0, "an old record reloads coated");
        assert!(got[0].kv.is_empty(), "an old record reloads with no mod KV");
    }

    #[test]
    fn v2_records_decode_without_a_kv_map() {
        // A v2 section record stored 21-byte mobs (shear-regrow, no mod KV).
        // Hand-write that layout and decode it as record version 2: the mob
        // reloads with the field defaulted empty.
        let mut buf = Vec::new();
        put_u16(&mut buf, 1);
        put_u8(&mut buf, Mob::Sheep.id());
        for v in [1.0f32, 64.0, 2.0, 0.5] {
            put_f32(&mut buf, v);
        }
        put_u32(&mut buf, 77);
        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r, 2).expect("v2 layout decodes");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].shear_regrow, 77, "v2 fields still decode");
        assert!(got[0].kv.is_empty(), "a v2 record reloads with no mod KV");
    }

    #[test]
    fn an_unknown_species_is_skipped_without_desyncing_the_list() {
        // Hand-write three records where the middle one carries a species id no
        // registry entry backs (the identity palette maps it through unchanged).
        // The reader must skip exactly that mob and keep the other two intact —
        // INCLUDING consuming the stranger's variable-length KV map.
        let known = crate::mob::defs().len() as u8;
        let mut buf = Vec::new();
        put_u16(&mut buf, 3);
        for (kind, x) in [(Mob::Owl.id(), 1.0f32), (200, 2.0), (Mob::Sheep.id(), 3.0)] {
            assert!(kind == 200 || kind < known);
            put_u8(&mut buf, kind);
            for v in [x, 64.0, 2.0, 0.5] {
                put_f32(&mut buf, v);
            }
            put_u32(&mut buf, 7);
            put_kv_map(
                &mut buf,
                &BTreeMap::from([("strange:mod".to_owned(), vec![9, 9])]),
            );
        }
        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r, 3).expect("decodes despite the stranger");
        assert_eq!(got.len(), 2, "only the unknown mob is dropped");
        assert_eq!(got[0].kind, Mob::Owl);
        assert_eq!(got[0].pos.x, 1.0);
        assert_eq!(got[1].kind, Mob::Sheep);
        assert_eq!(
            got[1].pos.x, 3.0,
            "records after the skipped mob decode from the right offset"
        );
    }

    #[test]
    fn empty_list_roundtrips() {
        let mut buf = Vec::new();
        put_mobs(&mut buf, &[]);
        let mut r = Reader::new(&buf);
        assert!(get_mobs(&mut r, 3).expect("decodes").is_empty());
    }

    #[test]
    fn truncated_input_is_none() {
        // Claims one mob but provides no body.
        let mut buf = Vec::new();
        put_u16(&mut buf, 1);
        let mut r = Reader::new(&buf);
        assert!(get_mobs(&mut r, 3).is_none());
    }
}
