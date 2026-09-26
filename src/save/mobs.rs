//! Per-mob (de)serialization for the saved mobs stored inside a section's
//! save record.
//!
//! Mobs persist with their owning section: a passive owl saved into its
//! section as it unloads reappears when the section loads again, exactly
//! like a dropped item-stack. So this is a helper for the section codec
//! rather than a standalone file format (see `save::codec`). Only a mob's
//! persisted projection is stored (species, position, facing, tags, carried
//! slots); a reloaded mob resumes with a fresh brain.
//!
//! Layout: one [`MobList`] — the list's distinct tag KEYS once (every penned
//! mob carries `petramond:confined`; storing each key once keeps a herd's
//! record small), then each mob as a tagged record (`save::wire`) whose tags
//! index that table. Species ids are the SAVE's ids (see
//! [`super::palette`]) so mod packs renumbering the mob registry can't
//! corrupt old worlds.
//!
//! A mob this build cannot represent — its species cannot be resolved (a
//! removed or disabled mod's mob), or its record carries fields this build
//! does not know — never becomes a live mob and is never dropped: it
//! decodes to a [`DiskMob`], which the save keeps beside its section and
//! writes back unchanged, so the mob returns with its mod.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::mob::{Mob, MobTagValue, SavedMob};
use crate::save::codec::DiskSlot;
use crate::save::palette::Palette;
use crate::save::wire::{tagged_record, wire_struct, UnknownFields, Wire};
use petramond_math::world_pos::WorldPos;
use petramond_persist::bytecodec::{put_u8, Reader};
use petramond_world::container::{Container, MAX_CONTAINER_SLOTS};

/// Tag type discriminators for a mob tag value on the wire.
const TAG_BOOL: u8 = 0;
const TAG_INT: u8 = 1;
const TAG_FLOAT: u8 = 2;
const TAG_STRING: u8 = 3;

/// `[type: u8][value]`; a string may hold a full KV value (64 KiB), so it
/// carries a `u32` length.
impl Wire for MobTagValue {
    fn put(&self, buf: &mut Vec<u8>) {
        match self {
            MobTagValue::Bool(b) => {
                put_u8(buf, TAG_BOOL);
                b.put(buf);
            }
            MobTagValue::Int(i) => {
                put_u8(buf, TAG_INT);
                i.put(buf);
            }
            MobTagValue::Float(f) => {
                put_u8(buf, TAG_FLOAT);
                f.put(buf);
            }
            MobTagValue::String(s) => {
                put_u8(buf, TAG_STRING);
                s.put(buf);
            }
        }
    }

    fn get(r: &mut Reader) -> Option<Self> {
        Some(match r.u8()? {
            TAG_BOOL => MobTagValue::Bool(bool::get(r)?),
            TAG_INT => MobTagValue::Int(i64::get(r)?),
            TAG_FLOAT => MobTagValue::Float(f64::get(r)?),
            TAG_STRING => MobTagValue::String(String::get(r)?),
            _ => return None,
        })
    }
}

/// One mob as stored; its tags index the list's key table.
struct MobRecord {
    species: u8,
    pos: WorldPos,
    yaw: f32,
    tags: Vec<(u16, MobTagValue)>,
    slots: Vec<DiskSlot>,
    unknown: UnknownFields,
}
tagged_record!(MobRecord {
    1 => species,
    2 => pos,
    3 => yaw,
    4 => tags,
    5 => slots,
});

/// The mobs payload of a section record.
struct MobList {
    keys: Vec<String>,
    mobs: Vec<MobRecord>,
}
wire_struct!(MobList { keys, mobs });

/// A mob as stored, its tag keys resolved: the form a mob this build cannot
/// represent is kept in (see the module docs). Its species is the world's
/// DISK id.
#[derive(Clone, Debug, PartialEq)]
pub struct DiskMob {
    pub species: u8,
    pub pos: WorldPos,
    pub yaw: f32,
    pub tags: BTreeMap<String, MobTagValue>,
    pub slots: Vec<DiskSlot>,
    pub unknown: UnknownFields,
}

