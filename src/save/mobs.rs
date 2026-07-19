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

use crate::mathh::Vec3;
use crate::mob::{Mob, MobTagValue, SavedMob};
use crate::save::codec::{
    get_kv_map, put_f32, put_f64, put_i64, put_kv_map, put_u16, put_u32, put_u8, Reader,
};

/// Fixed bytes per serialized mob: kind(1) + pos(12) + yaw(4) + shear_regrow(4);
/// the variable-length tag map and mod KV map follow. Tag KEYS repeat across
/// mobs (every penned mob carries `petramond:confined`), so one per-list
/// string table holds each distinct key once and every tag stores a u16 index
/// into it instead of the full string.
const MOB_FIXED_BYTES: usize = 21;

/// Tag type discriminators for the mob tag map wire encoding.
const TAG_BOOL: u8 = 0;
const TAG_INT: u8 = 1;
const TAG_FLOAT: u8 = 2;
const TAG_STRING: u8 = 3;

/// Append a `u16`-length-prefixed list of saved mobs to `buf`. The count is capped at
/// `u16::MAX` (a chunk never holds anywhere near that many mobs). A species the
/// active palette has no disk pin for (its mod is disabled for this world) is
/// skipped with a warning: there is no air-mob sentinel, and writing another
/// species' disk id would corrupt the record.
///
/// Layout: count, then (when non-empty) the sorted distinct tag-key string
/// table, then per mob the fixed fields, the tag map as (u16 table index,
/// typed value) pairs, and the mod KV map.
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
    if saved.is_empty() {
        return;
    }
    let distinct: std::collections::BTreeSet<&str> = saved
        .iter()
        .flat_map(|(m, _)| m.tags.keys().map(String::as_str))
        .collect();
    if distinct.len() > u16::MAX as usize {
        // Needs >65k distinct keys in one 16³ section — pathological (mod
        // abuse). Bounded like the other caps here: loud, lossy, never a
        // corrupt record. Tags whose key missed the table are dropped.
        log::warn!(
            "{} distinct mob tag keys in one section record; only the first {} are persisted",
            distinct.len(),
            u16::MAX
        );
    }
    let table: Vec<&str> = distinct.into_iter().take(u16::MAX as usize).collect();
    let index_of: std::collections::HashMap<&str, u16> = table
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, i as u16))
        .collect();
    put_u16(buf, table.len() as u16);
    for key in &table {
        let bytes = key.as_bytes();
        put_u16(buf, bytes.len().min(u16::MAX as usize) as u16);
        buf.extend_from_slice(bytes);
    }
    for (m, disk) in saved {
        put_u8(buf, disk);
        put_f32(buf, m.pos.x);
        put_f32(buf, m.pos.y);
        put_f32(buf, m.pos.z);
        put_f32(buf, m.yaw);
        put_u32(buf, m.shear_regrow);
        put_mob_tags(buf, &m.tags, &index_of);
        put_kv_map(buf, &m.kv);
    }
}

fn put_mob_tags(
    buf: &mut Vec<u8>,
    tags: &std::collections::BTreeMap<String, MobTagValue>,
    index_of: &std::collections::HashMap<&str, u16>,
) {
    // The count is of the tags ACTUALLY written: a key that missed the table
    // (the overflow case above) drops its tag rather than desync the record.
    let writable = tags
        .iter()
        .filter(|(k, _)| index_of.contains_key(k.as_str()))
        .count();
    put_u16(buf, writable.min(u16::MAX as usize) as u16);
    for (k, v) in tags {
        let Some(&index) = index_of.get(k.as_str()) else {
            continue;
        };
        put_u16(buf, index);
        match v {
            MobTagValue::Bool(b) => {
                put_u8(buf, TAG_BOOL);
                put_u8(buf, u8::from(*b));
            }
            MobTagValue::Int(i) => {
                put_u8(buf, TAG_INT);
                put_i64(buf, *i);
            }
            MobTagValue::Float(f) => {
                put_u8(buf, TAG_FLOAT);
                put_f64(buf, *f);
            }
            MobTagValue::String(s) => {
                put_u8(buf, TAG_STRING);
                let bytes = s.as_bytes();
                put_u16(buf, bytes.len().min(u16::MAX as usize) as u16);
                buf.extend_from_slice(bytes);
            }
        }
    }
}

fn get_mob_tags(
    r: &mut Reader,
    table: &[String],
) -> Option<std::collections::BTreeMap<String, MobTagValue>> {
    let n = r.u16()? as usize;
    let mut tags = std::collections::BTreeMap::new();
    for _ in 0..n {
        let key = table.get(r.u16()? as usize)?.clone();
        let tag = match r.u8()? {
            TAG_BOOL => MobTagValue::Bool(r.u8()? != 0),
            TAG_INT => MobTagValue::Int(r.i64()?),
            TAG_FLOAT => MobTagValue::Float(r.f64()?),
            TAG_STRING => {
                let len = r.u16()? as usize;
                let s = std::str::from_utf8(r.bytes(len)?).ok()?.to_string();
                MobTagValue::String(s)
            }
            _ => return None,
        };
        tags.insert(key, tag);
    }
    Some(tags)
}

