//! Gold's edge: NONDESTRUCTIVE ("gentle") mining. A golden tool of the right
//! kind takes the block ITSELF — stone comes up as stone, not cobblestone; a
//! grass block as the grass block, not dirt; a leaf block under a gold axe as
//! the leaf block; clay as the clay block, not balls. This is the identity
//! that justifies gold's tier-3-but-hand-slow stats: gold does not mine
//! faster or deeper, it mines GENTLY.
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
//! - WHICH tools mine gently INNATELY is row data: any item carrying
//!   `forge:nondestructive` (the three gold tools; another pack extends the
//!   set with a patch row). The value may name extra ground: `{"blocks":
//!   ["petramond:leaves"]}` — block TAGS this tool gently takes beyond its
//!   preferred-kind rule (leaves have no preferred tool at all, so the gold
//!   axe needs the explicit pairing).
//! - A tool AUGMENTED with a gentle-granting fit (`gentle` on a
//!   `forge:augment` fit — the gold inlay) mines gently at the fit's own
//!   CHANCE (`{"chance": 25, "blocks"?: [...]}`): a rolled fraction of its
//!   gentle-eligible breaks takes the block, the rest drop as usual.
//! - The APPROPRIATE tool is required: the held tool's kind must be the
//!   block's own preferred kind (or the block must be in the declared extra
//!   set), and the break must pass the harvest gate (`harvested`).
//! - ORES are exempt (`material == "ore"` still drops the raw ore): the
//!   gate to metals stays the pickaxe ladder, not a gold duplication trick.
//! - A block no item places cannot be taken (nothing to give).
//! - A DIAMOND-TIPPED gold tool (any INSTALLED augment on the record) mines
//!   at augment speed but slips 10% of the time — the tip's hard edge
//!   sometimes shatters what gold alone would have lifted whole. Seeded
//!   stream RNG, deterministic on the tick like everything else; every roll
//!   happens only on breaks the policy would otherwise replace, so the
//!   streams advance on candidates, never on bystanders.

use std::collections::{HashMap, HashSet};

use mod_sdk::*;

use crate::augments::{Record, AUGMENTS_KEY, AUGMENT_KEY, NONDESTRUCTIVE_KEY};
use crate::content::read_rows;

/// Seeded RNG stream for the augmented slip roll.
const SLIP_STREAM: &str = "gold_slip";
/// One in this many augmented gentle breaks slips to ordinary drops.
const SLIP_ONE_IN: u64 = 10;
/// Seeded RNG stream for the augment-granted gentle chance.
const GENTLE_STREAM: &str = "gold_gentle";

/// A gentle grant carried by one augment fit, resolved per tool kind.
struct Grant {
    /// The tool KIND the owning fit is for (the fit vocabulary's `tool`).
    kind: String,
    /// Percent of gentle-eligible breaks that take the block (1..=100).
    chance: u8,
    /// Blocks taken beyond the preferred-kind rule (resolved from tags).
    extra: HashSet<BlockId>,
}

#[derive(Default)]
pub struct Gold {
    /// Innately gentle tools: item registry NAME → extra blocks beyond the
    /// preferred-kind rule.
    tools: HashMap<String, HashSet<BlockId>>,
    /// Gentle grants by augment IDENTITY (a record entry on a held tool).
    granted: HashMap<String, Vec<Grant>>,
}

impl Gold {
    /// Resolve both policy tables from row data. The member counts are
    /// logged — the only cheap signal the data keys and this module's
    /// constants are the same strings (the tag-typo trap).
    pub fn resolve() -> Gold {
        let tools = read_rows(NONDESTRUCTIVE_KEY, |value| Some(tag_blocks(value.get("blocks"))));
        let mut granted: HashMap<String, Vec<Grant>> = HashMap::new();
        let grant_rows = read_rows(AUGMENT_KEY, |value| {
            let grants: Vec<(String, Grant)> = value
                .as_array()?
                .iter()
                .filter_map(|f| {
                    let gentle = f.get("gentle")?;
                    Some((
                        f.get("overlay")?.as_str()?.to_owned(),
                        Grant {
                            kind: f.get("tool")?.as_str()?.to_owned(),
                            chance: gentle
                                .get("chance")
                                .and_then(|c| c.as_u8())
                                .unwrap_or(100)
                                .clamp(1, 100),
                            extra: tag_blocks(gentle.get("blocks")),
                        },
                    ))
                })
                .collect();
            Some(grants)
        });
        for (identity, grant) in grant_rows.into_values().flatten() {
            granted.entry(identity).or_default().push(grant);
        }
        log(&format!(
            "forge: {} nondestructive tools, {} gentle-granting augments",
            tools.len(),
            granted.len()
        ));
        Gold { tools, granted }
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty() && self.granted.is_empty()
    }

