//! Registry id remapping at the TCP transport boundary.
//!
//! Dynamic block/item/mob/sound/effect/biome ids are assigned per PROCESS at
//! load, so a client's ids need not match the server's (the client may have
//! more mods installed than the server enables). At join the server sends its
//! name tables in server-id order ([`NameTables`]); the client builds dense
//! server-id→client-id LUTs here and rewrites every inbound message right
//! after decode (and outbound before encode) on the transport threads.
//! Everything above the transport speaks client-local ids; the LOCAL
//! connection is identity and skips this module entirely.
//!
//! The rewrite is DERIVED FROM THE TYPES: every wire type implements
//! [`Remap`], and every struct impl destructures its value EXHAUSTIVELY (no
//! `..`), naming each id-bearing field's rewrite and binding each id-free
//! field to `_` with the reason. A field added to any wire type therefore
//! fails compilation in its `Remap` impl until its id story is decided — a
//! new id can never ship raw to a client whose registries differ. The impls
//! live in [`wire`].
//!
//! A server name unknown to the client can only be a server-side DISABLED
//! mod's registered residue (the handshake guarantees enabled mods are
//! installed): blocks map to air, biomes to the unregistered-id fallback,
//! items/mobs/sounds/effects to MISSING (the consumer skips), each with one
//! warning — the palette's unknown-name semantics, never a rejection.

use super::protocol::{ClientToServer, NameTables, ServerToClient};
use crate::player::animator::AnimatorNames;
use crate::player::RigId;

mod wire;

#[cfg(test)]
mod tests;

/// LUT entry for "the client doesn't know this name". Registry ids are `u16`
/// and the tables are dense, so a sentinel VALUE would collide with a real id;
/// entries are `Option`-shaped instead, which niche-packs to the same size.
pub const MISSING: Option<u16> = None;

/// A wire value whose registry ids the transport rewrites into this process's
/// ids. `false` = the value names something this client lacks and its
/// container drops it (skip semantics: an unknown mob row, effect, or sound
/// event); a value that can always stand (a player row whose held item is
/// unknown reads as an empty hand) degrades in place and returns `true`.
pub trait Remap {
    fn remap(&mut self, map: &IdRemap) -> bool;
}

impl<T: Remap> Remap for Vec<T> {
    fn remap(&mut self, map: &IdRemap) -> bool {
        self.retain_mut(|v| v.remap(map));
        true
    }
}

/// An optional value the client cannot name reads as absent (an unknown
/// inventory item is an empty slot).
impl<T: Remap> Remap for Option<T> {
    fn remap(&mut self, map: &IdRemap) -> bool {
        if self.as_mut().is_some_and(|v| !v.remap(map)) {
            *self = None;
        }
        true
    }
}

impl<T: Remap + ?Sized> Remap for Box<T> {
    fn remap(&mut self, map: &IdRemap) -> bool {
        (**self).remap(map)
    }
}

/// A shared run of rows. The batch arrives decoded, so this process is the
/// sole owner and the run is rewritten in place; only a run that loses rows
/// (or one still shared, which the transport never hands over) is rebuilt.
impl<T: Remap + Clone> Remap for std::sync::Arc<[T]> {
    fn remap(&mut self, map: &IdRemap) -> bool {
        let keep: Vec<bool> = match std::sync::Arc::get_mut(self) {
            Some(rows) => rows.iter_mut().map(|row| row.remap(map)).collect(),
            None => {
                let mut rows = self.to_vec();
                rows.retain_mut(|row| row.remap(map));
                *self = rows.into();
                return true;
            }
        };
        if keep.iter().all(|&k| k) {
            return true;
        }
        *self = self
            .iter()
            .zip(keep)
            .filter(|(_, k)| *k)
            .map(|(row, _)| row.clone())
            .collect();
        true
    }
}

/// One rig graph's vocabulary tables, server id → this process's id.
#[derive(Debug, Default)]
struct AnimatorLut {
    clips: Vec<Option<u16>>,
    params: Vec<Option<u16>>,
    slots: Vec<Option<u16>>,
    events: Vec<Option<u16>>,
}

