//! `level.dat`: the per-world header — format version, seed, the player
//! (position, velocity, mode, full inventory), and the game-tick counter.

use crate::inventory::{Inventory, TOTAL_SLOTS};
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;
use crate::player::{Player, PlayerMode};
use crate::save::codec::{Reader, Writer};

const VERSION: u32 = 1;

/// Decoded `level.dat` contents.
pub struct LevelData {
    pub seed: u32,
    pub player_pos: Vec3,
    pub player_vel: Vec3,
    pub player_mode: PlayerMode,
    pub inventory: Inventory,
    pub tick: u64,
}

pub fn encode(seed: u32, player: &Player, tick: u64) -> Vec<u8> {
    let mut b = Vec::new();
    b.put_u32(VERSION);
    b.put_u32(seed);
    put_vec3(&mut b, player.pos);
    put_vec3(&mut b, player.vel);
    b.put_u8(match player.mode() {
        PlayerMode::Survival => 0,
        PlayerMode::Spectator => 1,
    });
    b.put_u64(tick);
    for slot in player.inventory.raw_slots() {
        put_slot(&mut b, slot.as_ref());
    }
    put_slot(&mut b, player.inventory.cursor());
    b.put_u8(player.inventory.active_slot());
    b
}

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
        *slot = get_slot(&mut r)?;
    }
    let cursor = get_slot(&mut r)?;
    let active = r.u8()?;
    let inventory = Inventory::from_parts(slots, cursor, active);

    Some(LevelData {
        seed,
        player_pos,
        player_vel,
        player_mode,
        inventory,
        tick,
    })
}

fn put_vec3(b: &mut Vec<u8>, v: Vec3) {
    b.put_f32(v.x);
    b.put_f32(v.y);
    b.put_f32(v.z);
}

fn get_vec3(r: &mut Reader) -> Option<Vec3> {
    Some(Vec3::new(r.f32()?, r.f32()?, r.f32()?))
}

/// A slot is two bytes: item id + count. Empty is encoded as id 0 (`Air`).
fn put_slot(b: &mut Vec<u8>, slot: Option<&ItemStack>) {
    match slot {
        Some(s) if !s.is_empty() => {
            b.put_u8(s.item.id());
            b.put_u8(s.count);
        }
        _ => {
            b.put_u8(0);
            b.put_u8(0);
        }
    }
}

fn get_slot(r: &mut Reader) -> Option<Option<ItemStack>> {
    let id = r.u8()?;
    let count = r.u8()?;
    if id == 0 || count == 0 {
        Some(None)
    } else {
        Some(Some(ItemStack::new(ItemType::from_id(id), count)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_roundtrips() {
        let mut player = Player::new(Vec3::new(10.0, 72.0, -4.0));
        player.set_mode(PlayerMode::Spectator);
        player.vel = Vec3::new(0.0, -1.5, 0.25); // after set_mode, which zeroes vel
        player.inventory.set_active(3);

        let bytes = encode(0xDEAD_BEEF, &player, 12_345);
        let got = decode(&bytes).expect("decodes");

        assert_eq!(got.seed, 0xDEAD_BEEF);
        assert_eq!(got.player_pos, Vec3::new(10.0, 72.0, -4.0));
        assert_eq!(got.player_vel, Vec3::new(0.0, -1.5, 0.25));
        assert_eq!(got.player_mode, PlayerMode::Spectator);
        assert_eq!(got.tick, 12_345);
        assert_eq!(got.inventory.active_slot(), 3);
        // Demo hotbar survives the round-trip.
        assert_eq!(got.inventory.selected().map(|s| s.item), player.inventory.selected().map(|s| s.item));
    }
}
