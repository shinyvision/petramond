//! `players/<name>.dat`: one player's persisted state — position, velocity,
//! look, mode, health, bed spawn, full inventory, and active status effects.
//!
//! Split out of `level.dat` at v7 so every connected player saves and restores
//! independently (multiplayer Phase C). The file name is the player name run
//! through the same sanitize routine world save directories use (see
//! `save::mod.rs`); this module owns only the codec.

use crate::inventory::{Inventory, TOTAL_SLOTS};
use crate::item::ItemStack;
use crate::mathh::{IVec3, Vec3};
use crate::player::{BedSpawn, Player, PlayerMode};
use crate::save::codec::{get_item_slot, put_f32, put_item_slot, put_u32, put_u8, Reader};

/// The one supported player-file version. Only the CURRENT version decodes —
/// no legacy ladders. Bump this and let old dev players respawn fresh.
const VERSION: u32 = 2;

/// Decoded `players/<name>.dat` contents.
pub struct PlayerData {
    pub pos: Vec3,
    pub vel: Vec3,
    /// Look direction, radians (see `player::Player::yaw` / `pitch`).
    pub yaw: f32,
    pub pitch: f32,
    pub mode: PlayerMode,
    /// Health in half-heart points (`0..=`[`crate::player::MAX_HEALTH`]).
    pub health: i32,
    /// The player's bed spawn point (`None` = no bed spawn — respawn falls back
    /// to a fresh surface pick).
    pub bed_spawn: Option<BedSpawn>,
    pub inventory: Inventory,
    /// Active status effects as `(registry name, remaining ticks)` — names, not
    /// ids, because ids are session-scoped (like the block palette). Unknown
    /// names (a removed mod's effect) are dropped with a warning at restore.
    pub effects: Vec<(String, u32)>,
    /// The recipe browser's craftable-only filter preference.
    pub craft_craftable_only: bool,
}

pub fn encode(player: &Player) -> Vec<u8> {
    let mut b = Vec::new();
    put_u32(&mut b, VERSION);
    put_vec3(&mut b, player.pos);
    put_vec3(&mut b, player.vel);
    put_f32(&mut b, player.yaw);
    put_f32(&mut b, player.pitch);
    put_u8(
        &mut b,
        match player.mode() {
            PlayerMode::Survival => 0,
            PlayerMode::Spectator => 1,
        },
    );
    put_u32(&mut b, player.health() as u32);
    // The bed spawn point: presence byte + bed base cell + wake spot.
    match player.bed_spawn {
        Some(bs) => {
            put_u8(&mut b, 1);
            put_ivec3(&mut b, bs.bed);
            put_ivec3(&mut b, bs.spot);
        }
        None => put_u8(&mut b, 0),
    }
    for slot in player.inventory.raw_slots() {
        put_item_slot(&mut b, *slot);
    }
    put_item_slot(&mut b, player.inventory.cursor().copied());
    put_u8(&mut b, player.inventory.active_slot());
    put_u8(&mut b, player.craft_craftable_only as u8);
    // Active status effects, persisted by registry NAME — ids are
    // session-scoped.
    put_u32(&mut b, player.effects().len() as u32);
    for e in player.effects() {
        let name = e.effect.def().name;
        put_u32(&mut b, name.len() as u32);
        b.extend_from_slice(name.as_bytes());
        put_u32(&mut b, e.remaining);
    }
    b
}

/// Decode a CURRENT-version player file. Any other version returns `None` —
/// the player respawns fresh (pre-release, breaking saves is free).
pub fn decode(bytes: &[u8]) -> Option<PlayerData> {
    let mut r = Reader::new(bytes);
    if r.u32()? != VERSION {
        return None;
    }
    let pos = get_vec3(&mut r)?;
    let vel = get_vec3(&mut r)?;
    let (yaw, pitch) = (r.f32()?, r.f32()?);
    let mode = match r.u8()? {
        1 => PlayerMode::Spectator,
        _ => PlayerMode::Survival,
    };
    let health = r.u32()? as i32;

    let bed_spawn = if r.u8()? == 1 {
        Some(BedSpawn {
            bed: get_ivec3(&mut r)?,
            spot: get_ivec3(&mut r)?,
        })
    } else {
        None
    };

    let mut slots: [Option<ItemStack>; TOTAL_SLOTS] = [None; TOTAL_SLOTS];
    for slot in slots.iter_mut() {
        *slot = get_item_slot(&mut r)?;
    }
    let cursor = get_item_slot(&mut r)?;
    let active = r.u8()?;
    let inventory = Inventory::from_parts(slots, cursor, active);
    let craft_craftable_only = r.u8()? != 0;

    let mut effects = Vec::new();
    let n = r.u32()?;
    for _ in 0..n {
        let klen = r.u32()? as usize;
        let name = std::str::from_utf8(r.bytes(klen)?).ok()?.to_owned();
        let remaining = r.u32()?;
        effects.push((name, remaining));
    }

    Some(PlayerData {
        pos,
        vel,
        yaw,
        pitch,
        mode,
        health,
        bed_spawn,
        inventory,
        effects,
        craft_craftable_only,
    })
}

