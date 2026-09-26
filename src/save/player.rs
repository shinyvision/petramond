//! `players/<key>.dat` (one per player identity): one player's persisted
//! state — position, velocity, look, mode, health, bed spawn, full inventory,
//! and active status effects.
//!
//! Split out of `level.dat` at v7 so every connected player saves and restores
//! independently. The file name is the player name run
//! through the same sanitize routine world save directories use (see
//! `save::mod.rs`); this module owns only the codec.

use crate::player::{BedSpawn, Player, PlayerMode};
use crate::save::codec::{get_item_slot, put_f32, put_item_slot, put_u32, put_u8, Reader};
use crate::save::format::{Format, RecordError};
use petramond_math::math::{IVec3, Vec3};
use petramond_world::inventory::{Inventory, TOTAL_SLOTS};
use petramond_world::item::ItemStack;

/// The player-file version this build writes, and the oldest it reads; a
/// later layout change adds an upgrade step to [`FORMAT`] (see
/// `save::format`).
const VERSION: u32 = 7;

/// The player-file format and its upgrade chain.
pub const FORMAT: Format = Format::new("player file", VERSION, &[]);

/// Decoded `players/<key>.dat` contents.
pub struct PlayerData {
    pub pos: petramond_math::world_pos::WorldPos,
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
    /// Item kinds this player has ever held, by registry NAME (ids are
    /// session-scoped, exactly like the effect names above). This is what
    /// makes `item_obtained` a once-per-lifetime event across sessions.
    pub obtained_items: Vec<String>,
    /// Unlocked crafting recipe keys, in unlock order.
    pub unlocked_recipes: Vec<String>,
}

pub fn encode(player: &Player) -> Vec<u8> {
    let mut b = Vec::new();
    put_u32(&mut b, VERSION);
    put_world_pos(&mut b, player.pos);
    put_vec3(&mut b, player.vel);
    put_f32(&mut b, player.yaw);
    put_f32(&mut b, player.pitch);
    put_u8(&mut b, player.mode().to_u8());
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
    put_item_slot(&mut b, player.inventory.off_hand().copied());
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
    // Progression: obtained items by registry name, then unlocked recipe keys
    // in unlock order (the order the wire catch-up depends on).
    let obtained: Vec<&str> = player
        .progression
        .obtained()
        .iter()
        .filter_map(|item| petramond_world::registry::names().items.name(item.id()))
        .collect();
    put_strings(&mut b, obtained.iter().copied());
    put_strings(
        &mut b,
        player.progression.unlocked().iter().map(String::as_str),
    );
    b
}

fn put_strings<'a>(b: &mut Vec<u8>, values: impl ExactSizeIterator<Item = &'a str>) {
    put_u32(b, values.len() as u32);
    for value in values {
        put_u32(b, value.len() as u32);
        b.extend_from_slice(value.as_bytes());
    }
}

fn get_strings(r: &mut Reader) -> Option<Vec<String>> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let len = r.u32()? as usize;
        out.push(std::str::from_utf8(r.bytes(len)?).ok()?.to_owned());
    }
    Some(out)
}

/// Decode a player file, migrating an older version first. A newer, retired
/// or malformed one is an error — never a fresh player.
pub fn decode(bytes: &[u8]) -> Result<PlayerData, RecordError> {
    let body = FORMAT.upgrade_u32_record(bytes)?;
    let mut r = Reader::new(&body);
    let data = decode_body(&mut r)
        .ok_or_else(|| RecordError::corrupt(FORMAT.name, "player record", 4 + r.offset()))?;
    if !r.is_at_end() {
        return Err(RecordError::corrupt(
            FORMAT.name,
            "trailing bytes",
            4 + r.offset(),
        ));
    }
    Ok(data)
}

