//! `players/<key>.dat` (one per player identity): one player's persisted
//! state — position, velocity, look, mode, health, bed spawn, full inventory,
//! active status effects and progression.
//!
//! Split out of `level.dat` at v7 so every connected player saves and
//! restores independently. Since v8 the body is one tagged record
//! (`save::wire`): a field added later reads as its default in older files,
//! and a field this build does not know is kept (see [`KeptPlayer`]) and
//! written back. Slots whose items this world cannot resolve (a removed or
//! disabled mod's items) load empty and are kept the same way, so they
//! return with their mod.

mod v7;

use crate::player::{BedSpawn, Player, PlayerMode};
use crate::save::codec::DiskSlot;
use crate::save::format::{Format, RecordError};
use crate::save::palette::Palette;
use crate::save::wire::{tagged_record, wire_struct, UnknownFields, Wire};
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_persist::bytecodec::{put_u32, Reader};
use petramond_world::inventory::{Inventory, TOTAL_SLOTS};
use petramond_world::item::ItemStack;

/// The player-file version this build writes. v8 (2026-09-26) replaces the
/// positional v7 body with a tagged record; v7 files upgrade through
/// [`v7::upgrade`] on read.
const VERSION: u32 = 8;

/// The player-file format and its upgrade chain.
pub const FORMAT: Format = Format::new("player file", VERSION, &[v7::upgrade]);

/// The bed a player respawns at.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct BedRecord {
    bed: IVec3,
    spot: IVec3,
}
wire_struct!(BedRecord { bed, spot });

/// The v8 body. Tags are forever: a new field takes a new tag.
#[derive(Default)]
struct PlayerRecord {
    pos: WorldPos,
    vel: Vec3,
    yaw: f32,
    pitch: f32,
    mode: u8,
    health: u32,
    bed_spawn: Option<BedRecord>,
    /// Every inventory slot, in [`Inventory::raw_slots`] order.
    slots: Vec<DiskSlot>,
    cursor: DiskSlot,
    off_hand: DiskSlot,
    active_slot: u8,
    craft_craftable_only: bool,
    /// Active status effects as `(registry name, remaining ticks)`.
    effects: Vec<(String, u32)>,
    obtained_items: Vec<String>,
    unlocked_recipes: Vec<String>,
    unknown: UnknownFields,
}
tagged_record!(PlayerRecord {
    1 => pos,
    2 => vel,
    3 => yaw,
    4 => pitch,
    5 => mode,
    6 => health,
    7 => bed_spawn,
    8 => slots,
    9 => cursor,
    10 => off_hand,
    11 => active_slot,
    12 => craft_craftable_only,
    13 => effects,
    14 => obtained_items,
    15 => unlocked_recipes,
});

/// What a player file holds that a live [`Player`] cannot: slots whose
/// items this world cannot resolve (each loads empty) and fields this build
/// does not know. The save keeps it per player and writes it back — a kept
/// slot into its slot while that slot is still empty.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeptPlayer {
    /// `(inventory slot index, slot as stored)`.
    inventory: Vec<(usize, DiskSlot)>,
    cursor: Option<DiskSlot>,
    off_hand: Option<DiskSlot>,
    unknown: UnknownFields,
}

impl KeptPlayer {
    pub fn is_empty(&self) -> bool {
        self.inventory.is_empty()
            && self.cursor.is_none()
            && self.off_hand.is_none()
            && self.unknown.is_empty()
    }
}

/// Decoded `players/<key>.dat` contents.
pub struct PlayerData {
    pub pos: WorldPos,
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
    /// What the file held that the live player cannot (see [`KeptPlayer`]).
    pub kept: KeptPlayer,
}

/// Encode `player`; item ids are written as the world's disk ids through
/// `pal`.
pub fn encode(player: &Player, pal: &Palette) -> Vec<u8> {
    encode_keeping(player, pal, &KeptPlayer::default())
}

