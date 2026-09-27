use machine_core::StepCtx;
use mod_sdk::*;

use crate::keys;
use crate::schema::UpgradeSpec;

pub const KEY: &str = "forge:fittings";
pub const INFO_KEY: &str = "petramond:info";
pub const KINDS: [&str; 4] = ["quench", "counterweight", "chute", "stoker"];

#[derive(Default)]
pub struct Fittings {
    pub rows: Vec<Upgrade>,
}

pub struct Upgrade {
    pub index: usize,
    pub name: String,
    pub info: String,
    pub icon: String,
    pub cost: Vec<(String, u8)>,
    pub set_ticks: u32,
    pub dwell_ticks: u32,
    pub feed_every: u32,
    pub feed_keep: u8,
}

pub fn record(bytes: &[u8]) -> u8 {
    match bytes {
        [1, bits] => bits & 15,
        _ => 0,
    }
}

impl Fittings {
    pub fn resolve() -> Self {
        let specs = resolve_block(keys::FORGING_FURNACE)
            .and_then(|b| block_data(b, keys::UPGRADES_DATA))
            .and_then(|b| String::from_utf8(b).ok())
            .map(|text| parse_row_data::<Vec<UpgradeSpec>>(&text))
            .unwrap_or_else(|| Ok(Vec::new()));
        let specs = specs.unwrap_or_else(|reason| {
            log(&row_error(
                keys::UPGRADES_DATA,
                keys::FORGING_FURNACE,
                &reason,
            ));
            Vec::new()
        });
        let rows = specs
            .into_iter()
            .filter_map(|spec| {
                let upgrade = Upgrade::from_spec(&spec);
                if upgrade.is_none() {
                    let reason = format!("unknown fitting kind '{}'", spec.kind);
                    log(&row_error(
                        keys::UPGRADES_DATA,
                        keys::FORGING_FURNACE,
                        &reason,
                    ));
                }
                upgrade
            })
            .collect();
        Self { rows }
    }

    pub fn active(&self, bits: u8, index: usize) -> Option<&Upgrade> {
        self.rows
            .iter()
            .find(|r| r.index == index && bits & (1 << index) != 0)
    }

    pub fn buy(&self, anchor: [i32; 3], index: usize) {
        let Some(row) = self.rows.iter().find(|r| r.index == index) else {
            return;
        };
        let mut bits = record(&section_kv_get(anchor, KEY).unwrap_or_default());
        if bits & (1 << row.index) != 0 {
            return;
        }
        let Some(player) = player_state().id else {
            return;
        };
        let Some(inventory) = player_inventory(player) else {
            return;
        };
        let Some(plan) = purchase_plan(&inventory, &row.cost) else {
            return;
        };
        for stack in plan {
            let data: Vec<_> = stack
                .data
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_slice()))
                .collect();
            if take_item(player, &stack.item, stack.count, Some(&data)).is_none() {
                log("forge: purchase inventory changed during synchronous dispatch");
                return;
            }
        }
        bits |= 1 << row.index;
        section_kv_set(anchor, KEY, vec![1, bits]);
        let icons = self
            .rows
            .iter()
            .filter(|r| bits & (1 << r.index) != 0)
            .map(|r| r.icon.as_str())
            .collect::<Vec<_>>()
            .join(",");
        section_kv_set(anchor, INFO_KEY, format!("Upgrades\n{icons}").into_bytes());
    }

    pub fn publish(&self, ctx: &StepCtx<'_>, bits: u8) {
        for &player in ctx.viewers {
            let inventory = player_inventory(player).unwrap_or_default();
            for row in &self.rows {
                let bought = bits & (1 << row.index) != 0;
                let affordable = purchase_plan(&inventory, &row.cost).is_some();
                let page = &keys::fittings::TABLE[row.index];
                for (key, value) in [
                    (page.name, GuiValue::Str(row.name.clone())),
                    (
                        page.image,
                        GuiValue::Str(format!("../icons/{}.png", row.icon)),
                    ),
                    (
                        page.inactive_image,
                        GuiValue::Str(format!("../icons/{}_inactive.png", row.icon)),
                    ),
                    (page.unbought, GuiValue::I32(i32::from(!bought))),
                    (page.info, GuiValue::Str(row.info.clone())),
                    (
                        page.cost,
                        GuiValue::List(
                            row.cost
                                .iter()
                                .map(|(item, count)| {
                                    [
                                        ("item".into(), GuiValue::Str(item.clone())),
                                        ("count".into(), GuiValue::Str(format!("×{count}"))),
                                    ]
                                    .into_iter()
                                    .collect()
                                })
                                .collect(),
                        ),
                    ),
                    (
                        page.status,
                        GuiValue::Str(
                            if bought {
                                ""
                            } else if affordable {
                                "Affordable — click to buy"
                            } else {
                                "Missing materials"
                            }
                            .into(),
                        ),
                    ),
                    (
                        page.palette,
                        GuiValue::Str(
                            if bought || affordable {
                                "accent"
                            } else {
                                "danger"
                            }
                            .into(),
                        ),
                    ),
                    (page.on, GuiValue::I32(i32::from(!bought && affordable))),
                    (page.bought, GuiValue::I32(i32::from(bought))),
                ] {
                    gui_state_set_for(player, key, value);
                }
            }
        }
    }
}

impl Upgrade {
    fn from_spec(spec: &UpgradeSpec) -> Option<Self> {
        let number = |value: Option<f64>, fallback: u32| {
            value.unwrap_or(f64::from(fallback)).max(1.0) as u32
        };
        Some(Self {
            index: KINDS.iter().position(|k| *k == spec.kind)?,
            name: spec.name.clone(),
            info: spec.info.clone(),
            icon: spec.icon.clone(),
            cost: spec
                .cost
                .iter()
                .map(|c| (c.item.clone(), c.count.max(1)))
                .collect(),
            set_ticks: number(spec.set_ticks, super::SET_TICKS),
            dwell_ticks: number(spec.dwell_ticks, 20),
            feed_every: number(spec.feed_every, 20),
            feed_keep: number(spec.feed_keep, 8).min(64) as u8,
        })
    }
}

fn purchase_plan(
    inventory: &[Option<ItemStackData>],
    cost: &[(String, u8)],
) -> Option<Vec<ItemStackData>> {
    let mut inventory = inventory.to_vec();
    let mut plan = Vec::new();
    for (item, count) in cost {
        let mut remaining = *count;
        for slot in inventory.iter_mut().flatten().filter(|s| &s.item == item) {
            let n = remaining.min(slot.count);
            if n == 0 {
                continue;
            }
            plan.push(ItemStackData {
                count: n,
                ..slot.clone()
            });
            slot.count -= n;
            remaining -= n;
        }
        if remaining > 0 {
            return None;
        }
    }
    Some(plan)
}

#[cfg(test)]
mod tests;
