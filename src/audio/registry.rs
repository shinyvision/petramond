//! The sound-asset table: a stable [`Sound`] id per sound effect, mapped to its
//! embedded source bytes and default playback parameters.
//!
//! This mirrors the id-ordered registry the `block`/`item`/`biome` modules use
//! (see [`crate::registry`]): a `#[repr(u8)]` key enum indexes an id-ordered
//! `&'static [Def]` table. Introducing a new sound is a data change — add a
//! [`Sound`] variant, a [`SOUND_DEFS`] row, and the asset file; no other code.

use crate::registry::{self, RegistryKey, TableEntry};

/// A sound effect the game can play, identified by a stable id (its `#[repr(u8)]`
/// value, which is the row's index in [`SOUND_DEFS`]).
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Sound {
    /// The wood "punch" — re-triggered while mining wood (see `crate::mining`).
    WoodPunch = 0,
    /// Placing a wood block.
    WoodPlace,
    /// Breaking / destroying a wood block.
    WoodBreak,
    /// Picking a dropped item up into the inventory — a global gameplay sound, not a
    /// block sound.
    ItemPickup,
    /// A door swung open (its `open` bit just flipped to true).
    DoorOpen,
    /// A door swung shut (its `open` bit just flipped to false).
    DoorClose,
    /// A chest's lid is swinging open (its GUI was just opened).
    ChestOpen,
    /// A chest's lid is dropping shut (its GUI was just closed).
    ChestClose,
    /// The stone "punch" — re-triggered while mining stone (and ore, which shares
    /// the stone set). See [`crate::block::sounds::STONE`].
    StonePunch,
    /// A stone block finished breaking / was destroyed.
    StoneBreak,
    /// A stone block was placed into the world.
    StonePlace,
}

impl Sound {
    /// Every sound in id order — the table's key list (drives the ordering test).
    #[cfg(test)]
    pub(crate) const ALL: &'static [Sound] = &[
        Sound::WoodPunch,
        Sound::WoodPlace,
        Sound::WoodBreak,
        Sound::ItemPickup,
        Sound::DoorOpen,
        Sound::DoorClose,
        Sound::ChestOpen,
        Sound::ChestClose,
        Sound::StonePunch,
        Sound::StoneBreak,
        Sound::StonePlace,
    ];

    /// This sound's static definition row.
    #[inline]
    pub(crate) fn def(self) -> &'static SoundDef {
        registry::def(SOUND_DEFS, self)
    }
}

/// The broad mixing group a sound belongs to. Effective gain is
/// `master × category × per-sound`; all categories are full volume today, so this
/// is the hook for a future per-category (e.g. options-menu) volume control.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SoundCategory {
    /// Block interaction — mining, breaking, placing, footsteps.
    Block,
    /// UI / interface & player feedback — menu clicks, item pickup, etc.
    Ui,
}

/// One row of the sound table: a sound's embedded source bytes + default playback
/// parameters. The bytes are decoded once at startup (see [`crate::audio::Audio`]);
/// this is just the static description.
pub(crate) struct SoundDef {
    pub sound: Sound,
    /// One or more interchangeable source clips (OGG/Vorbis), embedded at compile
    /// time (like every other asset, e.g. `crate::mob::loot`). A random variant is
    /// chosen each play — on top of the per-play pitch jitter — so a repeated sound
    /// never sounds identical. Order is irrelevant; add or remove clips freely.
    pub variants: &'static [&'static [u8]],
    /// Base linear gain on top of the category/master gain (`1.0` = unit).
    pub gain: f32,
    /// Per-play pitch jitter as a ± fraction of unit playback speed: each play picks
    /// a random speed in `[1 - v, 1 + v]`, so a repeated sound never sounds
    /// identical. `0.0` = none. (Speed shifts pitch — rodio `Source::speed`.)
    pub pitch_variation: f32,
    pub category: SoundCategory,
}

/// The sound table, ordered by [`Sound`] id (`index == sound as usize`). One row
/// per [`Sound`]; the ordering is asserted by the test below.
pub(crate) static SOUND_DEFS: &[SoundDef] = &[
    SoundDef {
        sound: Sound::WoodPunch,
        variants: &[
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/sounds/wood_punch_1.ogg"
            )),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/sounds/wood_punch_2.ogg"
            )),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/sounds/wood_punch_3.ogg"
            )),
        ],
        gain: 1.0,
        // ±12% speed: a clearly audible but natural variation, in the Minecraft range.
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::WoodPlace,
        variants: &[include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/sounds/wood_place.ogg"
        ))],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::WoodBreak,
        variants: &[include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/sounds/wood_break.ogg"
        ))],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::ItemPickup,
        variants: &[include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/sounds/item_pickup.ogg"
        ))],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Ui,
    },
    SoundDef {
        sound: Sound::DoorOpen,
        variants: &[include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/sounds/door_open.ogg"
        ))],
        gain: 1.0,
        pitch_variation: 0.08,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::DoorClose,
        variants: &[include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/sounds/door_close.ogg"
        ))],
        gain: 1.0,
        pitch_variation: 0.08,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::ChestOpen,
        variants: &[include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/sounds/chest_open.ogg"
        ))],
        gain: 1.0,
        pitch_variation: 0.08,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::ChestClose,
        variants: &[include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/sounds/chest_close.ogg"
        ))],
        gain: 1.0,
        pitch_variation: 0.08,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::StonePunch,
        variants: &[
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/sounds/stone_punch_1.ogg"
            )),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/sounds/stone_punch_2.ogg"
            )),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/sounds/stone_punch_3.ogg"
            )),
        ],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::StoneBreak,
        variants: &[include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/sounds/stone_break.ogg"
        ))],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::StonePlace,
        variants: &[include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/sounds/stone_place.ogg"
        ))],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
];

impl RegistryKey for Sound {
    #[inline]
    fn to_id(self) -> u8 {
        self as u8
    }
}

impl TableEntry for SoundDef {
    type Key = Sound;
    #[inline]
    fn key(&self) -> Sound {
        self.sound
    }
}

#[cfg(test)]
mod tests {
    /// The sound table is id-ordered and one-to-one with [`super::Sound`] — the
    /// same guarantee the block/item/biome tables carry, via the shared helper.
    #[test]
    fn sounds_are_id_ordered() {
        crate::registry::assert_id_ordered(super::SOUND_DEFS, super::Sound::ALL);
    }
}
