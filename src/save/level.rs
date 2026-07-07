//! `level.dat`: the per-world header — format version, seed, the player
//! (position, velocity, look direction, mode, full inventory), and the
//! game-tick counter.

use std::collections::BTreeMap;

use crate::inventory::{Inventory, TOTAL_SLOTS};
use crate::item::ItemStack;
use crate::mathh::{IVec3, Vec3};
use crate::player::{BedSpawn, Player, PlayerMode};
use crate::save::codec::{get_item_slot, put_f32, put_item_slot, put_u32, put_u64, put_u8, Reader};

/// Bumped to 2 for the player's look direction (yaw/pitch), then 3 for player
/// health, then 4 for the mod world KV map, then 5 for the bed spawn point,
/// then 6 for the player's active status effects. `decode` still accepts
/// v1..v5; their missing fields default (facing 0, full health, empty KV, no
/// bed spawn, no effects).
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
    /// Health in half-heart points (`0..=`[`crate::player::MAX_HEALTH`]). Defaults to
    /// full when loading a pre-v3 save that predates health.
    pub player_health: i32,
    pub inventory: Inventory,
    /// Reserved/vestigial save-format field: `encode` writes it and `decode` reads the
    /// bytes, so dropping it would change the on-disk save codec.
    #[allow(dead_code)]
    pub tick: u64,
    /// The mod world KV map (`mod_id:key` → bytes; WIKI/modding.md Phase 3b).
    /// Defaults empty when loading a pre-v4 save.
    pub world_kv: BTreeMap<String, Vec<u8>>,
    /// The player's bed spawn point (`None` = no bed spawn — respawn falls back
    /// to a fresh surface pick). Defaults `None` when loading a pre-v5 save.
    pub bed_spawn: Option<BedSpawn>,
    /// Active status effects as `(registry name, remaining ticks)` — names, not
    /// ids, because ids are session-scoped (like the block palette). Unknown
    /// names (a removed mod's effect) are dropped with a warning at restore.
    /// Defaults empty when loading a pre-v6 save.
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
    // v2: the player's look direction, appended after the inventory so a v1 save
    // still decodes (its facing defaults on load — see `decode`).
    put_f32(&mut b, player.yaw);
    put_f32(&mut b, player.pitch);
    // v3: player health, appended after the look so v1/v2 saves still decode
    // (health defaults to full on load).
    put_u32(&mut b, player.health() as u32);
    // v4: the mod world KV map, appended last so v1..v3 saves still decode
    // (defaults empty on load). BTreeMap iteration is sorted, so identical
    // maps encode identically.
    put_u32(&mut b, world_kv.len().min(u32::MAX as usize) as u32);
    for (key, value) in world_kv {
        put_u32(&mut b, key.len() as u32);
        b.extend_from_slice(key.as_bytes());
        put_u32(&mut b, value.len() as u32);
        b.extend_from_slice(value);
    }
    // v5: the bed spawn point, appended last so v1..v4 saves still decode
    // (defaults to none on load): presence byte + bed base cell + wake spot.
    match player.bed_spawn {
        Some(bs) => {
            put_u8(&mut b, 1);
            put_ivec3(&mut b, bs.bed);
            put_ivec3(&mut b, bs.spot);
        }
        None => put_u8(&mut b, 0),
    }
    // v6: active status effects, appended last so v1..v5 saves still decode
    // (default none). Persisted by registry NAME — ids are session-scoped.
    put_u32(&mut b, player.effects().len() as u32);
    for e in player.effects() {
        let name = e.effect.def().name;
        put_u32(&mut b, name.len() as u32);
        b.extend_from_slice(name.as_bytes());
        put_u32(&mut b, e.remaining);
    }
    b
}