/// The current-version body; `r` is left where decoding stopped.
fn decode_body(r: &mut Reader) -> Option<PlayerData> {
    let pos = get_world_pos(r)?;
    let vel = get_vec3(r)?;
    let (yaw, pitch) = (r.f32()?, r.f32()?);
    let mode = PlayerMode::from_u8(r.u8()?);
    let health = r.u32()? as i32;

    let bed_spawn = if r.u8()? == 1 {
        Some(BedSpawn {
            bed: get_ivec3(r)?,
            spot: get_ivec3(r)?,
        })
    } else {
        None
    };

    let mut slots: [Option<ItemStack>; TOTAL_SLOTS] = [None; TOTAL_SLOTS];
    for slot in slots.iter_mut() {
        *slot = get_item_slot(r)?;
    }
    let cursor = get_item_slot(r)?;
    let off_hand = get_item_slot(r)?;
    let active = r.u8()?;
    let inventory = Inventory::from_parts(slots, cursor, off_hand, active);
    let craft_craftable_only = r.u8()? != 0;

    let mut effects = Vec::new();
    let n = r.u32()?;
    for _ in 0..n {
        let klen = r.u32()? as usize;
        let name = std::str::from_utf8(r.bytes(klen)?).ok()?.to_owned();
        let remaining = r.u32()?;
        effects.push((name, remaining));
    }

    let obtained_items = get_strings(r)?;
    let unlocked_recipes = get_strings(r)?;

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
        obtained_items,
        unlocked_recipes,
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
        // An item whose pack is gone simply drops out of the record, like an
        // unknown effect: the set is a discovery log, not addressing.
        player.progression.restore(
            self.obtained_items
                .iter()
                .filter_map(|name| petramond_world::item::ItemType::by_name(name)),
            self.unlocked_recipes.clone(),
        );
        for (name, remaining) in &self.effects {
            match petramond_world::effect::by_name(name) {
                Some(effect) => player.apply_effect(effect, *remaining),
                None => log::warn!("player file: dropping unknown status effect '{name}'"),
            }
        }
        player
    }
}

fn put_world_pos(b: &mut Vec<u8>, p: petramond_math::world_pos::WorldPos) {
    crate::save::codec::put_f64(b, p.x);
    crate::save::codec::put_f64(b, p.y);
    crate::save::codec::put_f64(b, p.z);
}

