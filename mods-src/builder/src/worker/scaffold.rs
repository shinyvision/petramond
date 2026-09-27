use std::collections::BTreeMap;

use crate::host::prelude::*;

use super::tuning::hands::SCAFFOLD_STOCK;
use super::{cargo, hands, Body, Ctx};
use crate::content::ScaffoldKind;
use crate::survey::ItemKey;
use crate::worker::Job;

fn key(kind: &ScaffoldKind) -> ItemKey {
    (kind.item.clone(), Vec::new())
}

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

fn spare(job: &Job, kind: &ScaffoldKind, held: u32) -> u32 {
    let owed = job
        .summary()
        .and_then(|s| s.bill.get(&key(kind)).copied())
        .unwrap_or(0);
    held.saturating_sub(owed)
}

pub fn in_hand(ctx: &Ctx, job: &Job, slots: &[Option<ItemStackData>]) -> u32 {
    carried(ctx, slots)
        .iter()
        .map(|(kind, held)| spare(job, kind, *held))
        .sum()
}

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

pub fn to_fetch(
    ctx: &Ctx,
    job: &Job,
    slots: &[Option<ItemStackData>],
    stock: &BTreeMap<ItemKey, u32>,
) -> Option<(ItemKey, u32)> {
    let have = in_hand(ctx, job, slots);
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

pub fn keeps(ctx: &Ctx, job: &Job, slots: &[Option<ItemStackData>], stack: &ItemStackData) -> bool {
    stack.data.is_empty()
        && ctx.content.scaffolding.iter().any(|k| k.item == stack.item)
        && spare(job, kind_of(ctx, stack), u32::from(stack.count)) > 0
        && in_hand(ctx, job, slots) <= SCAFFOLD_STOCK.max(job.crew.scaffolding.want) * 2
}

pub fn lay(ctx: &mut Ctx, job: &mut Job, body: &Body, cell: [i32; 3]) -> hands::Lay {
    let Some(kind) = pick(ctx, job, body).cloned() else {
        return hands::Lay::Refused(ActionRefusal::MissingItems);
    };
    hands::lay(ctx, job, body, cell, kind.record(), true, Some(kind.item))
}

pub fn record(ctx: &Ctx, job: &Job, body: &Body) -> Option<BlockRecord> {
    pick(ctx, job, body)
        .or(ctx.content.scaffolding.first())
        .map(ScaffoldKind::record)
}

pub fn stands(content: &crate::content::Content, cell: [i32; 3]) -> Option<bool> {
    get_block(cell).map(|block| content.scaffold_kind(block).is_some())
}

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