impl AnimatorLut {
    fn build(server: &AnimatorNames, local: &AnimatorNames) -> Self {
        let table = |server: &[String], mine: &[String], what: &str| {
            build_lut(server, what, |n| {
                mine.iter().position(|m| m == n).map(|i| i as u16)
            })
        };
        Self {
            clips: table(&server.clips, &local.clips, "animator clip"),
            params: table(&server.params, &local.params, "animator param"),
            slots: table(&server.slots, &local.slots, "animator slot"),
            events: table(&server.events, &local.events, "animator event"),
        }
    }

    fn is_identity(&self) -> bool {
        [&self.clips, &self.params, &self.slots, &self.events]
            .into_iter()
            .all(|t| is_identity_lut(t))
    }
}

/// Dense server-id → client-id lookup tables.
#[derive(Debug)]
pub struct IdRemap {
    /// Blocks: unknown maps to air (0) — a cell must still hold SOMETHING.
    blocks: Vec<u16>,
    /// Surface biomes (index = server biome id; id 0 is unassigned and maps
    /// to itself): unknown maps to the unregistered-id fallback, exactly what
    /// `Biome::from_id` reads an unregistered id as — a column must still
    /// hold something.
    biomes: Vec<u8>,
    items: Vec<Option<u16>>,
    /// The inverse of `items` (index = THIS process's item id), for the few
    /// client→server messages that name an item.
    items_to_server: Vec<Option<u16>>,
    mobs: Vec<Option<u16>>,
    sounds: Vec<Option<u16>>,
    effects: Vec<Option<u16>>,
    emitters: Vec<Option<u16>>,
    conditions: Vec<Option<u16>>,
    /// Per SERVER rig id: this process's rig of the same name and its
    /// graph's clips, params, slots and events; `None` for a rig this
    /// process never registered (its rows drop).
    animators: Vec<Option<(RigId, AnimatorLut)>>,
    /// True when every table is the identity — the fast path (a client whose
    /// registries happen to match the server's exactly).
    identity: bool,
}

impl IdRemap {
    /// Build the LUTs from the server's tables against THIS process's loaded
    /// registries.
    pub fn build(tables: &NameTables) -> IdRemap {
        let names = petramond_world::registry::names();
        let blocks: Vec<u16> = tables
            .blocks
            .iter()
            .map(|n| match names.blocks.id(n) {
                Some(id) => id,
                None => {
                    log::warn!("remap: unknown server block '{n}' maps to air");
                    petramond_world::block::Block::Air.0
                }
            })
            .collect();
        let biomes = Self::biome_lut(&tables.biomes, |key| {
            petramond_world::biome::Biome::from_name(key).map(|b| b.id())
        });
        let items = build_lut(&tables.items, "item", |n| names.items.id(n));
        // The mob wire vocabulary is `MobDef::key` (not the registry name), so
        // the mob name table can't answer it; a one-shot hash join keeps this
        // O(server ids + species) instead of a per-id linear scan.
        let mob_ids: std::collections::HashMap<&str, u16> = crate::mob::defs()
            .iter()
            .enumerate()
            .map(|(id, d)| (d.key, id as u16))
            .collect();
        let mobs = build_lut(&tables.mobs, "mob", |n| mob_ids.get(n).copied());
        let sounds = build_lut(&tables.sounds, "sound", |n| {
            petramond_world::sound_registry::by_name(n).map(|s| s.0 as u16)
        });
        let effects = build_lut(&tables.effects, "effect", |n| {
            petramond_world::effect::by_name(n).map(|e| e.0 as u16)
        });
        let emitters = build_lut(&tables.emitters, "emitter", |n| {
            petramond_world::particle_emitters::by_key(n).map(|b| b.id as u16)
        });
        let conditions = build_lut(&tables.conditions, "condition", |n| {
            petramond_world::condition::by_name(n).map(|c| c.0 as u16)
        });
        let animators = Self::animator_luts(&tables.animators, &AnimatorNames::all());
        Self::assemble(RemapTables {
            blocks,
            biomes,
            items,
            mobs,
            sounds,
            effects,
            emitters,
            conditions,
            animators,
        })
    }

