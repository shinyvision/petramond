use crate::block::{Block, ItemRender};
use crate::tile::Tile;

use super::{
    data, definition, DroppedReaction, FoodDef, HeldPose, ItemRenderKind, ItemTag, ItemType,
    ItemUse, Projectile, Tool, UseRay,
};

impl ItemType {
    pub fn all() -> &'static [ItemType] {
        data::all()
    }

    #[inline]
    pub const fn id(self) -> u16 {
        self.0
    }

    #[inline]
    pub fn from_id(id: u16) -> ItemType {
        data::from_id(id)
    }

    #[inline]
    pub fn registry_name(self) -> &'static str {
        crate::registry::names()
            .items
            .name(self.id())
            .expect("registered item")
    }

    #[inline]
    pub fn by_name(name: &str) -> Option<ItemType> {
        crate::registry::names().items.id(name).map(ItemType)
    }

    #[inline]
    pub fn by_key(key: &str) -> Option<ItemType> {
        data::item_for_key(key)
    }

    #[inline]
    pub fn from_block(b: Block) -> ItemType {
        data::item_for_block(b)
    }

    pub fn creative_visible(self) -> bool {
        self != ItemType::Air && data::def(self).creative_visible
    }

    pub fn placement_variants(self) -> &'static [Block] {
        data::def(self).placement_variants
    }

    pub fn creative_only(self) -> bool {
        self != ItemType::Air && data::def(self).creative_only
    }

    pub fn world_tool(self) -> Option<&'static str> {
        data::def(self).world_tool
    }

    #[inline]
    pub fn as_block(self) -> Option<Block> {
        self.def().block
    }

    #[inline]
    pub fn tool(self) -> Option<Tool> {
        self.def().tool
    }

    #[inline]
    pub fn fuel_burn_ticks(self) -> u16 {
        self.def().fuel_burn_ticks
    }

    #[inline]
    pub fn data_value(self, key: &str) -> Option<&'static str> {
        let data = self.def().data;
        data.binary_search_by(|(k, _)| (*k).cmp(key))
            .ok()
            .map(|i| data[i].1)
    }

    #[inline]
    pub fn item_use(self) -> Option<ItemUse> {
        self.def().item_use
    }

    #[inline]
    pub fn use_ray(self) -> UseRay {
        self.def().use_ray
    }

    #[inline]
    pub fn food(self) -> Option<FoodDef> {
        self.def().food
    }

    #[inline]
    pub fn dropped_reaction(self) -> Option<DroppedReaction> {
        self.def().dropped_reaction
    }

    #[inline]
    pub fn projectile(self) -> Projectile {
        self.def().projectile.unwrap_or_default()
    }

    #[inline]
    pub fn has_tag(self, tag: ItemTag) -> bool {
        self.def().tags.contains(&tag)
    }

    #[inline]
    pub fn tags(self) -> &'static [ItemTag] {
        self.def().tags
    }

    #[inline]
    pub fn max_stack_size(self) -> u8 {
        if self.is_durable() {
            1
        } else {
            self.def().max_stack_size
        }
    }

    #[inline]
    pub fn is_durable(self) -> bool {
        self.tool().is_some()
    }

    #[inline]
    pub fn key(self) -> &'static str {
        self.def().key
    }

    #[inline]
    pub fn name(self) -> &'static str {
        self.def().name
    }

    #[inline]
    pub fn info(self) -> Option<&'static str> {
        self.def().info
    }

    #[inline]
    pub fn declared_sprite(self) -> Option<Tile> {
        self.def().sprite
    }

    #[inline]
    pub fn render_kind(self) -> ItemRenderKind {
        if let Some(sprite) = self.def().sprite {
            return ItemRenderKind::Sprite(sprite);
        }
        match self.as_block() {
            Some(block) => {
                let k = block.shape_kind_def();
                match k.render.item_render(&k.params, block) {
                    ItemRender::ItemSprite if self.creative_only() => {
                        ItemRenderKind::Sprite(block.tiles()[0])
                    }
                    ItemRender::ItemSprite => ItemRenderKind::Sprite(self.item_sprite()),
                    ItemRender::Tile(tile) => ItemRenderKind::Sprite(tile),
                    ItemRender::BlockForm(b) => ItemRenderKind::BlockCube(b),
                    ItemRender::Model(kind) => ItemRenderKind::Model(kind),
                }
            }
            None => match self.item_model() {
                Some(kind) => ItemRenderKind::Model(kind),
                None => ItemRenderKind::Sprite(self.item_sprite()),
            },
        }
    }

    #[inline]
    pub fn held_pose(self) -> HeldPose {
        self.def().held_pose
    }

    #[inline]
    pub fn sprite_axis_roll(self) -> f32 {
        -self.def().sprite_axis_degrees.to_radians()
    }

    #[inline]
    pub fn sprite_face_leads(self) -> bool {
        self.def().sprite_face_leads
    }

    #[inline]
    fn item_sprite(self) -> Tile {
        self.def()
            .sprite
            .unwrap_or_else(|| Tile::from_name("stick").expect("atlas has a 'stick' tile"))
    }

    #[inline]
    fn item_model(self) -> Option<crate::block_model::BlockModelKind> {
        self.def().model
    }

    #[inline]
    fn def(self) -> &'static definition::ItemDef {
        data::def(self)
    }
}