/// Read a list of saved mobs written by [`put_mobs`]; `None` on truncated or
/// corrupt input (a tag-key index outside the string table included). A
/// disk species the palette (or registry) doesn't know is skipped with a warning —
/// its record bytes are still consumed, so the rest of the list stays intact.
pub fn get_mobs(r: &mut Reader) -> Option<Vec<SavedMob>> {
    let pal = super::palette::active();
    let registered = crate::mob::defs().len();
    let n = r.u16()? as usize;
    if n == 0 {
        return Some(Vec::new());
    }
    let table_len = r.u16()? as usize;
    let mut table = Vec::with_capacity(table_len.min(1024));
    for _ in 0..table_len {
        let len = r.u16()? as usize;
        table.push(std::str::from_utf8(r.bytes(len)?).ok()?.to_string());
    }
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let disk = r.u8()?;
        let pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let yaw = r.f32()?;
        let shear_regrow = r.u32()?;
        let tags = get_mob_tags(r, &table)?;
        let kv = get_kv_map(r)?;
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
            tags,
            kv,
        });
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::Mob;
    use std::collections::BTreeMap;

    #[test]
    fn mobs_roundtrip_through_a_buffer() {
        let a = SavedMob {
            kind: Mob::Owl,
            pos: Vec3::new(1.0, 64.0, 2.0),
            yaw: 1.5,
            shear_regrow: 0,
            tags: BTreeMap::new(),
            kv: BTreeMap::new(),
        };
        let b = SavedMob {
            kind: Mob::Sheep,
            pos: Vec3::new(-3.0, 70.0, 8.0),
            yaw: -0.25,
            shear_regrow: 4321,
            tags: BTreeMap::from([
                (crate::mob::tags::CONFINED.to_owned(), MobTagValue::Bool(true)),
                ("farm:quality".to_owned(), MobTagValue::Int(7)),
            ]),
            kv: BTreeMap::from([
                ("zombies:target".to_owned(), vec![1, 2, 3]),
                ("othermod:tag".to_owned(), Vec::new()),
            ]),
        };
        let mut buf = Vec::new();
        put_mobs(&mut buf, &[a.clone(), b.clone()]);

        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r).expect("decodes");
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[0], a,
            "species, position and facing survive the round-trip"
        );
        assert_eq!(got[1], b, "the shear-regrow counter and mod KV survive too");
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
        put_u16(&mut buf, 0); // empty tag-key string table
        for (kind, x) in [(Mob::Owl.id(), 1.0f32), (200, 2.0), (Mob::Sheep.id(), 3.0)] {
            assert!(kind == 200 || kind < known);
            put_u8(&mut buf, kind);
            for v in [x, 64.0, 2.0, 0.5] {
                put_f32(&mut buf, v);
            }
            put_u32(&mut buf, 7);
            put_u16(&mut buf, 0); // empty tag map
            put_kv_map(
                &mut buf,
                &BTreeMap::from([("strange:mod".to_owned(), vec![9, 9])]),
            );
        }
        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r).expect("decodes despite the stranger");
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
        assert!(get_mobs(&mut r).expect("decodes").is_empty());
    }

    #[test]
    fn a_shared_tag_key_is_stored_once_and_read_back_by_every_mob() {
        // Two mobs carrying the same key must encode the string ONCE (the
        // table) — a regression to per-record strings doubles the bytes.
        let key = crate::mob::tags::CONFINED;
        let penned = |x| SavedMob {
            kind: Mob::Sheep,
            pos: Vec3::new(x, 64.0, 2.0),
            yaw: 0.0,
            shear_regrow: 0,
            tags: BTreeMap::from([(key.to_owned(), MobTagValue::Bool(true))]),
            kv: BTreeMap::new(),
        };
        let mut buf = Vec::new();
        put_mobs(&mut buf, &[penned(1.0), penned(2.0)]);
        assert_eq!(
            buf.windows(key.len())
                .filter(|w| *w == key.as_bytes())
                .count(),
            1,
            "the shared key rides the table, not each mob's record"
        );
        let mut r = Reader::new(&buf);
        let got = get_mobs(&mut r).expect("decodes");
        assert_eq!(got.len(), 2);
        assert!(
            got.iter()
                .all(|m| m.tags.get(key) == Some(&MobTagValue::Bool(true))),
            "both mobs resolve their tag through the shared table"
        );
    }

    #[test]
    fn a_tag_key_index_outside_the_table_is_rejected() {
        // One mob, an EMPTY table, but its tag claims table index 0: the
        // reader must refuse the record, not invent a key or desync.
        let mut buf = Vec::new();
        put_u16(&mut buf, 1); // one mob
        put_u16(&mut buf, 0); // ... with an empty string table
        put_u8(&mut buf, Mob::Owl.id());
        for v in [1.0f32, 64.0, 2.0, 0.5] {
            put_f32(&mut buf, v);
        }
        put_u32(&mut buf, 0); // shear_regrow
        put_u16(&mut buf, 1); // one tag...
        put_u16(&mut buf, 0); // ...keying an EMPTY table
        put_u8(&mut buf, TAG_BOOL);
        put_u8(&mut buf, 1);
        put_kv_map(&mut buf, &BTreeMap::new());
        let mut r = Reader::new(&buf);
        assert!(get_mobs(&mut r).is_none());
    }

    #[test]
    fn truncated_input_is_none() {
        // Claims one mob but provides no body.
        let mut buf = Vec::new();
        put_u16(&mut buf, 1);
        let mut r = Reader::new(&buf);
        assert!(get_mobs(&mut r).is_none());
    }
}
