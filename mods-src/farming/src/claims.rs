//! The pack's click claims, written ONCE and run by both instances.
//!
//! Which handler is offered a click, in which order, and each handler's
//! GATE (the `*_gate` / `gate` functions in the handler modules, generic
//! over the SDK's [`WorldView`]) are the same code on the server and the
//! client. The server instance walks a chain against the authoritative
//! world and EXECUTES each gated plan in turn until one's mutation lands (a
//! refused consume or swap falls through to the next link, exactly as a
//! handler's `Continue` always did). The client instance walks the SAME
//! chain against the replica and answers the claim: a gate that passes is a
//! predicted Cancel — a jab, and no place ghost. Nothing here restates a
//! server rule for the client, so a gate cannot drift from its twin.
//!
//! The one thing only the server can know is whether a mutation's resource
//! check lands (a held-item swap into a full inventory). A plan whose
//! execution refuses there is the prediction's only miss, and it costs
//! feel, never the outcome.

use mod_sdk::*;
use weather_core::FieldParams;

use crate::content::{Content, CropDef};
use crate::crops::{self, Growth};
use crate::fertilize::{self, Target};
use crate::trough::{self, TroughUse};
use crate::{compost, tilling};

/// A gated `item_use_pre` plan.
#[derive(Copy, Clone)]
pub enum UsePlan {
    /// The hoe tills the target, clearing `cover` above it.
    Till { cover: BlockId },
    /// Fertilizer acts on `at` (the target, or the soil it proxies to).
    Fertilize { at: [i32; 3], action: Target },
    /// A compostable unit advances the barrel from `stage`.
    Compost { stage: u8 },
    /// A bucket or wheat bundle uses the trough.
    Trough(TroughUse),
}

impl UsePlan {
    /// Execute the plan on the server: `Cancel` when its mutation landed.
    pub fn execute(
        self,
        content: &Content,
        sky: Option<&FieldParams>,
        item: ItemId,
        pos: [i32; 3],
    ) -> Outcome {
        match self {
            UsePlan::Till { cover } => tilling::till(content, sky, pos, cover),
            UsePlan::Fertilize { at, action } => fertilize::apply(at, item, action),
            UsePlan::Compost { stage } => compost::fill(content, item, pos, stage),
            UsePlan::Trough(using) => trough::apply(content, pos, using),
        }
    }
}

/// The `item_use_pre` chain: the hoe first (it consumes eligible clicks),
/// then the fertilizer targets, then the compostable barrel fill, then the
/// water trough — each passing quietly when the held item is not its
/// business. `act` receives each gated plan in order; the first `Cancel`
/// claims the click. A target the instance cannot read claims nothing:
/// nothing here may claim a click it cannot inspect.
pub fn item_use(
    content: &Content,
    world: &impl WorldView,
    item: ItemId,
    target: Option<[i32; 3]>,
    mut act: impl FnMut([i32; 3], UsePlan) -> Outcome,
) -> Outcome {
    let Some(pos) = target else {
        return Outcome::Continue;
    };
    let Some(block) = world.block(pos) else {
        return Outcome::Continue;
    };
    let links: [&dyn Fn() -> Option<UsePlan>; 4] = [
        &|| tilling::gate(content, world, item, pos, block).map(|cover| UsePlan::Till { cover }),
        &|| {
            fertilize::gate(content, world, item, pos, block)
                .map(|(at, action)| UsePlan::Fertilize { at, action })
        },
        &|| compost::fill_gate(content, item, block).map(|stage| UsePlan::Compost { stage }),
        &|| trough::use_gate(content, item, block).map(UsePlan::Trough),
    ];
    for link in links {
        if let Some(plan) = link() {
            if act(pos, plan) == Outcome::Cancel {
                return Outcome::Cancel;
            }
        }
    }
    Outcome::Continue
}

/// A gated `interact_attempt` plan.
#[derive(Copy, Clone)]
pub enum InteractPlan<'c> {
    /// Harvest the mature crop.
    Harvest(&'c CropDef),
    /// Collect fertilizer from the full barrel.
    CompostCollect,
    /// Take the feed back out of the wheat trough.
    TroughTakeOut,
}

impl InteractPlan<'_> {
    /// Execute the plan on the server: `Cancel` when it landed.
    pub fn execute(self, content: &Content, growth: &mut Growth, pos: [i32; 3]) -> Outcome {
        match self {
            InteractPlan::Harvest(def) => crops::harvest(content, growth, pos, def),
            InteractPlan::CompostCollect => compost::collect(content, pos),
            InteractPlan::TroughTakeOut => trough::take_out(content, pos),
        }
    }
}

/// The `interact_attempt` chain on a block target: crops first, then the
/// compost collect, then the trough's sneak take-out. The attempt is the
/// bare gesture: each gate reads the world and the acting player's snapshot
/// itself. A frozen cell (`None`) passes.
pub fn interact<'c>(
    content: &'c Content,
    world: &impl WorldView,
    pos: [i32; 3],
    mut act: impl FnMut(InteractPlan<'c>) -> Outcome,
) -> Outcome {
    let Some(block) = world.block(pos) else {
        return Outcome::Continue;
    };
    let actor = player_state();
    let links = [
        crops::harvest_gate(content, block, &actor).map(InteractPlan::Harvest),
        compost::collect_gate(content, block).then_some(InteractPlan::CompostCollect),
        trough::take_out_gate(content, block, &actor).then_some(InteractPlan::TroughTakeOut),
    ];
    for plan in links.into_iter().flatten() {
        if act(plan) == Outcome::Cancel {
            return Outcome::Cancel;
        }
    }
    Outcome::Continue
}
