//! The pack's registry names, resolved once per session.

use mod_sdk::*;

pub const TABLE_KIND: &str = "builder:schematic_table";
pub const MATERIALS_KIND: &str = "builder:materials";
pub const GOLEM_KIND: &str = "builder:golem";
pub const GOLEM: &str = "builder:mason_golem";
/// The golem row's `size.half_width` (pack `mobs.json`): its footprint.
pub const GOLEM_HALF_WIDTH: f32 = 0.3;
/// Row data a block opts into scaffolding with: any pack's plain, sturdy
/// block joins by patching its row.
pub const SCAFFOLDING_DATA: &str = "builder:scaffolding";
/// Instance data a bound blueprint carries: the world nonce and project id.
pub const PROJECT_DATA: &str = "builder:project";
pub const INFO_DATA: &str = "petramond:info";
pub const EARTH_BURST: &str = "builder:earth_burst";

pub const BLUEPRINT: &str = "builder:blueprint";
const RAW_COPPER: &str = "petramond:raw_copper";
pub const COPPER_BLOCK_RECIPE: &str = "builder:copper_block";

pub struct Content {
    pub table: BlockId,
    /// The blocks a golem may stand its scaffolding up from.
    pub scaffolding: Vec<ScaffoldKind>,
    pub golem: MobId,
    pub blueprint: ItemId,
    /// Raw copper hints at the golem core's block long before nine ingots
    /// would.
    pub raw_copper: Option<ItemId>,
}

impl Content {
    pub fn resolve() -> Option<Self> {
        Some(Self {
            table: resolve_block_logged(TABLE_KIND)?,
            scaffolding: ScaffoldKind::resolve(),
            golem: resolve_mob_logged(GOLEM)?,
            blueprint: resolve_item_logged(BLUEPRINT)?,
            raw_copper: resolve_item_logged(RAW_COPPER),
        })
    }
}

/// One block scaffolding may be made of: an ordinary block, paid for from
/// what the golem carries and dug back out when the work above is done.
#[derive(Clone, Debug)]
pub struct ScaffoldKind {
    pub block: BlockId,
    pub name: String,
    /// The item that places it and that digging it gives back.
    pub item: String,
    pub hardness: f32,
    pub tool: Option<String>,
}

impl ScaffoldKind {
    fn resolve() -> Vec<Self> {
        let blocks: Vec<BlockId> = blocks_with_data(SCAFFOLDING_DATA)
            .into_iter()
            .map(|(block, _)| block)
            .collect();
        let names = block_names(blocks.clone());
        let mut kinds = Vec::new();
        for (block, name) in blocks.into_iter().zip(names) {
            let (Some(name), Some(info)) = (name, block_info(block)) else {
                continue;
            };
            let Some(item) = info
                .item
                .and_then(|item| item_names(vec![item]).into_iter().next().flatten())
            else {
                continue;
            };
            kinds.push(Self {
                block,
                name,
                item,
                hardness: info.hardness,
                tool: info.preferred_tool.clone(),
            });
        }
        kinds
    }

    pub fn record(&self) -> BlockRecord {
        BlockRecord {
            block: self.name.clone(),
            state: Vec::new(),
            refs: Vec::new(),
            data: Vec::new(),
        }
    }
}

impl Content {
    pub fn scaffold_kind(&self, block: BlockId) -> Option<&ScaffoldKind> {
        self.scaffolding.iter().find(|k| k.block == block)
    }
}
