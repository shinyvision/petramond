//! What the golem stands on to reach high work: ordinary blocks out of its
//! own hands. Which block is a choice — what it carries already (earth it
//! dug counts), else what the chests can best spare — and every one of them
//! is paid for going up and dug back out, with a tool where one helps,
//! coming down.

use std::collections::BTreeMap;

use mod_sdk::*;

use super::tuning::hands::SCAFFOLD_STOCK;
use super::{cargo, hands, Body, Ctx};
use crate::content::ScaffoldKind;
use crate::jobs::Job;
use crate::survey::ItemKey;

fn key(kind: &ScaffoldKind) -> ItemKey {
    (kind.item.clone(), Vec::new())
}

/// How many blocks of each scaffolding kind the golem carries.
fn carried<'a>(ctx: &'a Ctx, slots: &[Option<ItemStackData>]) -> Vec<(&'a ScaffoldKind, u32)> {
    let totals = cargo::totals(slots);
    ctx.content
        .scaffolding
        .iter()
        .filter_map(|kind| {
            totals
                .get(&key(kind))
                .copied()
                .filter(|n| *n > 0)
                .map(|n| (kind, n))
        })
        .collect()
}

/// Blocks in hand the design does not want: what may go into scaffolding
/// without borrowing from the build. Every block borrowed comes back, so
/// with nothing spare the build's own are used all the same.
fn spare(job: &Job, kind: &ScaffoldKind, held: u32) -> u32 {
    let owed = job
        .summary()
        .and_then(|s| s.bill.get(&key(kind)).copied())
        .unwrap_or(0);
    held.saturating_sub(owed)
}

/// Scaffold blocks in hand beyond what the build still wants: the house's own
/// cobblestone and planks are scaffolding kinds too, and must not count as
/// spare.
pub fn in_hand(ctx: &Ctx, job: &Job, slots: &[Option<ItemStackData>]) -> u32 {
    carried(ctx, slots)
        .iter()
        .map(|(kind, held)| spare(job, kind, *held))
        .sum()
}

/// What the task a climb or a walkway is for is laid with: never stood on.
fn spoken_for(job: &Job) -> Vec<ItemKey> {
    let task = job
        .crew
        .aloft
        .bridge
        .as_ref()
        .map(|walkway| walkway.task)
        .or(job.crew.aloft.climbed_for.map(|(task, _)| task));
    match (task, job.survey.as_ref()) {
        (Some(super::Task::Unit(i)), Some(survey)) => match &survey.known[i] {
            crate::survey::Known::Place(missing) => {
                missing.iter().map(crate::survey::key_of).collect()
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// The next scaffold's block, from what is carried: most to spare, softest to
/// dig between equals. With nothing spare the build's own blocks are borrowed,
/// never the ones the work up here waits for.
pub fn pick<'a>(ctx: &'a Ctx, job: &Job, body: &Body) -> Option<&'a ScaffoldKind> {
    let spoken_for = spoken_for(job);
    carried(ctx, &body.slots)
        .into_iter()
        .filter(|(kind, held)| spare(job, kind, *held) > 0 || !spoken_for.contains(&key(kind)))
        .max_by(|(a, an), (b, bn)| {
            (spare(job, a, *an).min(SCAFFOLD_STOCK))
                .cmp(&spare(job, b, *bn).min(SCAFFOLD_STOCK))
                .then(b.hardness.total_cmp(&a.hardness))
        })
        .map(|(kind, _)| kind)
}

/// The kind worth fetching from `stock` and how many, when the hands run
/// low: the chests' most spare, softest between equals.
pub fn to_fetch(
    ctx: &Ctx,
    job: &Job,
    slots: &[Option<ItemStackData>],
    stock: &BTreeMap<ItemKey, u32>,
) -> Option<(ItemKey, u32)> {
    let have = in_hand(ctx, job, slots);
    // A pillar taller than the usual stock asks for its own height.
    let target = SCAFFOLD_STOCK.max(job.crew.scaffolding.want);
    if have >= target {
        return None;
    }
    ctx.content
        .scaffolding
        .iter()
        .filter_map(|kind| {
            let held = stock.get(&key(kind)).copied().unwrap_or(0);
            (held > 0).then(|| (kind, held, spare(job, kind, held)))
        })
        .max_by(|(a, _, a_spare), (b, _, b_spare)| {
            (a_spare.min(&SCAFFOLD_STOCK))
                .cmp(b_spare.min(&SCAFFOLD_STOCK))
                .then(b.hardness.total_cmp(&a.hardness))
        })
        .map(|(kind, held, spare)| {
            // Spare blocks first; the build's own only when nothing is spare.
            let from = if spare > 0 { spare } else { held };
            (key(kind), from.min(target - have))
        })
}

fn kind_of<'a>(ctx: &'a Ctx, stack: &ItemStackData) -> &'a ScaffoldKind {
    ctx.content
        .scaffolding
        .iter()
        .find(|k| k.item == stack.item)
        .expect("checked to be a scaffolding kind")
}

/// Whether `stack` is scaffolding the golem keeps on it between climbs. Past
/// twice its stock (a hillside's worth of dug earth) it is spoil like any.
pub fn keeps(ctx: &Ctx, job: &Job, slots: &[Option<ItemStackData>], stack: &ItemStackData) -> bool {
    stack.data.is_empty()
        && ctx.content.scaffolding.iter().any(|k| k.item == stack.item)
        && spare(job, kind_of(ctx, stack), u32::from(stack.count)) > 0
        && in_hand(ctx, job, slots) <= SCAFFOLD_STOCK.max(job.crew.scaffolding.want) * 2
}

/// Stand a scaffold block in `cell`: an ordinary paid placement of whatever
/// the golem picks to stand on.
pub fn lay(ctx: &mut Ctx, job: &mut Job, body: &Body, cell: [i32; 3]) -> hands::Lay {
    let Some(kind) = pick(ctx, job, body).cloned() else {
        return hands::Lay::Refused(ActionRefusal::MissingItems);
    };
    hands::lay(ctx, job, body, cell, kind.record(), true, Some(kind.item))
}

/// The record a look at a scaffold's place is judged with.
pub fn record(ctx: &Ctx, job: &Job, body: &Body) -> Option<BlockRecord> {
    pick(ctx, job, body)
        .or(ctx.content.scaffolding.first())
        .map(ScaffoldKind::record)
}

/// Whether recorded scaffold cell `cell` still holds a block scaffolding is
/// made of. `None` = unloaded.
pub fn stands(content: &crate::content::Content, cell: [i32; 3]) -> Option<bool> {
    get_block(cell).map(|block| content.scaffold_kind(block).is_some())
}

/// The tool kinds that take the golem's scaffolding back down: of what it
/// carries, and of `fetching`, the kind it is about to fetch.
pub fn tools(
    ctx: &Ctx,
    slots: &[Option<ItemStackData>],
    fetching: Option<&ItemKey>,
) -> Vec<String> {
    let held = carried(ctx, slots);
    ctx.content
        .scaffolding
        .iter()
        .filter(|kind| {
            held.iter().any(|(k, _)| k.block == kind.block) || fetching == Some(&key(kind))
        })
        .filter(|kind| kind.hardness > 0.0)
        .filter_map(|kind| kind.tool.clone())
        .collect()
}
