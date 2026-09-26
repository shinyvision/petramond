//! The pack's registry names, resolved once per session.

use crate::host::logged;
use crate::host::prelude::*;

/// The golem, its blueprint and its burst, re-exported for the worker's
/// modules; every pack id is declared once, in [`crate::keys`].
pub use crate::keys::{BLUEPRINT, EARTH_BURST, GOLEM};
use crate::keys::{RAW_COPPER, SCAFFOLDING_DATA, TABLE_BLOCK};

/// The golem row's `size.half_width` (pack `mobs.json`): its footprint.
pub const GOLEM_HALF_WIDTH: f32 = 0.3;
/// Instance data a bound blueprint carries: the world nonce and project id.
pub const PROJECT_DATA: &str = "builder:project";
pub const INFO_DATA: &str = "petramond:info";

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
            table: logged("block", TABLE_BLOCK, resolve_block(TABLE_BLOCK))?,
            scaffolding: ScaffoldKind::resolve(),
            golem: logged("mob", GOLEM, resolve_mob(GOLEM))?,
            blueprint: logged("item", BLUEPRINT, resolve_item(BLUEPRINT))?,
            raw_copper: logged("item", RAW_COPPER, resolve_item(RAW_COPPER)),
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

#[cfg(test)]
mod tests;