pub fn decode(bytes: &[u8]) -> Option<LevelData> {
    let mut r = Reader::new(bytes);
    let version = r.u32()?;
    if !(1..=6).contains(&version) {
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

    // The look direction was appended in v2; a v1 save predates it, so its
    // player faces the default direction (yaw/pitch 0) on load.
    let (player_yaw, player_pitch) = if version >= 2 {
        (r.f32()?, r.f32()?)
    } else {
        (0.0, 0.0)
    };
    // Health was appended in v3; older saves predate it, so their player loads at
    // full health.
    let player_health = if version >= 3 {
        r.u32()? as i32
    } else {
        crate::player::MAX_HEALTH
    };
    // The mod world KV map was appended in v4; older saves load with none.
    let mut world_kv = BTreeMap::new();
    if version >= 4 {
        let n = r.u32()?;
        for _ in 0..n {
            let klen = r.u32()? as usize;
            let key = std::str::from_utf8(r.bytes(klen)?).ok()?.to_owned();
            let vlen = r.u32()? as usize;
            world_kv.insert(key, r.bytes(vlen)?.to_vec());
        }
    }

    // The bed spawn point was appended in v5; older saves load without one.
    let bed_spawn = if version >= 5 && r.u8()? == 1 {
        Some(BedSpawn {
            bed: get_ivec3(&mut r)?,
            spot: get_ivec3(&mut r)?,
        })
    } else {
        None
    };

    // Active status effects were appended in v6; older saves load with none.
    let mut effects = Vec::new();
    if version >= 6 {
        let n = r.u32()?;
        for _ in 0..n {
            let klen = r.u32()? as usize;
            let name = std::str::from_utf8(r.bytes(klen)?).ok()?.to_owned();
            let remaining = r.u32()?;
            effects.push((name, remaining));
        }
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
    fn v1_save_without_look_or_health_decodes_with_defaults() {
        // A pre-look (v1) save must still load: build a current blob (empty KV
        // + no bed spawn + no effects, so the appended tail is yaw/pitch +
        // health + KV count + bed presence byte + effects count = 21 bytes),
        // rewrite the version word to 1, and strip that tail. It decodes with
        // the rest intact, the facing defaulted, and health full.
        let mut player = Player::new(Vec3::new(1.0, 2.0, 3.0));
        player.yaw = 0.9; // present in the bytes, then truncated away below
        player.pitch = 0.3;
        player.set_health(5);
        let mut bytes = encode(7, &player, 0, &BTreeMap::new());
        bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
        bytes.truncate(bytes.len() - 21);

        let got = decode(&bytes).expect("v1 decodes");
        assert_eq!(got.player_pos, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(got.player_yaw, 0.0, "v1 facing defaults");
        assert_eq!(got.player_pitch, 0.0, "v1 facing defaults");
        assert_eq!(
            got.player_health,
            crate::player::MAX_HEALTH,
            "v1 loads at full health"
        );
        assert!(got.world_kv.is_empty(), "v1 loads with no mod world KV");
    }

    #[test]
    fn v2_save_without_health_decodes_at_full_health() {
        // A v2 save carries the look but predates health, the KV map, the bed
        // spawn, and effects: strip the trailing health + KV-count + bed
        // presence + effects-count bytes (13 bytes) and mark it v2. Facing
        // survives; health defaults to full.
        let mut player = Player::new(Vec3::new(4.0, 5.0, 6.0));
        player.yaw = 1.1;
        player.pitch = -0.2;
        player.set_health(9);
        let mut bytes = encode(3, &player, 0, &BTreeMap::new());
        bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        bytes.truncate(bytes.len() - 13);

        let got = decode(&bytes).expect("v2 decodes");
        assert_eq!(got.player_yaw, 1.1, "v2 keeps the look");
        assert_eq!(got.player_pitch, -0.2);
        assert_eq!(
            got.player_health,
            crate::player::MAX_HEALTH,
            "v2 loads at full health"
        );
        assert!(got.world_kv.is_empty(), "v2 loads with no mod world KV");
    }

    #[test]
    fn v3_save_without_world_kv_decodes_with_an_empty_map() {
        // A v3 save (health, no KV, no bed spawn, no effects) must keep
        // loading: strip the trailing KV count + bed presence + effects count
        // (9 bytes) and mark it v3. Everything else is intact and the KV
        // defaults empty.
        let mut player = Player::new(Vec3::new(4.0, 5.0, 6.0));
        player.set_health(9);
        let mut bytes = encode(3, &player, 0, &BTreeMap::new());
        bytes[0..4].copy_from_slice(&3u32.to_le_bytes());
        bytes.truncate(bytes.len() - 9);

        let got = decode(&bytes).expect("v3 decodes");
        assert_eq!(got.player_health, 9, "v3 keeps its health");
        assert!(got.world_kv.is_empty(), "v3 loads with no mod world KV");
    }

    #[test]
    fn v4_save_without_bed_spawn_decodes_with_none() {
        // A v4 save predates the bed spawn and effects: strip the trailing
        // bed block + effects count and mark it v4. Everything else
        // (including the KV) is intact.
        let mut player = Player::new(Vec3::new(4.0, 5.0, 6.0));
        player.bed_spawn = Some(BedSpawn {
            bed: IVec3::new(1, 2, 3),
            spot: IVec3::new(2, 2, 3),
        });
        let kv = BTreeMap::from([("llama:clock".to_owned(), vec![1, 2, 3, 4, 5, 6, 7, 8])]);
        let mut bytes = encode(3, &player, 0, &kv);
        bytes[0..4].copy_from_slice(&4u32.to_le_bytes());
        // presence byte + two encoded cells (6 × u32) + effects count (u32)
        bytes.truncate(bytes.len() - 29);

        let got = decode(&bytes).expect("v4 decodes");
        assert_eq!(got.world_kv, kv, "v4 keeps its KV");
        assert!(got.bed_spawn.is_none(), "v4 loads with no bed spawn");
    }

    #[test]
    fn v5_save_without_effects_decodes_with_none() {
        // A v5 save predates status effects: strip the trailing effects count
        // and mark it v5. The bed spawn (v5's own tail) is intact.
        let mut player = Player::new(Vec3::new(4.0, 5.0, 6.0));
        player.bed_spawn = Some(BedSpawn {
            bed: IVec3::new(1, 2, 3),
            spot: IVec3::new(2, 2, 3),
        });
        player.apply_effect(crate::effect::Effect::Regeneration, 100);
        let mut bytes = encode(3, &player, 0, &BTreeMap::new());
        bytes[0..4].copy_from_slice(&5u32.to_le_bytes());
        // effects count (u32) + one entry (name len u32 + 18-byte name + remaining u32)
        bytes.truncate(bytes.len() - (4 + 4 + 18 + 4));

        let got = decode(&bytes).expect("v5 decodes");
        assert_eq!(got.bed_spawn, player.bed_spawn, "v5 keeps its bed spawn");
        assert!(got.effects.is_empty(), "v5 loads with no effects");
    }
}