impl PlayerData {
    /// Rebuild a live [`Player`] from the decoded record. Effects resolve by
    /// registry name; a name the session doesn't know (its mod was removed or
    /// disabled) is dropped with a warning, never an error.
    pub fn restore(&self) -> Player {
        let mut player = Player::new(self.pos);
        player.set_mode(self.mode);
        // `set_mode` clears velocity, so restore saved motion after mode.
        player.vel = self.vel;
        player.yaw = self.yaw;
        player.pitch = self.pitch;
        player.set_health(self.health);
        player.inventory = self.inventory.clone();
        player.bed_spawn = self.bed_spawn;
        player.craft_craftable_only = self.craft_craftable_only;
        for (name, remaining) in &self.effects {
            match crate::effect::by_name(name) {
                Some(effect) => player.apply_effect(effect, *remaining),
                None => log::warn!("player file: dropping unknown status effect '{name}'"),
            }
        }
        player
    }
}

fn put_vec3(b: &mut Vec<u8>, v: Vec3) {
    put_f32(b, v.x);
    put_f32(b, v.y);
    put_f32(b, v.z);
}

fn get_vec3(r: &mut Reader) -> Option<Vec3> {
    Some(Vec3::new(r.f32()?, r.f32()?, r.f32()?))
}

fn put_ivec3(b: &mut Vec<u8>, v: IVec3) {
    put_u32(b, v.x as u32);
    put_u32(b, v.y as u32);
    put_u32(b, v.z as u32);
}

fn get_ivec3(r: &mut Reader) -> Option<IVec3> {
    Some(IVec3::new(
        r.u32()? as i32,
        r.u32()? as i32,
        r.u32()? as i32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_file_roundtrips() {
        let mut player = Player::new(Vec3::new(10.0, 72.0, -4.0));
        player.set_mode(PlayerMode::Spectator);
        player.vel = Vec3::new(0.0, -1.5, 0.25); // after set_mode, which zeroes vel
        player.yaw = 1.25;
        player.pitch = -0.5;
        player.set_health(7);
        player.inventory.set_active(3);
        player.bed_spawn = Some(BedSpawn {
            bed: IVec3::new(-3, 70, 12),
            spot: IVec3::new(-2, 70, 13),
        });
        player.apply_effect(crate::effect::Effect::Regeneration, 950);
        player.craft_craftable_only = true;

        let bytes = encode(&player);
        let got = decode(&bytes).expect("decodes");

        assert_eq!(got.pos, Vec3::new(10.0, 72.0, -4.0));
        assert_eq!(got.vel, Vec3::new(0.0, -1.5, 0.25));
        assert_eq!(got.yaw, 1.25);
        assert_eq!(got.pitch, -0.5);
        assert_eq!(got.mode, PlayerMode::Spectator);
        assert_eq!(got.health, 7, "health survives the round-trip");
        assert_eq!(got.inventory.active_slot(), 3);
        assert_eq!(
            got.bed_spawn, player.bed_spawn,
            "the bed spawn survives the round-trip"
        );
        assert_eq!(
            got.effects,
            vec![("petramond:regeneration".to_owned(), 950)],
            "active effects survive the round-trip by name"
        );
        // Demo hotbar survives the round-trip.
        assert_eq!(
            got.inventory.selected().map(|s| s.item),
            player.inventory.selected().map(|s| s.item)
        );
        assert!(
            got.restore().craft_craftable_only,
            "the craftable-only browser preference survives the round-trip"
        );
    }

    #[test]
    fn a_stale_version_is_rejected_not_half_decoded() {
        // Only the current version loads (project rule: no legacy decode
        // paths; bump + wipe dev players instead). A stale blob must return
        // None so the player respawns fresh.
        let mut bytes = encode(&Player::new(Vec3::new(1.0, 2.0, 3.0)));
        bytes[0..4].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(decode(&bytes).is_none(), "future version rejected");
    }

    #[test]
    fn restore_drops_unknown_effect_names_and_keeps_known_ones() {
        // A removed/disabled mod's effect must not error the whole restore —
        // it is dropped (with a warning) while known effects still apply.
        let mut player = Player::new(Vec3::new(0.0, 70.0, 0.0));
        player.apply_effect(crate::effect::Effect::Regeneration, 400);
        let mut data = decode(&encode(&player)).expect("decodes");
        data.effects
            .push(("gone_mod:vanished_effect".to_owned(), 100));

        let restored = data.restore();
        let active = restored.effects();
        assert_eq!(active.len(), 1, "only the known effect is restored");
        assert_eq!(active[0].effect, crate::effect::Effect::Regeneration);
        assert_eq!(active[0].remaining, 400);
    }
}
