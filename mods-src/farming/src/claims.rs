use mod_sdk::*;
use weather_core::FieldParams;

use crate::content::{Content, CropDef};
use crate::crops;
use crate::fertilize::{self, Target};
use crate::trough::{self, TroughUse};
use crate::{compost, tilling};

#[derive(Copy, Clone)]
pub enum UsePlan {
    Till { cover: BlockId },
    Fertilize { at: [i32; 3], action: Target },
    Compost { stage: u8 },
    Trough(TroughUse),
}

impl UsePlan {
    pub fn execute(
        self,
        content: &Content,
        sky: Option<&FieldParams>,
        player: PlayerId,
        item: ItemId,
        pos: [i32; 3],
    ) -> Outcome {
        match self {
            UsePlan::Till { cover } => tilling::till(content, sky, pos, cover),
            UsePlan::Fertilize { at, action } => fertilize::apply(player, at, item, action),
            UsePlan::Compost { stage } => compost::fill(content, player, item, pos, stage),
            UsePlan::Trough(using) => trough::apply(content, player, pos, using),
        }
    }
}

pub fn item_use(
    content: &Content,
    player: PlayerId,
    item: ItemId,
    target: Option<[i32; 3]>,
    mut act: impl FnMut([i32; 3], UsePlan) -> Outcome,
) -> Outcome {
    let Some(pos) = target else {
        return Outcome::Continue;
    };
    let Some(block) = get_block(pos) else {
        return Outcome::Continue;
    };
    let links: [&dyn Fn() -> Option<UsePlan>; 4] = [
        &|| tilling::gate(content, item, pos, block).map(|cover| UsePlan::Till { cover }),
        &|| {
            fertilize::gate(content, item, pos, block)
                .map(|(at, action)| UsePlan::Fertilize { at, action })
        },
        &|| compost::fill_gate(content, item, block).map(|stage| UsePlan::Compost { stage }),
        &|| trough::use_gate(content, player, item, block).map(UsePlan::Trough),
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

#[derive(Copy, Clone)]
pub enum InteractPlan<'c> {
    Harvest(&'c CropDef),
    CompostCollect,
    TroughTakeOut,
}

impl InteractPlan<'_> {
    pub fn execute(self, content: &Content, player: PlayerId, pos: [i32; 3]) -> Outcome {
        match self {
            InteractPlan::Harvest(def) => crops::harvest(content, pos, def),
            InteractPlan::CompostCollect => compost::collect(content, pos),
            InteractPlan::TroughTakeOut => trough::take_out(content, player, pos),
        }
    }
}

pub fn interact<'c>(
    content: &'c Content,
    player: PlayerId,
    pos: [i32; 3],
    mut act: impl FnMut(InteractPlan<'c>) -> Outcome,
) -> Outcome {
    let Some(block) = get_block(pos) else {
        return Outcome::Continue;
    };
    let Some(actor) = player_state(player) else {
        return Outcome::Continue;
    };
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
