//! Gold tools of the right kind pick up the block itself, so stone stays stone and grass stays
//! grass. Ores still drop raw ore.
//!
//! Which tools do this is row data (`forge:nondestructive`). The gold inlay gives an augmented
//! tool its own chance at it, and a diamond tip makes it slip now and then.

use std::collections::{HashMap, HashSet};

use mod_sdk::*;

use crate::augments::{Record, AUGMENTS_KEY};
use crate::keys;
use crate::schema::{read_rows, FitSpec, NondestructiveSpec};

const SLIP_STREAM: &str = "gold_slip";
const SLIP_ONE_IN: u64 = 10;
const GENTLE_STREAM: &str = "gold_gentle";

struct Grant {
    kind: String,
    chance: u8,
    extra: HashSet<BlockId>,
}

#[derive(Default)]
pub struct Gold {
    tools: HashMap<String, HashSet<BlockId>>,
    granted: HashMap<String, Vec<Grant>>,
}

impl Gold {
    pub fn resolve() -> Gold {
        let tools = read_rows::<NondestructiveSpec>(keys::NONDESTRUCTIVE_DATA)
            .into_iter()
            .filter(|(_, spec)| spec.enabled())
            .map(|(item, spec)| (item, tag_blocks(spec.tags())))
            .collect::<HashMap<_, _>>();
        let mut granted: HashMap<String, Vec<Grant>> = HashMap::new();
        for fit in read_rows::<Vec<FitSpec>>(keys::AUGMENT_DATA)
            .into_values()
            .flatten()
        {
            let Some(gentle) = &fit.gentle else {
                continue;
            };
            granted.entry(fit.overlay.clone()).or_default().push(Grant {
                kind: fit.tool.clone(),
                chance: gentle.chance(),
                extra: tag_blocks(&gentle.blocks),
            });
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

    pub fn on_block_break(
        &self,
        block: BlockId,
        harvested: bool,
        player: PlayerId,
        drops: &mut Option<Vec<ItemStackData>>,
    ) -> Option<String> {
        if drops.is_some() || !harvested {
            return None;
        }
        let held = player_held(player)?;
        let held_kind = item_info(&held.item).and_then(|i| i.tool).map(|t| t.kind);
        let info = block_info(block)?;
        let rec = Record::of_stack(&held.data);

        if let Some(extra) = self.tools.get(&held.item) {
            let item = gentle_target(block, &info, held_kind.as_deref(), extra)?;
            let augmented = match &rec {
                Some(r) => r.installed().next().is_some(),
                None => held.data.iter().any(|(k, _)| k == AUGMENTS_KEY),
            };
            if augmented && rng_u64(SLIP_STREAM).is_multiple_of(SLIP_ONE_IN) {
                return None;
            }
            give(item, drops);
            return None;
        }

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

fn tag_blocks(tags: &[String]) -> HashSet<BlockId> {
    tags.iter().flat_map(|tag| blocks_by_tag(tag)).collect()
}

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
            collision: vec![([0.0; 3], [1.0; 3])],
            fluid: None,
            replaceable: false,
            interaction: None,
        }
    }

    #[test]
    fn only_the_appropriate_tool_or_listed_ground_is_taken_gently() {
        let none = HashSet::new();
        let stone = info("stone", Some("pickaxe"), Some(300));
        assert_eq!(
            gentle_target(BlockId(7), &stone, Some("pickaxe"), &none),
            Some(ItemId(300))
        );
        assert_eq!(
            gentle_target(BlockId(7), &stone, Some("shovel"), &none),
            None
        );
        let leaves = info("foliage", None, Some(310));
        assert_eq!(gentle_target(BlockId(9), &leaves, Some("axe"), &none), None);
        let extra: HashSet<BlockId> = [BlockId(9)].into();
        assert_eq!(
            gentle_target(BlockId(9), &leaves, Some("axe"), &extra),
            Some(ItemId(310))
        );
        assert_eq!(
            gentle_target(BlockId(10), &leaves, Some("axe"), &extra),
            None
        );
        let ore = info("ore", Some("pickaxe"), Some(301));
        let ore_extra: HashSet<BlockId> = [BlockId(11)].into();
        assert_eq!(
            gentle_target(BlockId(11), &ore, Some("pickaxe"), &ore_extra),
            None
        );
        let unplaceable = info("stone", Some("pickaxe"), None);
        assert_eq!(
            gentle_target(BlockId(7), &unplaceable, Some("pickaxe"), &none),
            None
        );
        let plant = info("plant", None, Some(302));
        assert_eq!(
            gentle_target(BlockId(8), &plant, Some("pickaxe"), &none),
            None
        );
    }
}
