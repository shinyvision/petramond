use crate::sound_registry::Sound;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockSoundAction {
    Dig,
    Break,
    Place,
    Step,
}

pub struct BlockSoundSet {
    pub dig: Option<Sound>,
    pub break_: Option<Sound>,
    pub place: Option<Sound>,
    pub step: Option<Sound>,
}

impl BlockSoundSet {
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

pub static SILENT: BlockSoundSet = BlockSoundSet {
    dig: None,
    break_: None,
    place: None,
    step: None,
};

pub static WOOD: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::WoodPunch),
    break_: Some(Sound::WoodBreak),
    place: Some(Sound::WoodPlace),
    step: Some(Sound::WoodStep),
};

pub static STONE: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::StonePunch),
    break_: Some(Sound::StoneBreak),
    place: Some(Sound::StonePlace),
    step: Some(Sound::StoneStep),
};

pub static DIRT: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::DirtPunch),
    break_: Some(Sound::DirtBreak),
    place: Some(Sound::DirtPlace),
    step: Some(Sound::DirtStep),
};

pub static SAND: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::SandPunch),
    break_: Some(Sound::SandBreak),
    place: Some(Sound::SandPlace),
    step: Some(Sound::SandStep),
};

pub static SNOW: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::SnowPunch),
    break_: Some(Sound::SnowBreak),
    place: Some(Sound::SnowPlace),
    step: Some(Sound::SnowStep),
};

pub static LEAF: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::LeafPunch),
    break_: Some(Sound::LeafBreak),
    place: Some(Sound::LeafPlace),
    step: Some(Sound::LeafStep),
};

pub static GLASS: BlockSoundSet = BlockSoundSet {
    dig: Some(Sound::GlassPunch),
    break_: Some(Sound::GlassBreak),
    place: Some(Sound::GlassPlace),
    step: Some(Sound::GlassStep),
};
