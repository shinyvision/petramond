//! Gold's edge: NONDESTRUCTIVE mining. A golden tool of the right kind
//! takes the block ITSELF — stone comes up as stone, not cobblestone; a
//! grass block as the grass block, not dirt; clay as the clay block, not
//! balls. This is the identity that justifies gold's tier-3-but-hand-slow
//! stats: gold does not mine faster or deeper, it mines GENTLY.
//!
//! The whole feature is policy over three primitive seams:
//!
//! - `block_break_pre` carries the breaking player and a MUTABLE `drops`
//!   override — `Some(stacks)` replaces the engine's drop tables for that
//!   one break;
//! - `player_held` answers the breaker's held stack WITH instance data (so
//!   an anvil-augmented tool is visible as one);
//! - `block_info` answers the block row's harvest facts — material, the
//!   tool kind the break gate credits, and the item that places the block —
//!   so this module never re-derives the engine's material→tool ladder.
//!
//! The rules, all data-driven:
//!
//! - WHICH tools mine gently is row data: any item carrying
//!   `forge:nondestructive` (the three gold tools; another pack extends the
//!   set with a patch row).
//! - The APPROPRIATE tool is required: the held tool's kind must be the
//!   block's own preferred kind, and the break must pass the harvest gate
//!   (`harvested`) — a gold shovel does nothing gentle to stone.
//! - ORES are exempt (`material == "ore"` still drops the raw ore): the
//!   gate to metals stays the pickaxe ladder, not a gold duplication trick.
//! - A block no item places cannot be taken (nothing to give).
//! - A DIAMOND-TIPPED gold tool (any `forge:augments` on the stack) mines
//!   at augment speed but slips 10% of the time — the tip's hard edge
//!   sometimes shatters what gold alone would have lifted whole. Seeded
//!   stream RNG, deterministic on the tick like everything else.

use std::collections::HashSet;

use mod_sdk::*;

use crate::anvil::AUGMENTS_KEY;

/// Row-data key marking an item as a gentle miner.
const NONDESTRUCTIVE_KEY: &str = "forge:nondestructive";
/// Seeded RNG stream for the augmented slip roll.
const SLIP_STREAM: &str = "gold_slip";
/// One in this many augmented gentle breaks slips to ordinary drops.
const SLIP_ONE_IN: u64 = 10;

#[derive(Default)]
pub struct Gold {
    /// Item registry NAMES carrying `forge:nondestructive`.
    tools: HashSet<String>,
}

impl Gold {
    /// Resolve the nondestructive tool set from row data. The member count
    /// is logged — the only cheap signal the data key and this module's
    /// constant are the same string (the tag-typo trap).
    pub fn resolve() -> Gold {
        let ids: Vec<ItemId> = items_with_data(NONDESTRUCTIVE_KEY)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let tools: HashSet<String> = item_names(ids).into_iter().flatten().collect();
        log(&format!("forge: {} nondestructive tools", tools.len()));
        Gold { tools }
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// The `block_break_pre` policy: replace the drops with the block's own
    /// item when a listed tool of the right kind harvested it (and the
    /// augmented slip roll allows).
    pub fn on_block_break(
        &self,
        block: BlockId,
        harvested: bool,
        player: PlayerId,
        drops: &mut Option<Vec<ItemStackData>>,
    ) {
        // Another handler's override is not ours to fight over.
        if drops.is_some() || !harvested {
            return;
        }
        let Some(held) = player_held(player) else {
            return;
        };
        let tool_kind = item_info(&held.item).and_then(|i| i.tool).map(|t| t.kind);
        let Some(info) = block_info(block) else {
            return;
        };
        let Some(item) = replacement(&self.tools, &held, tool_kind.as_deref(), &info) else {
            return;
        };
        // The slip is rolled LAST, so the stream advances only on breaks the
        // policy would otherwise replace — an augmented tool slips on one in
        // SLIP_ONE_IN of its gentle breaks, not of all its breaks.
        let augmented = held.data.iter().any(|(k, _)| k == AUGMENTS_KEY);
        if augmented && rng_u64(SLIP_STREAM).is_multiple_of(SLIP_ONE_IN) {
            return;
        }
        let Some(name) = item_names(vec![item]).pop().flatten() else {
            return;
        };
        *drops = Some(vec![ItemStackData {
            item: name,
            count: 1,
            data: Vec::new(),
        }]);
    }
}

/// The pure gate: the block's own placing item, when `held` is a listed
/// gentle tool whose kind is the one the block's break gate credits and the
/// block is not an ore. `None` = leave the engine's drops alone.
fn replacement(
    tools: &HashSet<String>,
    held: &ItemStackData,
    tool_kind: Option<&str>,
    info: &BlockInfoData,
) -> Option<ItemId> {
    if !tools.contains(&held.item) {
        return None;
    }
    if info.material == "ore" {
        return None;
    }
    if tool_kind.is_none() || info.preferred_tool.as_deref() != tool_kind {
        return None;
    }
    info.item
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(material: &str, preferred: Option<&str>, item: Option<u16>) -> BlockInfoData {
        BlockInfoData {
            material: material.into(),
            hardness: 1.5,
            harvest_tier: 1,
            preferred_tool: preferred.map(|s| s.to_owned()),
            item: item.map(ItemId),
        }
    }

    fn held(item: &str) -> ItemStackData {
        ItemStackData {
            item: item.into(),
            count: 1,
            data: Vec::new(),
        }
    }

    /// The whole gate in one table: only a LISTED tool of the block's OWN
    /// preferred kind takes a non-ore block with a placing item.
    #[test]
    fn only_the_appropriate_listed_tool_takes_the_block_itself() {
        let tools: HashSet<String> = ["forge:gold_pickaxe".to_owned()].into();
        let stone = info("stone", Some("pickaxe"), Some(300));
        // The happy path: gold pickaxe on stone → the stone item.
        assert_eq!(
            replacement(&tools, &held("forge:gold_pickaxe"), Some("pickaxe"), &stone),
            Some(ItemId(300))
        );
        // An unlisted tool (the plain iron pickaxe) changes nothing.
        assert_eq!(
            replacement(&tools, &held("forge:iron_pickaxe"), Some("pickaxe"), &stone),
            None
        );
        // The wrong kind — a gold SHOVEL on stone — changes nothing, even
        // though the shovel is listed for its own materials.
        let tools_shovel: HashSet<String> = ["forge:gold_shovel".to_owned()].into();
        assert_eq!(
            replacement(&tools_shovel, &held("forge:gold_shovel"), Some("shovel"), &stone),
            None
        );
        // Ores keep their raw drop regardless of gold.
        let ore = info("ore", Some("pickaxe"), Some(301));
        assert_eq!(
            replacement(&tools, &held("forge:gold_pickaxe"), Some("pickaxe"), &ore),
            None
        );
        // A block nothing places cannot be taken.
        let unplaceable = info("stone", Some("pickaxe"), None);
        assert_eq!(
            replacement(&tools, &held("forge:gold_pickaxe"), Some("pickaxe"), &unplaceable),
            None
        );
        // A hand-mined material (no preferred tool) is not this feature's
        // territory — nothing counts as "the appropriate tool" for it.
        let plant = info("plant", None, Some(302));
        assert_eq!(
            replacement(&tools, &held("forge:gold_pickaxe"), Some("pickaxe"), &plant),
            None
        );
    }
}