    /// Finish a remap from its forward tables: derive the inverse item table
    /// and the identity fast-path flag.
    fn assemble(t: RemapTables) -> IdRemap {
        let mut items_to_server: Vec<Option<u16>> = Vec::new();
        for (server, local) in t.items.iter().enumerate() {
            if let Some(local) = *local {
                let at = usize::from(local);
                if items_to_server.len() <= at {
                    items_to_server.resize(at + 1, MISSING);
                }
                items_to_server[at] = Some(server as u16);
            }
        }
        let identity = t.blocks.iter().enumerate().all(|(i, &v)| i == v as usize)
            && t.biomes.iter().enumerate().all(|(i, &v)| i == v as usize)
            && [
                &t.items,
                &t.mobs,
                &t.sounds,
                &t.effects,
                &t.emitters,
                &t.conditions,
            ]
            .into_iter()
            .all(|table| is_identity_lut(table))
            && t.animators.iter().enumerate().all(|(i, a)| {
                a.as_ref()
                    .is_some_and(|(rig, lut)| rig.index() == i && lut.is_identity())
            });
        IdRemap {
            blocks: t.blocks,
            biomes: t.biomes,
            items: t.items,
            items_to_server,
            mobs: t.mobs,
            sounds: t.sounds,
            effects: t.effects,
            emitters: t.emitters,
            conditions: t.conditions,
            animators: t.animators,
            identity,
        }
    }

    /// The biome table: server id → this process's id by registry KEY. Id 0
    /// is unassigned on both sides; an unknown key maps to the id every
    /// unregistered biome reads as.
    fn biome_lut(server: &[String], resolve: impl Fn(&str) -> Option<u8>) -> Vec<u8> {
        let fallback = petramond_world::biome::Biome::from_id(0).id();
        server
            .iter()
            .enumerate()
            .map(|(id, key)| {
                if id == 0 {
                    return 0;
                }
                resolve(key).unwrap_or_else(|| {
                    log::warn!("remap: unknown server biome '{key}' maps to the fallback biome");
                    fallback
                })
            })
            .collect()
    }

    /// The per-rig tables: each server rig joined BY NAME to this process's
    /// rig; an unknown name maps to nothing (one warning).
    fn animator_luts(
        server: &[AnimatorNames],
        local: &[AnimatorNames],
    ) -> Vec<Option<(RigId, AnimatorLut)>> {
        server
            .iter()
            .map(|names| {
                let Some(at) = local.iter().position(|l| l.rig == names.rig) else {
                    log::warn!(
                        "remap: unknown server rig '{}' drops its animator rows",
                        names.rig
                    );
                    return None;
                };
                Some((RigId(at as u16), AnimatorLut::build(names, &local[at])))
            })
            .collect()
    }

    #[inline]
    #[allow(dead_code)] // the identity fast path reads the field; tests read this
    pub fn is_identity(&self) -> bool {
        self.identity
    }

    #[inline]
    pub fn block(&self, server_id: u16) -> u16 {
        self.blocks
            .get(server_id as usize)
            .copied()
            .unwrap_or(petramond_world::block::Block::Air.0)
    }

    /// A server biome id as this process's; past the table reads as the
    /// unregistered-id fallback, like an unknown key.
    #[inline]
    pub fn biome(&self, server_id: u8) -> u8 {
        self.biomes
            .get(usize::from(server_id))
            .copied()
            .unwrap_or_else(|| petramond_world::biome::Biome::from_id(0).id())
    }

    #[inline]
    pub fn item(&self, server_id: u16) -> Option<u16> {
        lookup(&self.items, server_id as usize)
    }

    /// This process's item id as the SERVER's (client→server direction).
    #[inline]
    pub fn item_to_server(&self, local_id: u16) -> Option<u16> {
        lookup(&self.items_to_server, local_id as usize)
    }

    #[inline]
    pub fn mob(&self, server_id: u8) -> Option<u8> {
        lookup(&self.mobs, server_id as usize).map(|id| id as u8)
    }

    #[inline]
    pub fn sound(&self, server_id: u8) -> Option<u8> {
        lookup(&self.sounds, server_id as usize).map(|id| id as u8)
    }

    #[inline]
    pub fn effect(&self, server_id: u8) -> Option<u8> {
        lookup(&self.effects, server_id as usize).map(|id| id as u8)
    }

    #[inline]
    pub fn emitter(&self, server_id: u8) -> Option<u8> {
        lookup(&self.emitters, server_id as usize).map(|id| id as u8)
    }

    #[inline]
    pub fn condition(&self, server_id: u8) -> Option<u8> {
        lookup(&self.conditions, server_id as usize).map(|id| id as u8)
    }

    /// Rewrite one id in place through `lookup`; `false` = unknown (the
    /// caller drops what carries it).
    fn rewrite<T: Copy>(slot: &mut T, lookup: impl FnOnce(T) -> Option<T>) -> bool {
        match lookup(*slot) {
            Some(local) => {
                *slot = local;
                true
            }
            None => false,
        }
    }

