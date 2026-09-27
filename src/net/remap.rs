use super::protocol::{ClientToServer, NameTables, ServerToClient};
use crate::player::animator::AnimatorNames;
use crate::player::RigId;

mod wire;

#[cfg(test)]
mod tests;

pub const MISSING: Option<u16> = None;

pub trait Remap {
    fn remap(&mut self, map: &IdRemap) -> bool;
}

impl<T: Remap> Remap for Vec<T> {
    fn remap(&mut self, map: &IdRemap) -> bool {
        self.retain_mut(|v| v.remap(map));
        true
    }
}

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

#[derive(Debug)]
pub struct IdRemap {
    blocks: Vec<u16>,
    biomes: Vec<u8>,
    items: Vec<Option<u16>>,
    items_to_server: Vec<Option<u16>>,
    mobs: Vec<Option<u16>>,
    sounds: Vec<Option<u16>>,
    effects: Vec<Option<u16>>,
    emitters: Vec<Option<u16>>,
    conditions: Vec<Option<u16>>,
    animators: Vec<Option<(RigId, AnimatorLut)>>,
    identity: bool,
}

impl IdRemap {
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
    #[allow(dead_code)]
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

    fn rewrite<T: Copy>(slot: &mut T, lookup: impl FnOnce(T) -> Option<T>) -> bool {
        match lookup(*slot) {
            Some(local) => {
                *slot = local;
                true
            }
            None => false,
        }
    }

    pub(crate) fn optional_item(&self, slot: &mut Option<u16>) {
        *slot = slot.and_then(|id| self.item(id));
    }

    fn animator(&self, rig: RigId) -> Option<(RigId, &AnimatorLut)> {
        self.animators
            .get(rig.index())?
            .as_ref()
            .map(|(rig, lut)| (*rig, lut))
    }

    fn animator_event(&self, rig: RigId, event: u16) -> Option<(RigId, u16)> {
        let (rig, lut) = self.animator(rig)?;
        Some((rig, lookup(&lut.events, event as usize)?))
    }

    pub(crate) fn remap_animator_event(&self, rig: &mut RigId, event: &mut u16) -> bool {
        match self.animator_event(*rig, *event) {
            Some((local_rig, local)) => {
                *rig = local_rig;
                *event = local;
                true
            }
            None => false,
        }
    }

    pub fn remap_to_client(&self, msg: &mut ServerToClient) {
        if self.identity {
            return;
        }
        msg.remap(self);
    }

    pub fn apply<T: Remap>(&self, value: &mut T) -> bool {
        self.identity || value.remap(self)
    }

    pub fn remap_to_server(&self, msg: &mut ClientToServer) {
        if self.identity {
            return;
        }
        wire::remap_to_server(self, msg);
    }
}

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
    table.iter().enumerate().all(|(i, &v)| v == Some(i as u16))
}

#[inline]
fn lookup(table: &[Option<u16>], server_id: usize) -> Option<u16> {
    table.get(server_id).copied().flatten()
}
