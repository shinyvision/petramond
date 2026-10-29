use std::collections::BTreeMap;

use crate::content::BLUEPRINT;
use crate::geometry::feet_of;
use crate::host::prelude::*;
use crate::project::Project;
use crate::survey::{key_of, ItemKey};
use crate::worker::stance::Search;
use crate::worker::{Body, Ctx, Job, Step, Then};

use super::trip::{approach, at_container, deposit_container, hands_in_everything};
use super::wants::{tool_kind, wanted};

pub struct Trip {
    pub step: Step,
    pub to: [i32; 3],
    pub slots: Vec<Option<ItemStackData>>,
}

pub fn preview(
    ctx: &mut Ctx,
    job: &Job,
    body: &Body,
    project: &Project,
    deposit: bool,
) -> Search<Trip> {
    let stock = ctx.supplies.stock(project.table);
    if !stock.read {
        return Search::Busy;
    }
    let load = wanted(ctx, job, &body.slots, &stock.totals);
    let container = if deposit {
        deposit_container(&stock, body)
    } else {
        load.container(ctx, body, &stock)
    };
    let Some(container) = container else {
        return Search::None;
    };
    let then = if deposit {
        Then::Deposit(container)
    } else {
        Then::Fetch(container)
    };
    let step = match approach(ctx, job, body, project.home, container, then) {
        Search::Found(step) => step,
        Search::Busy => return Search::Busy,
        Search::None | Search::Unseen => return Search::None,
    };
    let Step::Walk { goal: to, .. } = step else {
        return Search::None;
    };
    let at = Body {
        cell: to,
        pos: feet_of(to),
        slots: Vec::new(),
        ..*body
    };
    let mut slots = body.slots.clone();
    let mut supplies = stock.slots.clone();
    let returns: Vec<(usize, u8)> = if deposit && hands_in_everything(project) {
        slots
            .iter()
            .enumerate()
            .filter_map(|(slot, stack)| {
                let stack = stack.as_ref()?;
                (stack.item != BLUEPRINT).then_some((slot, stack.count))
            })
            .collect()
    } else {
        load.returns(ctx, &slots)
    };
    for (slot, count) in returns {
        let stack = slots[slot].as_ref().unwrap();
        let mut left = count;
        for (cell, supply) in stock.containers.iter().zip(&mut supplies) {
            if at_container(&at, *cell) {
                left = stow(
                    ctx,
                    supply,
                    ItemStackData {
                        count: left,
                        ..stack.clone()
                    },
                );
            }
            if left == 0 {
                break;
            }
        }
        if left > 0 {
            return Search::None;
        }
        let stack = slots[slot].as_mut().unwrap();
        stack.count -= count;
        if stack.count == 0 {
            slots[slot] = None;
        }
    }
    if !deposit {
        let mut missing = load.missing(&slots);
        let mut tools: BTreeMap<String, (f32, ItemStackData)> = BTreeMap::new();
        for (cell, supply) in stock.containers.iter().zip(&supplies) {
            if !at_container(&at, *cell) {
                continue;
            }
            for stack in supply.iter().flatten() {
                if let Some((kind, speed)) = tool_kind(ctx, &stack.item) {
                    if load.tools.contains(&kind)
                        && tools.get(&kind).is_none_or(|(best, _)| speed > *best)
                    {
                        tools.insert(
                            kind,
                            (
                                speed,
                                ItemStackData {
                                    count: 1,
                                    ..stack.clone()
                                },
                            ),
                        );
                    }
                } else if let Some(want) = missing.get_mut(&key_of(stack)) {
                    let count = (*want).min(u32::from(stack.count)) as u8;
                    let left = stow(
                        ctx,
                        &mut slots,
                        ItemStackData {
                            count,
                            ..stack.clone()
                        },
                    );
                    *want -= u32::from(count - left);
                }
            }
        }
        for (_, (_, stack)) in tools {
            stow(ctx, &mut slots, stack);
        }
    }
    if slots == body.slots {
        return Search::None;
    }
    Search::Found(Trip { step, to, slots })
}

fn stow(ctx: &mut Ctx, slots: &mut [Option<ItemStackData>], mut stack: ItemStackData) -> u8 {
    let most = ctx.caches.max_stack(&stack.item);
    let key: ItemKey = key_of(&stack);
    for held in slots.iter_mut().flatten().filter(|s| key_of(s) == key) {
        let count = stack.count.min(most.saturating_sub(held.count));
        held.count += count;
        stack.count -= count;
    }
    for slot in slots.iter_mut().filter(|s| s.is_none()) {
        if stack.count == 0 {
            break;
        }
        let count = stack.count.min(most);
        *slot = Some(ItemStackData {
            count,
            ..stack.clone()
        });
        stack.count -= count;
    }
    stack.count
}