fn get_world_pos(r: &mut Reader) -> Option<petramond_math::world_pos::WorldPos> {
    Some(petramond_math::world_pos::WorldPos::new(
        r.f64()?,
        r.f64()?,
        r.f64()?,
    ))
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
    use petramond_math::world_pos::WorldPos;

    #[test]
    fn player_file_roundtrips() {
        let mut player = Player::new(WorldPos::new(10.0, 72.0, -4.0));
        player.set_mode(PlayerMode::Spectator);
        player.vel = Vec3::new(0.0, -1.5, 0.25); // after set_mode, which zeroes vel
        player.yaw = 1.25;
        player.pitch = -0.5;
        player.set_health(7);
        player.inventory.set_active(3);
        *player.inventory.off_hand_mut() = Some(petramond_world::item::ItemStack::new(
            petramond_world::item::ItemType::Coal,
            9,
        ));
        player.bed_spawn = Some(BedSpawn {
            bed: IVec3::new(-3, 70, 12),
            spot: IVec3::new(-2, 70, 13),
        });
        player.apply_effect(petramond_world::effect::Effect::Regeneration, 950);
        player.craft_craftable_only = true;
        player
            .progression
            .obtain(petramond_world::item::ItemType::OakLog);
        player
            .progression
            .obtain(petramond_world::item::ItemType::Coal);
        player.progression.unlock("petramond:oak_planks");
        player.progression.unlock("petramond:torch");

        let bytes = encode(&player);
        let got = decode(&bytes).expect("decodes");

        assert_eq!(got.pos, WorldPos::new(10.0, 72.0, -4.0));
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
        assert_eq!(
            got.inventory.off_hand().map(|s| (s.item, s.count)),
            Some((petramond_world::item::ItemType::Coal, 9)),
            "the off-hand slot survives the round-trip"
        );
        assert!(
            got.restore().craft_craftable_only,
            "the craftable-only browser preference survives the round-trip"
        );
        // Progression is the one record a player cannot re-earn by playing:
        // losing it re-hides recipes they already unlocked, and losing the
        // obtained set re-fires `item_obtained` for things they have had for
        // days. Both travel by NAME, and unlock ORDER is the wire catch-up's
        // contract.
        let restored = got.restore();
        assert_eq!(
            restored.progression.unlocked(),
            ["petramond:oak_planks", "petramond:torch"],
            "unlocked recipes survive in unlock order"
        );
        assert!(restored.progression.obtained().intersects(
            &[
                petramond_world::item::ItemType::OakLog,
                petramond_world::item::ItemType::Coal
            ]
            .into_iter()
            .collect()
        ));
        let mut fresh = petramond_world::item::ItemSet::EMPTY;
        fresh.insert(petramond_world::item::ItemType::Diamond);
        assert!(
            !restored.progression.obtained().intersects(&fresh),
            "an item never held stays unheld"
        );
    }

    #[test]
    fn other_versions_are_typed_errors_not_a_fresh_player() {
        // A player file this build cannot read must never look like "no
        // file": the caller would respawn the player with an empty inventory
        // and save it over the original.
        let mut bytes = encode(&Player::new(WorldPos::new(1.0, 2.0, 3.0)));
        bytes[0..4].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert!(matches!(
            decode(&bytes),
            Err(RecordError::Newer { found, .. }) if found == VERSION + 1
        ));
        bytes[0..4].copy_from_slice(&(FORMAT.oldest() - 1).to_le_bytes());
        assert!(matches!(decode(&bytes), Err(RecordError::Retired { .. })));
    }

    #[test]
    fn a_truncated_file_is_corrupt() {
        let bytes = encode(&Player::new(WorldPos::new(1.0, 2.0, 3.0)));
        assert!(matches!(
            decode(&bytes[..bytes.len() - 1]),
            Err(RecordError::Corrupt { .. })
        ));
    }

    /// Golden player file v7, laid out by hand rather than by the encoder:
    /// survival at (1.5, 64, -2.5), health 20, no bed, empty inventory with
    /// hotbar slot 2 active, one effect, one obtained item, one recipe. Item
    /// slots are all empty so the fixture holds no palette-dependent id.
    #[test]
    fn golden_player_v7_decodes() {
        let got = decode(include_bytes!("fixtures/player_v7.bin")).expect("v7 decodes");
        assert_eq!(got.pos, WorldPos::new(1.5, 64.0, -2.5));
        assert_eq!(got.vel, Vec3::new(0.0, -0.5, 0.0));
        assert_eq!((got.yaw, got.pitch), (0.5, -0.25));
        assert_eq!(got.mode, PlayerMode::Survival);
        assert_eq!(got.health, 20);
        assert_eq!(got.bed_spawn, None);
        assert_eq!(got.inventory.active_slot(), 2);
        assert!(got.inventory.raw_slots().iter().all(Option::is_none));
        assert!(got.craft_craftable_only);
        assert_eq!(
            got.effects,
            vec![("petramond:regeneration".to_owned(), 100)]
        );
        assert_eq!(got.obtained_items, vec!["petramond:coal".to_owned()]);
        assert_eq!(got.unlocked_recipes, vec!["petramond:torch".to_owned()]);
    }

    #[test]
    fn restore_drops_unknown_effect_names_and_keeps_known_ones() {
        // A removed/disabled mod's effect must not error the whole restore —
        // it is dropped (with a warning) while known effects still apply.
        let mut player = Player::new(WorldPos::new(0.0, 70.0, 0.0));
        player.apply_effect(petramond_world::effect::Effect::Regeneration, 400);
        let mut data = decode(&encode(&player)).expect("decodes");
        data.effects
            .push(("gone_mod:vanished_effect".to_owned(), 100));

        let restored = data.restore();
        let active = restored.effects();
        assert_eq!(active.len(), 1, "only the known effect is restored");
        assert_eq!(
            active[0].effect,
            petramond_world::effect::Effect::Regeneration
        );
        assert_eq!(active[0].remaining, 400);
    }
}
