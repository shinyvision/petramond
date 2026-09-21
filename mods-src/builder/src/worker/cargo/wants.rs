//! What the work ahead wants carried: the next batch of blocks, a tool per
//! kind of digging, and what is kept in hand.

use std::collections::{BTreeMap, BTreeSet};

use mod_sdk::*;

use super::totals;
use crate::content::BLUEPRINT;
use crate::jobs::Job;
use crate::project::Project;
use crate::survey::{key_of, ItemKey, Known};
use crate::worker::tuning::hands::{DIG_ROOM, DIG_ROOM_MOST, SPOIL_PER_SLOT};
use crate::worker::tuning::window::LOOKAHEAD;
use crate::worker::{scaffold, Body, Ctx};

/// Slots kept free for what digging collects: a cellar dug out of a bank
/// fills the hands many times over, and with room for two kinds of spoil the
/// golem was back at the chests every hundred blocks.
pub(super) fn dig_room(job: &Job) -> usize {
    let digs = job.survey.as_ref().map_or(0, |survey| {
        survey
            .known
            .iter()
            .skip(job.crew.pace.cursor)
            .filter(|k| {
                matches!(
                    k,
                    Known::Clear {
                        holds_items: false,
                        ..
                    }
                )
            })
            .count()
    });
    (DIG_ROOM + digs / SPOIL_PER_SLOT).min(DIG_ROOM_MOST)
}

pub(super) fn tool_kind(ctx: &mut Ctx, item: &str) -> Option<(String, f32)> {
    ctx.caches
        .item(item)
        .and_then(|i| i.tool.as_ref())
        .map(|t| (t.kind.clone(), t.speed))
}

/// The carried slot holding the fastest tool for `block`; `None` = hands.
pub fn tool_slot(ctx: &mut Ctx, slots: &[Option<ItemStackData>], block: BlockId) -> Option<u32> {
    let kind = ctx.caches.block(block)?.preferred_tool.clone()?;
    let mut best: Option<(u32, f32)> = None;
    for (i, stack) in slots.iter().enumerate() {
        let Some(stack) = stack else { continue };
        if let Some((k, speed)) = tool_kind(ctx, &stack.item) {
            if k == kind && best.is_none_or(|(_, s)| speed > s) {
                best = Some((i as u32, speed));
            }
        }
    }
    best.map(|(i, _)| i)
}

/// The tool kinds the golem's own digging wants besides the site's clearance:
/// built blocks taken back down to open a way in, and overgrowth cut back. A
/// block any hand breaks at once wants none.
fn dig_kinds(ctx: &mut Ctx, job: &Job) -> BTreeSet<String> {
    let mut cells: Vec<[i32; 3]> = job.crew.access.trims.iter().copied().collect();
    if let Some(survey) = job.survey.as_ref() {
        cells.extend(
            job.crew
                .access
                .reopen
                .values()
                .flatten()
                .filter(|o| matches!(survey.known[**o], Known::Satisfied))
                .map(|o| job.design.units[*o].pos),
        );
    }
    let mut kinds = BTreeSet::new();
    if cells.is_empty() {
        return kinds;
    }
    for block in get_blocks(cells).into_iter().flatten() {
        if let Some(info) = ctx.caches.block(block) {
            if info.hardness > 0.0 {
                kinds.extend(info.preferred_tool.clone());
            }
        }
    }
    kinds
}

/// A tool kind per kind of digging ahead that the golem does not carry, and
/// the tools it does. Asked by every plan, so it stays a walk over the survey
/// with nothing built per unit.
fn wanted_tools(
    ctx: &mut Ctx,
    job: &Job,
    slots: &[Option<ItemStackData>],
    scaffold: Option<&ItemKey>,
) -> (BTreeSet<String>, usize) {
    let mut kinds = dig_kinds(ctx, job);
    if let Some(survey) = job.survey.as_ref() {
        let mut asked: Option<BlockId> = None;
        for known in survey
            .known
            .iter()
            .skip(job.crew.pace.cursor)
            .take(LOOKAHEAD)
        {
            if let Known::Clear {
                block,
                holds_items: false,
                ..
            } = known
            {
                // A bank is one kind of earth a thousand times over.
                if asked.replace(*block) == Some(*block) {
                    continue;
                }
                if let Some(kind) = ctx
                    .caches
                    .block(*block)
                    .and_then(|b| b.preferred_tool.as_ref())
                {
                    if !kinds.contains(kind) {
                        kinds.insert(kind.clone());
                    }
                }
            }
        }
    }
    // Scaffolding rides along with the tool that takes it back down.
    kinds.extend(scaffold::tools(ctx, slots, scaffold));
    let mut tools = 0;
    for stack in slots.iter().flatten() {
        if let Some((kind, _)) = tool_kind(ctx, &stack.item) {
            kinds.remove(&kind);
            tools += 1;
        }
    }
    (kinds, tools)
}

