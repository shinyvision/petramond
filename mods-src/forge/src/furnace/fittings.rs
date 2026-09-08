use machine_core::StepCtx;
use mod_sdk::*;

pub const KEY: &str = "forge:fittings";
pub const INFO_KEY: &str = "petramond:info";
pub const PAGE: &str = "forge:forging_furnace_fittings";
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
        let rows = resolve_block("forge:forging_furnace")
            .and_then(|b| block_data(b, "forge:upgrades"))
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|s| json::Value::parse(&s))
            .and_then(|v| {
                v.as_array()
                    .map(|rows| rows.iter().filter_map(Upgrade::parse).collect())
            })
            .unwrap_or_default();
        Self { rows }
    }

    pub fn active(&self, bits: u8, index: usize) -> Option<&Upgrade> {
        self.rows
            .iter()
            .find(|r| r.index == index && bits & (1 << index) != 0)
    }

    pub fn buy(&self, anchor: [i32; 3], kind: &str) {
        let Some(row) = self.rows.iter().find(|r| KINDS[r.index] == kind) else {
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
        // A GUI dispatch is synchronous on the owning tick: nothing may change
        // inventory between this plan and its exact-variant takes.
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
                let prefix = format!("forge:fit_{}", KINDS[row.index]);
                for (suffix, value) in [
                    ("name", GuiValue::Str(row.name.clone())),
                    ("image", GuiValue::Str(format!("../icons/{}.png", row.icon))),
                    (
                        "inactive_image",
                        GuiValue::Str(format!("../icons/{}_inactive.png", row.icon)),
                    ),
                    ("unbought", GuiValue::I32(i32::from(!bought))),
                    ("info", GuiValue::Str(row.info.clone())),
                    (
                        "cost",
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
                        "status",
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
                        "palette",
                        GuiValue::Str(
                            if bought || affordable {
                                "accent"
                            } else {
                                "danger"
                            }
                            .into(),
                        ),
                    ),
                    ("on", GuiValue::I32(i32::from(!bought && affordable))),
                    ("bought", GuiValue::I32(i32::from(bought))),
                ] {
                    gui_state_set_for(player, &format!("{prefix}_{suffix}"), value);
                }
            }
        }
    }
}

impl Upgrade {
    fn parse(v: &json::Value) -> Option<Self> {
        let text = |key| v.get(key)?.as_str().map(str::to_owned);
        let number = |key, fallback| {
            v.get(key)
                .and_then(|v| v.as_f64())
                .unwrap_or(fallback as f64)
                .max(1.0) as u32
        };
        let index = KINDS
            .iter()
            .position(|k| Some(*k) == v.get("kind").and_then(|v| v.as_str()))?;
        let cost: Option<Vec<_>> = v
            .get("cost")?
            .as_array()?
            .iter()
            .map(|c| {
                Some((
                    c.get("item")?.as_str()?.to_owned(),
                    c.get("count")?.as_u8()?.max(1),
                ))
            })
            .collect();
        let cost = cost?;
        Some(Self {
            index,
            name: text("name")?,
            info: text("info")?,
            icon: text("icon")?,
            cost,
            set_ticks: number("set_ticks", super::SET_TICKS),
            dwell_ticks: number("dwell_ticks", 20),
            feed_every: number("feed_every", 20),
            feed_keep: number("feed_keep", 8).min(64) as u8,
        })
    }
}

/// Plan exact-variant takes over a private inventory copy, so duplicate cost
/// rows cannot count the same material twice and a short purchase spends nothing.
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