/// [`encode`], writing back what an earlier load kept.
pub fn encode_keeping(player: &Player, pal: &Palette, kept: &KeptPlayer) -> Vec<u8> {
    let or_kept = |live: Option<ItemStack>, kept: Option<&DiskSlot>| match (live, kept) {
        (None, Some(stored)) => stored.clone(),
        (live, _) => DiskSlot::of(live, pal),
    };
    let kept_at = |i: usize| {
        kept.inventory
            .iter()
            .find(|(at, _)| *at == i)
            .map(|(_, slot)| slot)
    };
    // Kept slots past this build's inventory ride along after it.
    let live_slots = player.inventory.raw_slots();
    let slot_count = kept
        .inventory
        .iter()
        .map(|(i, _)| i + 1)
        .fold(live_slots.len(), usize::max);
    let record = PlayerRecord {
        pos: player.pos,
        vel: player.vel,
        yaw: player.yaw,
        pitch: player.pitch,
        mode: player.mode().to_u8(),
        health: player.health() as u32,
        bed_spawn: player.bed_spawn.map(|bs| BedRecord {
            bed: bs.bed,
            spot: bs.spot,
        }),
        slots: (0..slot_count)
            .map(|i| or_kept(live_slots.get(i).copied().flatten(), kept_at(i)))
            .collect(),
        cursor: or_kept(player.inventory.cursor().copied(), kept.cursor.as_ref()),
        off_hand: or_kept(player.inventory.off_hand().copied(), kept.off_hand.as_ref()),
        active_slot: player.inventory.active_slot(),
        craft_craftable_only: player.craft_craftable_only,
        // By registry NAME — ids are session-scoped.
        effects: player
            .effects()
            .iter()
            .map(|e| (e.effect.def().name.to_owned(), e.remaining))
            .collect(),
        obtained_items: player
            .progression
            .obtained()
            .iter()
            .filter_map(|item| petramond_world::registry::names().items.name(item.id()))
            .map(str::to_owned)
            .collect(),
        // In unlock order: the order the wire catch-up depends on.
        unlocked_recipes: player.progression.unlocked().to_vec(),
        unknown: kept.unknown.clone(),
    };
    let mut b = Vec::new();
    put_u32(&mut b, VERSION);
    record.put(&mut b);
    b
}

/// Decode a player file, migrating an older version first. A newer, retired
/// or malformed one is an error — never a fresh player.
pub fn decode(bytes: &[u8], pal: &Palette) -> Result<PlayerData, RecordError> {
    let body = FORMAT.upgrade_u32_record(bytes)?;
    let mut r = Reader::new(&body);
    // Offsets count from the start of the file (the version header is 4 bytes).
    let record = PlayerRecord::get(&mut r)
        .ok_or_else(|| RecordError::corrupt(FORMAT.name, "player record", 4 + r.offset()))?;
    if !r.is_at_end() {
        return Err(RecordError::corrupt(
            FORMAT.name,
            "trailing bytes",
            4 + r.offset(),
        ));
    }
    Ok(PlayerData::from_record(record, pal))
}

impl PlayerData {
    fn from_record(record: PlayerRecord, pal: &Palette) -> Self {
        let mut kept = KeptPlayer {
            unknown: record.unknown,
            ..KeptPlayer::default()
        };
        let mut slots: [Option<ItemStack>; TOTAL_SLOTS] = [None; TOTAL_SLOTS];
        for (i, stored) in record.slots.into_iter().enumerate() {
            if i >= TOTAL_SLOTS {
                // A slot past this build's inventory is kept as stored.
                if !stored.is_empty() {
                    kept.inventory.push((i, stored));
                }
                continue;
            }
            match stored.resolve(pal) {
                Ok(live) => slots[i] = live,
                Err(stored) => kept.inventory.push((i, stored)),
            }
        }
        let cursor = record.cursor.resolve(pal).unwrap_or_else(|stored| {
            kept.cursor = Some(stored);
            None
        });
        let off_hand = record.off_hand.resolve(pal).unwrap_or_else(|stored| {
            kept.off_hand = Some(stored);
            None
        });
        Self {
            pos: record.pos,
            vel: record.vel,
            yaw: record.yaw,
            pitch: record.pitch,
            mode: PlayerMode::from_u8(record.mode),
            health: record.health as i32,
            bed_spawn: record.bed_spawn.map(|b| BedSpawn {
                bed: b.bed,
                spot: b.spot,
            }),
            inventory: Inventory::from_parts(slots, cursor, off_hand, record.active_slot),
            effects: record.effects,
            craft_craftable_only: record.craft_craftable_only,
            obtained_items: record.obtained_items,
            unlocked_recipes: record.unlocked_recipes,
            kept,
        }
    }

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

#[cfg(test)]
mod tests;