/// What the work ahead wants carried beyond what is carried: items for the
/// next batch of placements, and a tool kind per kind of digging.
pub(super) fn wanted(
    ctx: &mut Ctx,
    job: &Job,
    slots: &[Option<ItemStackData>],
    have: Option<&BTreeMap<ItemKey, u32>>,
) -> (BTreeMap<ItemKey, u32>, BTreeSet<String>) {
    let mut need: BTreeMap<ItemKey, u32> = BTreeMap::new();
    // Scaffolding rides along: a slot of whatever the chests can best spare.
    let scaffold = have.and_then(|have| scaffold::to_fetch(ctx, job, slots, have));
    let (kinds, tools) = wanted_tools(ctx, job, slots, scaffold.as_ref().map(|(k, _)| k));
    let Some(survey) = job.survey.as_ref() else {
        return (need, kinds);
    };
    let carried = totals(slots);
    // What the batch has to spend: the hands and the chests together, spent
    // down as work is taken into it.
    let mut left = carried.clone();
    if let Some(have) = have {
        for (key, n) in have {
            *left.entry(key.clone()).or_default() += n;
        }
    }
    // The batch fills what the tools, carried and to fetch, and digging leave.
    let room = slots
        .len()
        .saturating_sub(tools + kinds.len() + dig_room(job) + 1)
        .max(1);
    let mut seen = 0;
    for (i, known) in survey.known.iter().enumerate().skip(job.crew.pace.cursor) {
        if seen == LOOKAHEAD {
            break;
        }
        // Buried work is built the moment it is dug out, so its blocks ride
        // along now.
        let buried = match known {
            Known::Clear {
                holds_items: false, ..
            } => job.design.cost(job.design.units[i]),
            _ => &[],
        };
        let missing: Option<&[ItemStackData]> = match known {
            Known::Place(missing) if !job.crew.built.contains(&i) => Some(missing),
            _ if !buried.is_empty() => Some(buried),
            _ => None,
        };
        match (missing, known) {
            (Some(missing), _) => {
                seen += 1;
                // What neither the hands nor the chests hold cannot be built
                // now: the batch passes over it. Counted DOWN as the batch
                // takes it, never per block against the whole pile, or five
                // planks in hand answer for eighty-six blocks.
                if have.is_some() {
                    let short = missing.iter().any(|stack| {
                        left.get(&key_of(stack)).copied().unwrap_or(0) < u32::from(stack.count)
                    });
                    if short {
                        continue;
                    }
                    for stack in missing {
                        let key = key_of(stack);
                        let n = left.entry(key).or_default();
                        *n = n.saturating_sub(u32::from(stack.count));
                    }
                }
                let mut next = need.clone();
                for stack in missing {
                    *next.entry(key_of(stack)).or_default() += u32::from(stack.count);
                }
                let slots_used: usize = next
                    .iter()
                    .map(|((item, _), n)| {
                        (*n as usize).div_ceil(ctx.caches.max_stack(item) as usize)
                    })
                    .sum();
                if slots_used > room && !need.is_empty() {
                    break;
                }
                need = next;
            }
            (
                None,
                Known::Clear {
                    holds_items: false, ..
                },
            ) => seen += 1,
            _ => {}
        }
    }
    // Top the load up from the bill the job still owes, while slots are left:
    // the survey only knows what it sees, so a golem cutting into a bank would
    // otherwise go back for each newly uncovered block.
    if let (Some(summary), Some(have)) = (job.summary(), have) {
        let slots_used = |need: &BTreeMap<ItemKey, u32>, ctx: &mut Ctx| -> usize {
            need.iter()
                .map(|((item, _), n)| (*n as usize).div_ceil(ctx.caches.max_stack(item) as usize))
                .sum()
        };
        let mut bill: Vec<(ItemKey, u32)> = summary.bill.clone().into_iter().collect();
        bill.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        for (key, owed) in bill {
            let held =
                need.get(&key).copied().unwrap_or(0) + carried.get(&key).copied().unwrap_or(0);
            let spare = have.get(&key).copied().unwrap_or(0);
            let want = owed.saturating_sub(held).min(spare);
            if want == 0 {
                continue;
            }
            let mut next = need.clone();
            *next.entry(key).or_default() += want;
            if slots_used(&next, ctx) > room {
                continue;
            }
            need = next;
        }
    }
    for (key, count) in carried {
        if let Some(n) = need.get_mut(&key) {
            *n = n.saturating_sub(count);
        }
    }
    need.retain(|_, n| *n > 0);
    if let Some((key, n)) = scaffold {
        *need.entry(key).or_default() += n;
    }
    (need, kinds)
}

