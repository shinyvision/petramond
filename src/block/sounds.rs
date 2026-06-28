//! Data-driven block sounds: which [`Sound`] a block makes for each interaction.
//!
//! A block's sounds follow its **material** — wood sounds woody, stone stony —
//! exactly as its mineability does, so [`Block::sound`](super::Block::sound)
//! resolves a [`BlockSoundSet`] by `match`ing the block's `BlockMaterial`, mirroring
//! [`Block::preferred_tool`](super::Block::preferred_tool). Giving a whole material
//! a sound is one edit here; a new block of an existing material is heard for free.
//! The shared sets are `'static` singletons that resolution points at, like the
//! [`behavior`](super::behavior) singletons.

use crate::audio::Sound;

/// An interaction that can make a block sound — the data-driven vocabulary. Code
/// asks `block.sound(action)` and the [`BlockSoundSet`] answers, so wiring a new
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
    step: None,
};

/// Stone: stone, cobblestone, granite, ore, and every other `BlockMaterial::Stone`
/// or `BlockMaterial::Ore` block. Mining loops the stone "punch"; break and place use
/// the stone break/place sounds; the step slot awaits its asset.
pub static STONE: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::StonePunch),
    break_: Some(Sound::StoneBreak),
    place: Some(Sound::StonePlace),
    step: None,
};
