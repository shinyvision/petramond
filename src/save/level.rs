//! `level.dat`: the per-world header — format version, seed, the player
//! (position, velocity, look direction, mode, full inventory), and the
//! game-tick counter.

use std::collections::BTreeMap;

use crate::inventory::{Inventory, TOTAL_SLOTS};
use crate::item::ItemStack;
use crate::mathh::Vec3;
use crate::player::{Player, PlayerMode};
use crate::save::codec::{get_item_slot, put_f32, put_item_slot, put_u32, put_u64, put_u8, Reader};

/// Bumped to 2 for the player's look direction (yaw/pitch), then 3 for player
/// health, then 4 for the mod world KV map. `decode` still accepts v1..v3;
/// their missing fields default (facing 0, full health, empty KV).
const VERSION: u32 = 4;

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
    b
}

pub fn decode(bytes: &[u8]) -> Option<LevelData> {
    let mut r = Reader::new(bytes);
    let version = r.u32()?;
    if !(1..=4).contains(&version) {
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
        let kv = BTreeMap::from([
            ("llama:time".to_owned(), vec![0x10, 0x20]),
            ("zombies:invuln_until".to_owned(), Vec::new()),
        ]);

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
        // Demo hotbar survives the round-trip.
        assert_eq!(
            got.inventory.selected().map(|s| s.item),
            player.inventory.selected().map(|s| s.item)
        );
    }

    #[test]
    fn v1_save_without_look_or_health_decodes_with_defaults() {
        // A pre-look (v1) save must still load: build a current blob (empty KV
        // so its v4 tail is exactly the 4-byte zero count), rewrite the version
        // word to 1, and strip the appended yaw/pitch + health + KV count (four
        // trailing f32/u32 words = 16 bytes). It decodes with the rest intact,
        // the facing defaulted, and health full.
        let mut player = Player::new(Vec3::new(1.0, 2.0, 3.0));
        player.yaw = 0.9; // present in the bytes, then truncated away below
        player.pitch = 0.3;
        player.set_health(5);
        let mut bytes = encode(7, &player, 0, &BTreeMap::new());
        bytes[0..4].copy_from_slice(&1u32.to_le_bytes());
        bytes.truncate(bytes.len() - 16);

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
        // A v2 save carries the look but predates health and the KV map: strip
        // the trailing health + KV-count words (8 bytes) and mark it v2. Facing
        // survives; health defaults to full.
        let mut player = Player::new(Vec3::new(4.0, 5.0, 6.0));
        player.yaw = 1.1;
        player.pitch = -0.2;
        player.set_health(9);
        let mut bytes = encode(3, &player, 0, &BTreeMap::new());
        bytes[0..4].copy_from_slice(&2u32.to_le_bytes());
        bytes.truncate(bytes.len() - 8);

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
        // A v3 save (health, no KV) must keep loading: strip the trailing
        // 4-byte KV count and mark it v3. Everything else is intact and the KV
        // defaults empty.
        let mut player = Player::new(Vec3::new(4.0, 5.0, 6.0));
        player.set_health(9);
        let mut bytes = encode(3, &player, 0, &BTreeMap::new());
        bytes[0..4].copy_from_slice(&3u32.to_le_bytes());
        bytes.truncate(bytes.len() - 4);

        let got = decode(&bytes).expect("v3 decodes");
        assert_eq!(got.player_health, 9, "v3 keeps its health");
        assert!(got.world_kv.is_empty(), "v3 loads with no mod world KV");
    }
}
