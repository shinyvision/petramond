//! `level.dat`: the per-world header — format version, seed, the player
//! (position, velocity, look direction, mode, full inventory), and the
//! game-tick counter.

use std::collections::BTreeMap;

use crate::inventory::{Inventory, TOTAL_SLOTS};
use crate::item::ItemStack;
use crate::mathh::{IVec3, Vec3};
use crate::player::{BedSpawn, Player, PlayerMode};
use crate::save::codec::{get_item_slot, put_f32, put_item_slot, put_u32, put_u64, put_u8, Reader};

/// The one supported `level.dat` version. Only the CURRENT version decodes —
/// no legacy ladders, per WIKI/project-rules.md "Release Status and
/// Compatibility": bump this and wipe dev worlds when the layout changes.
const VERSION: u32 = 6;

/// Decoded `level.dat` contents.
pub struct LevelData {
    pub seed: u32,
    pub player_pos: Vec3,
    pub player_vel: Vec3,
    /// Look direction, radians (see `player::Player::yaw` / `pitch`).
    pub player_yaw: f32,
    pub player_pitch: f32,
    pub player_mode: PlayerMode,
    /// Health in half-heart points (`0..=`[`crate::player::MAX_HEALTH`]).
    pub player_health: i32,
    pub inventory: Inventory,
    /// Reserved/vestigial save-format field: `encode` writes it and `decode` reads the
    /// bytes, so dropping it would change the on-disk save codec.
    #[allow(dead_code)]
    pub tick: u64,
    /// The mod world KV map (`mod_id:key` → bytes; WIKI/modding.md Phase 3b).
    pub world_kv: BTreeMap<String, Vec<u8>>,
    /// The player's bed spawn point (`None` = no bed spawn — respawn falls back
    /// to a fresh surface pick).
    pub bed_spawn: Option<BedSpawn>,
    /// Active status effects as `(registry name, remaining ticks)` — names, not
    /// ids, because ids are session-scoped (like the block palette). Unknown
    /// names (a removed mod's effect) are dropped with a warning at restore.
    pub effects: Vec<(String, u32)>,
}