impl DiskMob {
    fn of(mob: &SavedMob, species: u8, pal: &Palette) -> Self {
        Self {
            species,
            pos: mob.pos,
            yaw: mob.yaw,
            tags: mob.tags.clone(),
            slots: mob
                .container
                .slots
                .iter()
                .take(MAX_CONTAINER_SLOTS)
                .map(|slot| DiskSlot::of(*slot, pal))
                .collect(),
            unknown: UnknownFields::new(),
        }
    }

    /// The live mob, or the mob itself when this build cannot represent it.
    /// A carried slot whose item cannot be resolved keeps the whole mob in
    /// disk form too, rather than losing the item.
    fn resolve(self, pal: &Palette) -> Result<SavedMob, DiskMob> {
        let registered = crate::mob::defs().len();
        let species = pal
            .mob_from_disk(self.species)
            .filter(|&id| (id as usize) < registered);
        let Some(id) = species.filter(|_| self.unknown.is_empty()) else {
            return Err(self);
        };
        let slots: Option<Vec<_>> = self
            .slots
            .iter()
            .map(|slot| slot.clone().resolve(pal).ok())
            .collect();
        let Some(slots) = slots else {
            return Err(self);
        };
        Ok(SavedMob {
            kind: Mob(id),
            pos: self.pos,
            yaw: self.yaw,
            tags: self.tags,
            container: Container { slots },
        })
    }
}

/// Append the mobs payload: the live mobs, then the mobs kept from an
/// earlier load. A live species the palette has no disk pin for (its mod is
/// disabled for this world, so it cannot have been loaded — only spawned
/// against the rules) is skipped with a warning: writing another species'
/// disk id would corrupt the record.
pub fn put_mobs(buf: &mut Vec<u8>, mobs: &[SavedMob], kept: &[DiskMob], pal: &Palette) {
    let live = mobs.iter().filter_map(|m| match pal.mob_to_disk(m.kind.id()) {
        Some(species) => Some(DiskMob::of(m, species, pal)),
        None => {
            log::warn!(
                "mob {:?} at {:?} has no save-palette pin (disabled mod?); not persisted",
                m.kind,
                m.pos
            );
            None
        }
    });
    let all: Vec<DiskMob> = live.chain(kept.iter().cloned()).collect();
    let distinct: BTreeSet<&str> = all
        .iter()
        .flat_map(|m| m.tags.keys().map(String::as_str))
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
    let keys: Vec<&str> = distinct.into_iter().take(u16::MAX as usize).collect();
    let index_of: HashMap<&str, u16> = keys
        .iter()
        .enumerate()
        .map(|(i, k)| (*k, i as u16))
        .collect();
    let records = all
        .iter()
        .map(|m| MobRecord {
            species: m.species,
            pos: m.pos,
            yaw: m.yaw,
            tags: m
                .tags
                .iter()
                .filter_map(|(k, v)| Some((*index_of.get(k.as_str())?, v.clone())))
                .collect(),
            slots: m.slots.clone(),
            unknown: m.unknown.clone(),
        })
        .collect();
    MobList {
        keys: keys.into_iter().map(str::to_owned).collect(),
        mobs: records,
    }
    .put(buf);
}

/// What a mobs payload holds: the mobs this build can spawn, and the ones
/// it must keep in disk form.
pub struct DecodedMobs {
    pub live: Vec<SavedMob>,
    pub kept: Vec<DiskMob>,
}

/// Read the mobs payload. `None` on truncated or corrupt input, a tag-key
/// index outside the table included.
pub fn get_mobs(r: &mut Reader, pal: &Palette) -> Option<DecodedMobs> {
    let list = MobList::get(r)?;
    let mut out = DecodedMobs {
        live: Vec::with_capacity(list.mobs.len()),
        kept: Vec::new(),
    };
    for record in list.mobs {
        let mut tags = BTreeMap::new();
        for (index, value) in record.tags {
            tags.insert(list.keys.get(index as usize)?.clone(), value);
        }
        let mob = DiskMob {
            species: record.species,
            pos: record.pos,
            yaw: record.yaw,
            tags,
            slots: record.slots,
            unknown: record.unknown,
        };
        match mob.resolve(pal) {
            Ok(live) => out.live.push(live),
            Err(kept) => {
                log::info!(
                    "saved mob with species disk id {} at {:?} is kept but not spawned — was \
                     this world last played with a mod that is missing or disabled now?",
                    kept.species,
                    kept.pos
                );
                out.kept.push(kept);
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests;
