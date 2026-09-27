use crate::block::Block;
use crate::tile::Tile;

use super::{HeldPose, ItemTag, ItemType, ItemUse, Tool};

#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct ItemDef {
    pub item: ItemType,
    pub key: &'static str,
    pub name: &'static str,
    pub info: Option<&'static str>,
    pub max_stack_size: u8,
    pub held_pose: HeldPose,
    pub sprite_axis_degrees: f32,
    pub sprite_face_leads: bool,
    pub sprite: Option<Tile>,
    pub tags: &'static [ItemTag],
    pub block: Option<Block>,
    pub creative_visible: bool,
    pub creative_only: bool,
    pub world_tool: Option<&'static str>,
    pub placement_variants: &'static [Block],
    pub model: Option<crate::block_model::BlockModelKind>,
    pub item_use: Option<ItemUse>,
    pub use_ray: super::UseRay,
    pub fuel_burn_ticks: u16,
    pub tool: Option<Tool>,
    pub food: Option<super::FoodDef>,
    pub dropped_reaction: Option<super::DroppedReaction>,
    pub projectile: Option<super::Projectile>,
    pub data: &'static [(&'static str, &'static str)],
}