pub fn encode(
    seed: u32,
    player: &Player,
    tick: u64,
    world_kv: &BTreeMap<String, Vec<u8>>,
) -> Vec<u8> {
    let mut b = Vec::new();
    put_u32(&mut b, VERSION);
    put_u32(&mut b, seed);
    put_vec3(&mut b, player.pos);
    put_vec3(&mut b, player.vel);
    put_u8(
        &mut b,
        match player.mode() {
            PlayerMode::Survival => 0,
            PlayerMode::Spectator => 1,
        },
    );
    put_u64(&mut b, tick);
    for slot in player.inventory.raw_slots() {
        put_item_slot(&mut b, *slot);
    }
    put_item_slot(&mut b, player.inventory.cursor().copied());
    put_u8(&mut b, player.inventory.active_slot());
    put_f32(&mut b, player.yaw);
    put_f32(&mut b, player.pitch);
    put_u32(&mut b, player.health() as u32);
    // The mod world KV map. BTreeMap iteration is sorted, so identical maps
    // encode identically.
    put_u32(&mut b, world_kv.len().min(u32::MAX as usize) as u32);
    for (key, value) in world_kv {
        put_u32(&mut b, key.len() as u32);
        b.extend_from_slice(key.as_bytes());
        put_u32(&mut b, value.len() as u32);
        b.extend_from_slice(value);
    }
    // The bed spawn point: presence byte + bed base cell + wake spot.
    match player.bed_spawn {
        Some(bs) => {
            put_u8(&mut b, 1);
            put_ivec3(&mut b, bs.bed);
            put_ivec3(&mut b, bs.spot);
        }
        None => put_u8(&mut b, 0),
    }
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

/// Decode a CURRENT-version `level.dat`. Any other version returns `None` —
/// the world starts fresh (pre-release, breaking saves is free).
pub fn decode(bytes: &[u8]) -> Option<LevelData> {
    let mut r = Reader::new(bytes);
    if r.u32()? != VERSION {
        return None;
    }
    let seed = r.u32()?;
    let player_pos = get_vec3(&mut r)?;
    let player_vel = get_vec3(&mut r)?;
    let player_mode = match r.u8()? {
        1 => PlayerMode::Spectator,
        _ => PlayerMode::Survival,
    };
    let tick = r.u64()?;

    let mut slots: [Option<ItemStack>; TOTAL_SLOTS] = [None; TOTAL_SLOTS];
    for slot in slots.iter_mut() {
        *slot = get_item_slot(&mut r)?;
    }
    let cursor = get_item_slot(&mut r)?;
    let active = r.u8()?;
    let inventory = Inventory::from_parts(slots, cursor, active);

    let (player_yaw, player_pitch) = (r.f32()?, r.f32()?);
    let player_health = r.u32()? as i32;

    let mut world_kv = BTreeMap::new();
    let n = r.u32()?;
    for _ in 0..n {
        let klen = r.u32()? as usize;
        let key = std::str::from_utf8(r.bytes(klen)?).ok()?.to_owned();
        let vlen = r.u32()? as usize;
        world_kv.insert(key, r.bytes(vlen)?.to_vec());
    }

    let bed_spawn = if r.u8()? == 1 {
        Some(BedSpawn {
            bed: get_ivec3(&mut r)?,
            spot: get_ivec3(&mut r)?,
        })
    } else {
        None
    };

    let mut effects = Vec::new();
    let n = r.u32()?;
    for _ in 0..n {
        let klen = r.u32()? as usize;
        let name = std::str::from_utf8(r.bytes(klen)?).ok()?.to_owned();
        let remaining = r.u32()?;
        effects.push((name, remaining));
    }

    Some(LevelData {
        seed,
        player_pos,
        player_vel,
        player_yaw,
        player_pitch,
        player_mode,
        player_health,
        inventory,
        tick,
        world_kv,
        bed_spawn,
        effects,
    })
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
    fn level_roundtrips() {
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
        let kv = BTreeMap::from([
            ("llama:time".to_owned(), vec![0x10, 0x20]),
            ("zombies:invuln_until".to_owned(), Vec::new()),
        ]);
        player.apply_effect(crate::effect::Effect::Regeneration, 950);

        let bytes = encode(0xDEAD_BEEF, &player, 12_345, &kv);
        let got = decode(&bytes).expect("decodes");

        assert_eq!(got.seed, 0xDEAD_BEEF);
        assert_eq!(got.player_pos, Vec3::new(10.0, 72.0, -4.0));
        assert_eq!(got.player_vel, Vec3::new(0.0, -1.5, 0.25));
        assert_eq!(got.player_yaw, 1.25);
        assert_eq!(got.player_pitch, -0.5);
        assert_eq!(got.player_mode, PlayerMode::Spectator);
        assert_eq!(got.player_health, 7, "health survives the round-trip");
        assert_eq!(got.tick, 12_345);
        assert_eq!(got.inventory.active_slot(), 3);
        assert_eq!(got.world_kv, kv, "the mod world KV survives the round-trip");
        assert_eq!(
            got.bed_spawn, player.bed_spawn,
            "the bed spawn survives the round-trip"
        );
        assert_eq!(
            got.effects,
            vec![("llama:regeneration".to_owned(), 950)],
            "active effects survive the round-trip by name"
        );
        // Demo hotbar survives the round-trip.
        assert_eq!(
            got.inventory.selected().map(|s| s.item),
            player.inventory.selected().map(|s| s.item)
        );
    }

    #[test]
    fn a_stale_version_is_rejected_not_half_decoded() {
        // Only the current version loads (project rule: no legacy decode
        // paths; bump + wipe dev worlds instead). A stale blob must return
        // None so the session starts fresh.
        let player = Player::new(Vec3::new(1.0, 2.0, 3.0));
        let mut bytes = encode(7, &player, 0, &BTreeMap::new());
        bytes[0..4].copy_from_slice(&(VERSION - 1).to_le_bytes());
        assert!(decode(&bytes).is_none(), "stale version rejected");
        bytes[0..4].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(decode(&bytes).is_none(), "future version rejected");
    }
}
