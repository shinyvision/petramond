//! Data-driven block sounds: which [`Sound`] a block makes for each interaction.
//!
//! A block's sounds follow its **material** — wood sounds woody, stone stony —
//! exactly as its mineability does, so [`Block::sound`](super::Block::sound)
//! resolves a [`BlockSoundSet`] by `match`ing the block's `BlockMaterial`, mirroring
//! [`Block::preferred_tool`](super::Block::preferred_tool). Giving a whole material
//! a sound is one edit here; a new block of an existing material is heard for free.
//! The shared sets are `'static` singletons that resolution points at, like the
//! [`behavior`](super::behavior) singletons.

use crate::sound_registry::Sound;

/// An interaction that can make a block sound — the data-driven vocabulary. Code
/// asks `block.sound(action)` and the `BlockSoundSet` answers, so wiring a new
/// interaction's sounds is a field here plus a lookup arm, never per-block logic.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockSoundAction {
    /// Re-triggered while the block is being mined (the "punch" loop) / when hit.
    Dig,
    /// The block finished breaking / was destroyed.
    Break,
    /// The block was placed into the world.
    Place,
    /// A footstep on top of the block.
    Step,
}

/// The sounds a block makes: one optional [`Sound`] per [`BlockSoundAction`].
/// `None` for an action means that interaction is silent for this block.
pub struct BlockSoundSet {
    pub dig: Option<Sound>,
    pub break_: Option<Sound>,
    pub place: Option<Sound>,
    pub step: Option<Sound>,
}

impl BlockSoundSet {
    /// The sound for `action`, if any.
    #[inline]
    pub fn get(&self, action: BlockSoundAction) -> Option<Sound> {
        match action {
            BlockSoundAction::Dig => self.dig,
            BlockSoundAction::Break => self.break_,
            BlockSoundAction::Place => self.place,
            BlockSoundAction::Step => self.step,
        }
    }
}

/// A block that makes no sound — the default for materials without sounds yet.
pub static SILENT: BlockSoundSet = BlockSoundSet {
    dig: None,
    break_: None,
    place: None,
    step: None,
};

/// Wood: logs, planks, the crafting table, chest, furniture workbench, and doors
/// (every `BlockMaterial::Wood` block). Mining loops the wood "punch"; the
/// break/place/step slots await their assets — add the asset + a [`Sound`] and fill
/// the slot in, no code elsewhere.
pub static WOOD: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::WoodPunch),
    break_: Some(Sound::WoodBreak),
    place: Some(Sound::WoodPlace),
    step: Some(Sound::WoodStep),
};

/// Stone: stone, cobblestone, granite, ore, and every other `BlockMaterial::Stone`
/// or `BlockMaterial::Ore` block. Mining loops the stone "punch"; break and place use
/// the stone break/place sounds; stepping replays the punch clips quietly.
pub static STONE: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::StonePunch),
    break_: Some(Sound::StoneBreak),
    place: Some(Sound::StonePlace),
    step: Some(Sound::StoneStep),
};

/// Dirt: dirt, grass (a smotherable dirt), gravel, and every other
/// `BlockMaterial::Dirt` block. Mining loops the dirt "punch"; break and place use
/// the dirt break/place sounds; stepping replays the punch clips quietly.
pub static DIRT: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::DirtPunch),
    break_: Some(Sound::DirtBreak),
    place: Some(Sound::DirtPlace),
    step: Some(Sound::DirtStep),
};

/// Sand-family: everything `BlockMaterial::Sand` — sand, red sand, clay, the
/// exploration pack's cave silt, and the snow layer/block (which are
/// shovel-classed as sand, so they inherit this set; a crunchier snow would be
/// its own material, not a per-block exception here). Mining loops the sand
/// "punch"; break and place use the sand break/place sounds; stepping replays
/// the punch clips quietly.
pub static SAND: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::SandPunch),
    break_: Some(Sound::SandBreak),
    place: Some(Sound::SandPlace),
    step: Some(Sound::SandStep),
};

/// Plant matter: LEAVES (`BlockMaterial::Foliage`) and every cross plant
/// (`BlockMaterial::Plant` — grass, flowers, saplings, the cactus, crops, the
/// exploration pack's cave flora and vines). Two materials because they mine
/// differently — shears grind a plant down and part foliage outright — but
/// they are the same matter and rustle the same, the way `Ice` shares the glass
/// set. Plant matter is walked THROUGH, not on, so its step slot only sounds
/// for the rare plant a body can stand on.
pub static LEAF: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::LeafPunch),
    break_: Some(Sound::LeafBreak),
    place: Some(Sound::LeafPlace),
    step: Some(Sound::LeafStep),
};

/// Glass-family: glass, panes (`BlockMaterial::Glass`) and ice
/// (`BlockMaterial::Ice`). Mining loops the glass "punch"; breaking shatters;
/// stepping replays the punch clips quietly.
pub static GLASS: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::GlassPunch),
    break_: Some(Sound::GlassBreak),
    place: Some(Sound::GlassPlace),
    step: Some(Sound::GlassStep),
};
