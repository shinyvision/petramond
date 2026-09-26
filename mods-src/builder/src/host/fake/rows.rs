//! The fake world's registry: a handful of rows standing for the kinds of
//! block the golem meets (ground, scaffolding, panes, doors, chests, leaves,
//! fluid, bedrock, fences, fixtures, plants) and the items and tools that go
//! with them. Ids follow the order rows are listed in.

use crate::host::prelude::*;

pub const AIR: BlockId = BlockId(0);
pub const STONE: BlockId = BlockId(1);
pub const DIRT: BlockId = BlockId(2);
pub const PLANKS: BlockId = BlockId(3);
pub const PANE: BlockId = BlockId(4);
pub const DOOR: BlockId = BlockId(5);
pub const CHEST: BlockId = BlockId(6);
pub const TABLE: BlockId = BlockId(7);
pub const LEAVES: BlockId = BlockId(8);
pub const WATER: BlockId = BlockId(9);
pub const BEDROCK: BlockId = BlockId(10);
pub const FENCE: BlockId = BlockId(11);
pub const TORCH: BlockId = BlockId(12);
pub const GRASS: BlockId = BlockId(13);

pub const GOLEM_KIND: MobId = MobId(0);
/// Slots a golem carries (the pack's `container_slots`).
pub const GOLEM_SLOTS: usize = 27;
/// A golem's health when it spawns (the pack's `petramond:health`).
pub const GOLEM_HEALTH: f32 = 60.0;

pub struct BlockRow {
    pub name: &'static str,
    pub info: BlockInfoData,
    pub tags: Vec<String>,
    pub data: Vec<(String, String)>,
    /// The cells an object of this row fills, relative to its anchor.
    pub footprint: Vec<[i32; 3]>,
}

pub struct ItemRow {
    pub name: &'static str,
    pub info: ItemInfoData,
}

const CUBE: ([f32; 3], [f32; 3]) = ([0.0; 3], [1.0; 3]);

fn block(name: &'static str, collision: Vec<([f32; 3], [f32; 3])>) -> BlockRow {
    BlockRow {
        name,
        info: BlockInfoData {
            material: "none".into(),
            hardness: 0.0,
            harvest_tier: 0,
            preferred_tool: None,
            item: None,
            collision,
            fluid: None,
            replaceable: false,
            interaction: None,
        },
        tags: Vec::new(),
        data: Vec::new(),
        footprint: vec![[0, 0, 0]],
    }
}

impl BlockRow {
    fn hard(mut self, hardness: f32, tier: u8, tool: Option<&str>) -> Self {
        self.info.hardness = hardness;
        self.info.harvest_tier = tier;
        self.info.preferred_tool = tool.map(str::to_owned);
        self
    }

    fn placed_by(mut self, item: u16) -> Self {
        self.info.item = Some(ItemId(item));
        self
    }

    fn replaceable(mut self) -> Self {
        self.info.replaceable = true;
        self
    }

    fn used(mut self, interaction: BlockUse) -> Self {
        self.info.interaction = Some(interaction);
        self
    }

    fn tagged(mut self, tag: &str) -> Self {
        self.tags.push(tag.into());
        self
    }

    fn with_data(mut self, key: &str) -> Self {
        self.data.push((key.into(), "true".into()));
        self
    }
}

fn item(name: &'static str, display: &str, block: Option<BlockId>) -> ItemRow {
    ItemRow {
        name,
        info: ItemInfoData {
            max_stack: 64,
            fuel_burn_ticks: 0,
            tags: Vec::new(),
            display_name: display.into(),
            block,
            tool: None,
            food: None,
            item_use: None,
        },
    }
}

fn tool(name: &'static str, display: &str, kind: &str, tier: u8, speed: f32) -> ItemRow {
    let mut row = item(name, display, None);
    row.info.max_stack = 1;
    row.info.tool = Some(ToolInfoData {
        kind: kind.into(),
        tier,
        speed,
        damage: [1.0, 2.0],
        knockback: 1.0,
    });
    row
}

