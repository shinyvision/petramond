//! The sound-asset table: a stable [`Sound`] id per sound effect, mapped to its
//! embedded source bytes and default playback parameters.
//!
//! Add a sound by adding a [`Sound`] variant, a [`SOUND_DEFS`] row, and the asset file.

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
    /// The dirt "punch" — re-triggered while mining dirt, grass, gravel, and other
    /// dirt-likes. See [`crate::block::sounds::DIRT`].
    DirtPunch,
    /// A dirt block finished breaking / was destroyed.
    DirtBreak,
    /// A dirt block was placed into the world.
    DirtPlace,
}

impl Sound {
    /// This sound's static definition row.
    #[inline]
    pub(crate) fn def(self) -> &'static SoundDef {
        &SOUND_DEFS[self as usize]
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

/// One row of the sound table: a sound's clip files + default playback
/// parameters. Clips are read through the asset roots (so a mod pack can
/// override one by shipping the same relative path) and decoded once at
/// startup (see [`crate::audio::Audio`]); this is just the static description.
pub(crate) struct SoundDef {
    pub sound: Sound,
    /// One or more interchangeable source clips (OGG/Vorbis), as asset-relative
    /// paths (`sounds/...`) resolved through [`crate::assets`] at startup. A random
    /// variant is chosen each play — on top of the per-play pitch jitter — so a
    /// repeated sound never sounds identical. Order is irrelevant; add or remove
    /// clips freely.
    pub variants: &'static [&'static str],
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
            "sounds/wood_punch_1.ogg",
            "sounds/wood_punch_2.ogg",
            "sounds/wood_punch_3.ogg",
        ],
        gain: 1.0,
        // ±12% speed: a clearly audible but natural variation, in the Minecraft range.
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::WoodPlace,
        variants: &["sounds/wood_place.ogg"],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::WoodBreak,
        variants: &["sounds/wood_break.ogg"],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::ItemPickup,
        variants: &["sounds/item_pickup.ogg"],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Ui,
    },
    SoundDef {
        sound: Sound::DoorOpen,
        variants: &["sounds/door_open.ogg"],
        gain: 1.0,
        pitch_variation: 0.08,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::DoorClose,
        variants: &["sounds/door_close.ogg"],
        gain: 1.0,
        pitch_variation: 0.08,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::ChestOpen,
        variants: &["sounds/chest_open.ogg"],
        gain: 1.0,
        pitch_variation: 0.08,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::ChestClose,
        variants: &["sounds/chest_close.ogg"],
        gain: 1.0,
        pitch_variation: 0.08,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::StonePunch,
        variants: &[
            "sounds/stone_punch_1.ogg",
            "sounds/stone_punch_2.ogg",
            "sounds/stone_punch_3.ogg",
        ],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::StoneBreak,
        variants: &["sounds/stone_break.ogg"],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::StonePlace,
        variants: &["sounds/stone_place.ogg"],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::DirtPunch,
        variants: &[
            "sounds/dirt_punch_1.ogg",
            "sounds/dirt_punch_2.ogg",
            "sounds/dirt_punch_3.ogg",
        ],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::DirtBreak,
        variants: &["sounds/dirt_break.ogg"],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
    SoundDef {
        sound: Sound::DirtPlace,
        variants: &["sounds/dirt_place.ogg"],
        gain: 1.0,
        pitch_variation: 0.12,
        category: SoundCategory::Block,
    },
];
