use crate::atlas::Tile;
use crate::block::{Block, ItemRender};

use super::{
    data, definition, DroppedReaction, FoodDef, HeldPose, ItemRenderKind, ItemTag, ItemType,
    ItemUse, Tool, UseRay,
};

impl ItemType {
    /// Every registered item in id order — engine first (frozen ids), then
    /// pack-registered items in load order (mirrors [`Block::all`]).
    pub fn all() -> &'static [ItemType] {
        data::all()
    }

    /// Stable numeric id.
    #[inline]
    pub const fn id(self) -> u8 {
        self.0
    }

    /// Item for `id`, or `Air` if `id` is out of range.
    #[inline]
    pub fn from_id(id: u8) -> ItemType {
        data::from_id(id)
    }

    /// The item registered under registry NAME `name` (`"petramond:coal"`,
    /// `"kitchen:raw_mutton"`), or `None`. O(1) through the shared name
    /// table's hash index — the lookup behind every name-addressed mod call.
    #[inline]
    pub fn by_name(name: &str) -> Option<ItemType> {
        crate::registry::names().items.id(name).map(ItemType)
    }

    /// The item whose row carries recipe [`key`](Self::key), or `None`. O(1)
    /// hash index. ENGINE-INTERNAL recipe plumbing (recipes/loot tables
    /// reference items by key); the mod-facing identity is the registry name
    /// — see [`by_name`](Self::by_name).
    #[inline]
    pub fn by_key(key: &str) -> Option<ItemType> {
        data::item_for_key(key)
    }

    /// The block-item for a block: the item whose `items.json` row links it
    /// via the row's `block` field, read from the dense reverse LUT built at
    /// load (see `data`). A block no item links to (a machine's lit variant,
    /// a late crop growth stage) maps to `Air` — nothing to hold. `Air -> Air`.
    #[inline]
    pub fn from_block(b: Block) -> ItemType {
        data::item_for_block(b)
    }

    /// The block this item places (its row's `block` field in `items.json`),
    /// or `None` for an item-only item (tools, raw drops, ingots).
    #[inline]
    pub fn as_block(self) -> Option<Block> {
        self.def().block
    }

    /// This item as a mining [`Tool`] (kind + material tier), or `None` if it
    /// isn't a tool. Drives tool-gated mining — the held tool's kind must match a
    /// block's [`preferred_tool`](crate::block::Block::preferred_tool) to mine it
    /// faster, and a pickaxe's tier must meet a block's
    /// [`harvest_tier`](crate::block::Block::harvest_tier) to unlock its drop (see
    /// [`crate::mining::break_time`]). The axe/pickaxe/shovel families share the
    /// tier ladder `1..=4` (wooden, stone, iron, diamond).
    #[inline]
    pub fn tool(self) -> Option<Tool> {
        self.def().tool
    }

    /// How many game ticks this item burns as furnace fuel (`0` = not a fuel).
    /// A property of the item (`"fuel_burn_ticks"` in `items.json`) — a furnace
    /// consuming it reads this, like mining reads [`tool`](Self::tool).
    #[inline]
    pub fn fuel_burn_ticks(self) -> u16 {
        self.def().fuel_burn_ticks
    }

    /// The right-click use this item's data row declares (`"use"` in
    /// `items.json`), or `None` for items with no use of their own. The tick
    /// dispatches on the resolved [`ItemUse`], so which item fills a bucket is
    /// row data, not code.
    #[inline]
    pub fn item_use(self) -> Option<ItemUse> {
        self.def().item_use
    }

    /// How this item's use click resolves its block target (`"use_ray"` in
    /// `items.json`) — see [`UseRay`].
    #[inline]
    pub fn use_ray(self) -> UseRay {
        self.def().use_ray
    }

    /// This item's edible data (`"food"` in `items.json`), or `None` for
    /// non-food. Which items are edible is row data, like fuel and tools.
    #[inline]
    pub fn food(self) -> Option<FoodDef> {
        self.def().food
    }

    /// This item's dropped-entity environmental reaction
    /// (`"dropped_reaction"` in `items.json`), or `None` — see
    /// [`DroppedReaction`].
    #[inline]
    pub fn dropped_reaction(self) -> Option<DroppedReaction> {
        self.def().dropped_reaction
    }

    /// Whether this item belongs to `tag`. Membership is item data — each item's
    /// [`ItemDef`](definition::ItemDef) lists its tags — so recipes can require a
    /// group (e.g. any `petramond:planks`) without naming every member, and a new
    /// item joins a group by editing its data row, never any recipe code.
    #[inline]
    pub fn has_tag(self, tag: ItemTag) -> bool {
        self.def().tags.contains(&tag)
    }

    /// Every tag this item carries (see [`has_tag`](Self::has_tag)).
    #[inline]
    pub fn tags(self) -> &'static [ItemTag] {
        self.def().tags
    }

    /// Maximum number of this item per stack. Durable items never stack (one per
    /// slot); everything else uses its table value.
    #[inline]
    pub fn max_stack_size(self) -> u8 {
        if self.is_durable() {
            1
        } else {
            self.def().max_stack_size
        }
    }

    /// Whether this item carries durability. A durable item never stacks (one per
    /// slot) — that limit is a CONSEQUENCE of durability, not of being a "tool".
    /// Durability isn't consumed yet, but the model is correct: a future durable
    /// non-tool item would also not stack, for the same reason. Every mining
    /// [`tool`](Self::tool) (the pickaxes, axes, shovels + shears) is durable.
    #[inline]
    pub fn is_durable(self) -> bool {
        self.tool().is_some()
    }

    /// Stable snake_case identity recipes reference (e.g. `oak_planks`), read from
    /// the item's [`ItemDef`](definition::ItemDef) row. This is the item's real id,
    /// distinct from its [`name`](Self::name) display string — renaming the name
    /// never moves the key, so recipes keep resolving (see `crate::crafting::load`).
    #[inline]
    pub fn key(self) -> &'static str {
        self.def().key
    }

    /// Human-readable display name (UI only; the recipe identity is
    /// [`key`](Self::key)).
    #[inline]
    pub fn name(self) -> &'static str {
        self.def().name
    }

    /// How to draw this item. A ROW-DECLARED sprite always wins: an item that
    /// places a block but ships its own flat art (seeds planting a crop, a
    /// door, the torch) draws that art everywhere the ITEM is shown — the
    /// block's in-world look never leaks into the icon/drop. Otherwise
    /// block-items follow their block's render shape (`BlockCube` for full
    /// cubes, `Sprite` for cross-model plants), and item-only items are flat
    /// sprites, unless they carry their own bbmodel
    /// ([`item_model`](Self::item_model)).
    #[inline]
    pub fn render_kind(self) -> ItemRenderKind {
        if let Some(sprite) = self.def().sprite {
            return ItemRenderKind::Sprite(sprite);
        }
        match self.as_block() {
            // A block-item's KIND is the shape's decision (`ShapeRender::item_render`),
            // so item entities / in-hand / icons stay coherent for every shape,
            // engine or modded. The one item-side piece is the flat art an
            // `ItemSprite` shape uses — the item's own row sprite.
            Some(block) => {
                let k = block.shape_kind_def();
                match k.render.item_render(&k.params, block) {
                    ItemRender::ItemSprite => ItemRenderKind::Sprite(self.item_sprite()),
                    ItemRender::Tile(tile) => ItemRenderKind::Sprite(tile),
                    ItemRender::Cube(b) => ItemRenderKind::BlockCube(b),
                    // The Cube/true-geometry split (stair/slab/fence) is drawn by
                    // `render::item_cube`; both are `BlockCube` render kinds here.
                    ItemRender::Geometry(b) => ItemRenderKind::BlockCube(b),
                    ItemRender::Model(kind) => ItemRenderKind::Model(kind),
                }
            }
            None => match self.item_model() {
                Some(kind) => ItemRenderKind::Model(kind),
                None => ItemRenderKind::Sprite(self.item_sprite()),
            },
        }
    }

    /// First-person hold orientation for this item when held as a sprite (tools,
    /// flowers, raw drops), read from its [`ItemDef`](definition::ItemDef) row.
    /// Pickaxes are laid diagonally like a swung tool; everything else carries
    /// [`HeldPose::DEFAULT`] (upright). Only meaningful for `Sprite` render-kind
    /// items — block-cube items use the cube hold transform instead.
    #[inline]
    pub fn held_pose(self) -> HeldPose {
        self.def().held_pose
    }

    /// The flat atlas sprite for an item drawn as a billboard — item-only items
    /// (tools + raw drops) and the doors/torch (which place a block but show a
    /// flat icon). Read from the item's data row (`sprite` in `items.json`).
    /// Cube/cross/model block-items get their icon from the block and never call
    /// this; the stick fallback mirrors the old defensive default for a row that
    /// should carry a sprite but doesn't.
    #[inline]
    fn item_sprite(self) -> Tile {
        self.def()
            .sprite
            .unwrap_or_else(|| Tile::from_name("stick").expect("atlas has a 'stick' tile"))
    }

    /// The bbmodel an ITEM-ONLY item renders as — held, dropped, and as its slot
    /// icon — or `None` for the flat-sprite item-only items. Read from the
    /// item's data row (`model` in `items.json`); the model counterpart of
    /// [`item_sprite`](Self::item_sprite). Block-items carry their model on
    /// their block's render shape and never consult this.
    #[inline]
    fn item_model(self) -> Option<crate::block_model::BlockModelKind> {
        self.def().model
    }

    #[inline]
    fn def(self) -> &'static definition::ItemDef {
        data::def(self)
    }
}