pub fn blocks() -> Vec<BlockRow> {
    let water = FluidInfoData {
        delay: 5,
        drop_off: 1,
        renewable: true,
        quench: None,
        contact_damage: None,
        applies: None,
        clears: Vec::new(),
        destroys_items: false,
    };
    let mut door = block("petramond:oak_door", vec![([0.0; 3], [1.0, 1.0, 0.1875])])
        .hard(3.0, 0, Some("axe"))
        .placed_by(4)
        .used(BlockUse::ToggleDoor);
    door.footprint = vec![[0, 0, 0], [0, 1, 0]];
    let mut fluid = block("petramond:water", Vec::new()).replaceable();
    fluid.info.fluid = Some(water);
    vec![
        block("petramond:air", Vec::new()).replaceable(),
        block("petramond:stone", vec![CUBE])
            .hard(1.5, 1, Some("pickaxe"))
            .placed_by(0),
        block("petramond:dirt", vec![CUBE])
            .hard(0.5, 0, Some("shovel"))
            .placed_by(1)
            .with_data(crate::keys::SCAFFOLDING_DATA),
        block("petramond:oak_planks", vec![CUBE])
            .hard(2.0, 0, Some("axe"))
            .placed_by(2)
            .with_data(crate::keys::SCAFFOLDING_DATA),
        block(
            "petramond:glass_pane",
            vec![([0.4375, 0.0, 0.0], [0.5625, 1.0, 1.0])],
        )
        .hard(0.3, 0, None)
        .placed_by(3),
        door,
        block(
            "petramond:chest",
            vec![([0.0625, 0.0, 0.0625], [0.9375, 0.875, 0.9375])],
        )
        .hard(2.5, 0, Some("axe"))
        .placed_by(5)
        .used(BlockUse::OpenGui)
        .with_data(crate::keys::SUPPLY_DATA),
        block(crate::keys::TABLE_BLOCK, Vec::new())
            .hard(2.5, 0, Some("axe"))
            .placed_by(6)
            .used(BlockUse::OpenGui),
        block("petramond:oak_leaves", vec![CUBE])
            .hard(0.2, 0, None)
            .placed_by(7)
            .tagged("leaves"),
        fluid,
        block("petramond:bedrock", vec![CUBE]).hard(-1.0, 0, None),
        block(
            "petramond:oak_fence",
            vec![([0.375, 0.0, 0.375], [0.625, 1.5, 0.625])],
        )
        .hard(2.0, 0, Some("axe"))
        .placed_by(8),
        block("petramond:torch", Vec::new())
            .placed_by(9)
            .tagged("fragile"),
        block("petramond:grass", Vec::new()).replaceable(),
    ]
}

pub fn items() -> Vec<ItemRow> {
    vec![
        item("petramond:stone", "Stone", Some(STONE)),
        item("petramond:dirt", "Dirt", Some(DIRT)),
        item("petramond:oak_planks", "Oak Planks", Some(PLANKS)),
        item("petramond:glass_pane", "Glass Pane", Some(PANE)),
        item("petramond:oak_door", "Oak Door", Some(DOOR)),
        item("petramond:chest", "Chest", Some(CHEST)),
        item(crate::keys::TABLE_ITEM, "Schematic Table", Some(TABLE)),
        item("petramond:oak_leaves", "Oak Leaves", Some(LEAVES)),
        item("petramond:oak_fence", "Oak Fence", Some(FENCE)),
        item("petramond:torch", "Torch", Some(TORCH)),
        {
            let mut blueprint = item(crate::keys::BLUEPRINT, "Blueprint", None);
            blueprint.info.max_stack = 1;
            blueprint
        },
        item("petramond:raw_copper", "Raw Copper", None),
        tool(
            "petramond:stone_pickaxe",
            "Stone Pickaxe",
            "pickaxe",
            2,
            4.0,
        ),
        tool("petramond:stone_shovel", "Stone Shovel", "shovel", 2, 4.0),
        tool("petramond:stone_axe", "Stone Axe", "axe", 2, 4.0),
    ]
}

pub fn mobs() -> Vec<&'static str> {
    vec![crate::keys::GOLEM]
}