/// The tool kinds the digging ahead wants, the golem lacks, and the chests
/// hold, fetched before the digging starts rather than on the next block run.
pub fn tools_waiting(ctx: &mut Ctx, job: &Job, body: &Body, project: &Project) -> BTreeSet<String> {
    let stock = ctx.supplies.stock(project.table);
    let scaffold = stock
        .read
        .then(|| scaffold::to_fetch(ctx, job, &body.slots, &stock.totals))
        .flatten();
    let (kinds, _) = wanted_tools(ctx, job, &body.slots, scaffold.as_ref().map(|(k, _)| k));
    if kinds.is_empty() {
        return kinds;
    }
    let mut waiting = BTreeSet::new();
    for stack in stock.slots.iter().flatten().flatten() {
        if let Some((kind, _)) = tool_kind(ctx, &stack.item) {
            if kinds.contains(&kind) {
                waiting.insert(kind);
            }
        }
    }
    waiting
}

/// Whether a carried stack is still wanted: needed by the work ahead, or a
/// tool, kept until the job is done.
pub(super) fn keeps(
    ctx: &mut Ctx,
    job: &Job,
    slots: &[Option<ItemStackData>],
    stack: &ItemStackData,
) -> bool {
    let Some(survey) = job.survey.as_ref() else {
        return true;
    };
    // Its plans: never spoil, never handed in.
    if stack.item == BLUEPRINT {
        return true;
    }
    if tool_kind(ctx, &stack.item).is_some() {
        return true;
    }
    if scaffold::keeps(ctx, job, slots, stack) {
        return true;
    }
    let key = key_of(stack);
    survey
        .known
        .iter()
        .enumerate()
        .skip(job.crew.pace.cursor)
        .take(LOOKAHEAD * 4)
        .any(|(i, k)| wants(job, i, k).iter().any(|m| key_of(m) == key))
}

/// What unit `i` still wants laid: what is missing of it, or, while its cell
/// is still to be dug out, what it costs.
fn wants<'a>(job: &'a Job, i: usize, known: &'a Known) -> &'a [ItemStackData] {
    match known {
        Known::Place(missing) => missing,
        Known::Clear {
            holds_items: false, ..
        } => job.design.cost(job.design.units[i]),
        _ => &[],
    }
}

/// Whether digging that collects what it breaks is still to come.
pub(super) fn digs_ahead(job: &Job) -> bool {
    !job.crew.access.reopen.is_empty()
        || job.survey.as_ref().is_some_and(|survey| {
            survey
                .known
                .iter()
                .skip(job.crew.pace.cursor)
                .take(LOOKAHEAD)
                .any(|k| {
                    matches!(
                        k,
                        Known::Clear {
                            holds_items: false,
                            ..
                        }
                    )
                })
        })
}

/// How far ahead the work first needs `stack`: the first open unit wanting
/// it, or never.
pub(super) fn needed_at(job: &Job, stack: &ItemStackData) -> usize {
    let key = key_of(stack);
    job.survey
        .as_ref()
        .and_then(|survey| {
            survey
                .known
                .iter()
                .enumerate()
                .skip(job.crew.pace.cursor)
                .position(|(i, k)| wants(job, i, k).iter().any(|m| key_of(m) == key))
        })
        .unwrap_or(usize::MAX)
}