    /// The `block_break_pre` policy: replace the drops with the block's own
    /// item when a gentle tool of the right kind harvested it (and the
    /// rolls allow). Returns the augment IDENTITY whose grant fired, when
    /// one did — the caller reports it as a wear PROC (the innate path is
    /// the tool's own metal and never wears).
    pub fn on_block_break(
        &self,
        block: BlockId,
        harvested: bool,
        player: PlayerId,
        drops: &mut Option<Vec<ItemStackData>>,
    ) -> Option<String> {
        // Another handler's override is not ours to fight over.
        if drops.is_some() || !harvested {
            return None;
        }
        let held = player_held(player)?;
        let held_kind = item_info(&held.item).and_then(|i| i.tool).map(|t| t.kind);
        let info = block_info(block)?;
        let rec = Record::of_stack(&held.data);

        // The INNATE path: full-strength gentle mining, with the augmented
        // slip. Rolled LAST, so the stream advances only on breaks the
        // policy would otherwise replace.
        if let Some(extra) = self.tools.get(&held.item) {
            let item = gentle_target(block, &info, held_kind.as_deref(), extra)?;
            let augmented = match &rec {
                Some(r) => r.installed().next().is_some(),
                // A record this build cannot read still marks a modified
                // edge — assume augmented rather than quietly upgrading it.
                None => held.data.iter().any(|(k, _)| k == AUGMENTS_KEY),
            };
            if augmented && rng_u64(SLIP_STREAM).is_multiple_of(SLIP_ONE_IN) {
                return None;
            }
            give(item, drops);
            return None;
        }

        // The AUGMENT path: a gentle-granting identity on the held record,
        // at the grant's own chance (the highest, if several apply). A
        // BROKEN augment grants nothing — its entry stays for repair.
        let rec = rec?;
        let kind = held_kind.as_deref()?;
        let best = rec
            .entries
            .iter()
            .filter(|e| !e.id.is_empty() && e.cond > 0)
            .flat_map(|e| {
                self.granted
                    .get(&e.id)
                    .into_iter()
                    .flatten()
                    .map(move |g| (e, g))
            })
            .filter(|(_, g)| g.kind == kind)
            .filter_map(|(e, g)| {
                gentle_target(block, &info, Some(kind), &g.extra).map(|i| (e, g, i))
            })
            .max_by_key(|(_, g, _)| g.chance);
        let (entry, grant, item) = best?;
        if grant.chance < 100 && rng_u64(GENTLE_STREAM) % 100 >= grant.chance as u64 {
            return None;
        }
        give(item, drops);
        Some(entry.id.clone())
    }
}

/// Resolve a JSON list of block-TAG names into the blocks carrying them
/// (`blocks_by_tag` never interns, so a typo is an empty set — which is why
/// `resolve` logs member counts).
fn tag_blocks(value: Option<&json::Value>) -> HashSet<BlockId> {
    value
        .and_then(|b| b.as_array())
        .map(|tags| {
            tags.iter()
                .filter_map(|t| t.as_str())
                .flat_map(blocks_by_tag)
                .collect()
        })
        .unwrap_or_default()
}

/// The pure gate: the block's own placing item, when the held tool's kind is
/// the one the block's break gate credits (or the block is in the policy's
/// extra set) and the block is not an ore. `None` = leave the engine's drops
/// alone.
fn gentle_target(
    block: BlockId,
    info: &BlockInfoData,
    held_kind: Option<&str>,
    extra: &HashSet<BlockId>,
) -> Option<ItemId> {
    if info.material == "ore" {
        return None;
    }
    let kind_ok = held_kind.is_some() && info.preferred_tool.as_deref() == held_kind;
    if !(kind_ok || extra.contains(&block)) {
        return None;
    }
    info.item
}

/// Replace the break's drops with exactly one of `item`.
fn give(item: ItemId, drops: &mut Option<Vec<ItemStackData>>) {
    let Some(name) = item_names(vec![item]).pop().flatten() else {
        return;
    };
    *drops = Some(vec![ItemStackData {
        item: name,
        count: 1,
        data: Vec::new(),
    }]);
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

    /// The whole gate in one table: the right kind takes a non-ore block
    /// with a placing item; the extra set reaches blocks the kind rule
    /// cannot (leaves under an axe); ores and unplaceables never yield.
    #[test]
    fn only_the_appropriate_tool_or_listed_ground_is_taken_gently() {
        let none = HashSet::new();
        let stone = info("stone", Some("pickaxe"), Some(300));
        // The happy path: the right kind on stone → the stone item.
        assert_eq!(
            gentle_target(BlockId(7), &stone, Some("pickaxe"), &none),
            Some(ItemId(300))
        );
        // The wrong kind — a shovel on stone — changes nothing.
        assert_eq!(gentle_target(BlockId(7), &stone, Some("shovel"), &none), None);
        // Leaves have NO preferred tool; only the declared extra set
        // reaches them, and only for the block ids actually in it.
        let leaves = info("foliage", None, Some(310));
        assert_eq!(gentle_target(BlockId(9), &leaves, Some("axe"), &none), None);
        let extra: HashSet<BlockId> = [BlockId(9)].into();
        assert_eq!(
            gentle_target(BlockId(9), &leaves, Some("axe"), &extra),
            Some(ItemId(310))
        );
        assert_eq!(gentle_target(BlockId(10), &leaves, Some("axe"), &extra), None);
        // Ores keep their raw drop regardless — even listed ones.
        let ore = info("ore", Some("pickaxe"), Some(301));
        let ore_extra: HashSet<BlockId> = [BlockId(11)].into();
        assert_eq!(gentle_target(BlockId(11), &ore, Some("pickaxe"), &ore_extra), None);
        // A block nothing places cannot be taken.
        let unplaceable = info("stone", Some("pickaxe"), None);
        assert_eq!(
            gentle_target(BlockId(7), &unplaceable, Some("pickaxe"), &none),
            None
        );
        // A hand-mined material with no extra listing is out of scope.
        let plant = info("plant", None, Some(302));
        assert_eq!(gentle_target(BlockId(8), &plant, Some("pickaxe"), &none), None);
    }
}
