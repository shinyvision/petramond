use super::ItemType;
use crate::block::Block;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DroppedReaction {
    pub fluid: Block,
    pub result: ItemType,
    pub burst: Option<u8>,
    pub sound: Option<crate::sound_registry::Sound>,
}