    /// An optional item id: an unknown one reads as absent (an empty hand).
    fn optional_item(&self, slot: &mut Option<u16>) {
        *slot = slot.and_then(|id| self.item(id));
    }

    /// This process's rig and tables for a server rig id; `None` for a rig
    /// this process lacks.
    fn animator(&self, rig: RigId) -> Option<(RigId, &AnimatorLut)> {
        self.animators
            .get(rig.index())?
            .as_ref()
            .map(|(rig, lut)| (*rig, lut))
    }

    /// A server rig's fired event as this process's `(rig, event)`; `None`
    /// when either is unknown here.
    fn animator_event(&self, rig: RigId, event: u16) -> Option<(RigId, u16)> {
        let (rig, lut) = self.animator(rig)?;
        Some((rig, lookup(&lut.events, event as usize)?))
    }

    /// Rewrite one fired graph event in place; `false` = unknown here.
    fn remap_animator_event(&self, rig: &mut RigId, event: &mut u16) -> bool {
        match self.animator_event(*rig, *event) {
            Some((local_rig, local)) => {
                *rig = local_rig;
                *event = local;
                true
            }
            None => false,
        }
    }

    /// Rewrite a freshly-decoded server message to client-local ids, in place.
    pub fn remap_to_client(&self, msg: &mut ServerToClient) {
        if self.identity {
            return;
        }
        msg.remap(self);
    }

    /// Rewrite an outbound client message to server-local ids.
    pub fn remap_to_server(&self, msg: &mut ClientToServer) {
        if self.identity {
            return;
        }
        wire::remap_to_server(self, msg);
    }
}

/// The forward tables an [`IdRemap`] is assembled from.
struct RemapTables {
    blocks: Vec<u16>,
    biomes: Vec<u8>,
    items: Vec<Option<u16>>,
    mobs: Vec<Option<u16>>,
    sounds: Vec<Option<u16>>,
    effects: Vec<Option<u16>>,
    emitters: Vec<Option<u16>>,
    conditions: Vec<Option<u16>>,
    animators: Vec<Option<(RigId, AnimatorLut)>>,
}

/// THIS process's registry names, in id order — what a server sends as its
/// wire vocabulary at join.
pub fn local_name_tables() -> NameTables {
    let names = petramond_world::registry::names();
    NameTables {
        blocks: (0..names.blocks.len())
            .map(|i| {
                names
                    .blocks
                    .name(i as u16)
                    .expect("dense table")
                    .to_string()
            })
            .collect(),
        // Index = biome id; id 0 is unassigned (an empty key).
        biomes: std::iter::once(String::new())
            .chain(petramond_world::biome::Biome::all().map(|b| b.key().to_string()))
            .collect(),
        items: (0..names.items.len())
            .map(|i| names.items.name(i as u16).expect("dense table").to_string())
            .collect(),
        mobs: crate::mob::Mob::all()
            .iter()
            .map(|m| crate::mob::def(*m).key.to_string())
            .collect(),
        sounds: petramond_world::sound_registry::defs()
            .iter()
            .map(|d| d.name.to_string())
            .collect(),
        effects: petramond_world::effect::Effect::all()
            .map(|e| e.def().name.to_string())
            .collect(),
        emitters: petramond_world::particle_emitters::defs()
            .iter()
            .map(|b| b.key.to_string())
            .collect(),
        animators: AnimatorNames::all(),
        conditions: petramond_world::condition::defs()
            .iter()
            .map(|c| c.name.to_string())
            .collect(),
    }
}

fn build_lut(
    server: &[String],
    what: &str,
    resolve: impl Fn(&str) -> Option<u16>,
) -> Vec<Option<u16>> {
    server
        .iter()
        .map(|n| match resolve(n) {
            Some(id) => Some(id),
            None => {
                log::warn!("remap: unknown server {what} '{n}' will be skipped");
                MISSING
            }
        })
        .collect()
}

fn is_identity_lut(table: &[Option<u16>]) -> bool {
    table
        .iter()
        .enumerate()
        .all(|(i, &v)| v == Some(i as u16))
}

#[inline]
fn lookup(table: &[Option<u16>], server_id: usize) -> Option<u16> {
    table.get(server_id).copied().flatten()
}
