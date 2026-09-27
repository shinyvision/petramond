//! Cultivated crops: planting checks, four-stage growth with a dry pause that doesn't reset,
//! right-click harvest, soil invalidation.
//!
//! Growth model. Stage is the block id, no separate age store. Planting/advancing schedules the
//! next attempt after a delay jittered by position and stage (see [`STAGE_DELAY_MIN`]).
//! Hydrated soil advances a stage when due. Dry soil retries on a short interval without
//! resetting the delay, so it picks up right where it left off once watered. Crops never regress
//! or die.
//!
//! Re-arming. Scheduled work doesn't survive an unload, and there's no offline catch-up. The armed
//! set is just an optimization: random ticks re-arm any growable crop that's missing or overdue, so
//! a reload never freezes a crop. We check the ledger's due tick before acting, to ignore a stale
//! fire racing a re-arm.

use std::collections::HashMap;

use mod_sdk::*;
use weather_core::FieldParams;

use crate::content::{Content, CropDef};
use crate::farmland::{self, Hydration};
use crate::keys;
use crate::rest::Rests;

const STAGE_DELAY_MIN: i32 = 2400;
const STAGE_DELAY_MAX: i32 = 3600;

const FERTILE_DELAY_NUM: u64 = 2;
const FERTILE_DELAY_DEN: u64 = 3;

const FERTILE_BONUS_IN: u64 = 10;

const MIN_GROW_LIGHT: u8 = 36;
const DRY_RETRY: u64 = 100;
const REARM_GRACE: u64 = 200;
const JITTER_SALT: u64 = 0x00FA_3417_6A0B_11ED;

#[derive(Default)]
pub struct Growth {
    pending: HashMap<[i32; 3], u64>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Planting {
    NotACrop,
    Refused,
    Allowed,
    SoilUnknown,
}

pub fn planting(
    content: &Content,
    world: &impl WorldView,
    pos: [i32; 3],
    block: BlockId,
) -> Planting {
    let Some((_, stage)) = content.crop_stage(block) else {
        return Planting::NotACrop;
    };
    if stage != 0 {
        return Planting::Refused;
    }
    match world.block([pos[0], pos[1] - 1, pos[2]]) {
        Some(b) if content.is_farmland(b) => Planting::Allowed,
        Some(_) => Planting::Refused,
        None => Planting::SoilUnknown,
    }
}

pub fn on_place_pre(content: &Content, pos: [i32; 3], block: BlockId) -> Outcome {
    match planting(content, &SideWorld::Server, pos, block) {
        Planting::NotACrop => Outcome::Continue,
        Planting::Allowed if !too_dark(pos) => Outcome::Continue,
        Planting::Allowed | Planting::Refused | Planting::SoilUnknown => Outcome::Cancel,
    }
}

pub fn predict_place_pre(content: &Content, pos: [i32; 3], block: BlockId) -> Outcome {
    match planting(content, &SideWorld::Replica, pos, block) {
        Planting::Refused => Outcome::Cancel,
        Planting::NotACrop | Planting::Allowed | Planting::SoilUnknown => Outcome::Continue,
    }
}

pub fn harvest_gate<'c>(
    content: &'c Content,
    block: BlockId,
    actor: &PlayerSnapshot,
) -> Option<&'c CropDef> {
    match content.crop_stage(block) {
        Some((def, 3)) if !(actor.sneak && held_item_places_block(actor.held)) => Some(def),
        _ => None,
    }
}

fn too_dark(pos: [i32; 3]) -> bool {
    light_at(pos).is_some_and(|l| l.combined < MIN_GROW_LIGHT)
}

pub fn on_placed(content: &Content, growth: &mut Growth, pos: [i32; 3], block: BlockId) {
    if let Some((_, stage @ 0..=2)) = content.crop_stage(block) {
        arm(growth, pos, stage, soil_is_fertile(content, pos));
    }
}

pub fn harvest(content: &Content, growth: &mut Growth, pos: [i32; 3], def: &CropDef) -> Outcome {
    let center = [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 0.4,
        pos[2] as f64 + 0.5,
    ];
    let fertile = soil_is_fertile(content, pos);
    spawn_all(mature_yield(def, fertile, Taking::Retained), center);
    emit_sound(keys::HARVEST_SOUND, Some(center));
    if let Some(emitter) = &def.harvest_emitter {
        emitter_burst(emitter, center, 1.0);
    }
    set_block(pos, def.stages[0]);
    arm(growth, pos, 0, fertile);
    Outcome::Cancel
}

#[derive(Copy, Clone, PartialEq)]
enum Taking {
    Retained,
    Removed,
}

fn mature_yield(def: &CropDef, fertile: bool, taking: Taking) -> Vec<(&str, u8)> {
    let roll =
        |key: &str, (lo, hi): (u64, u64)| -> u8 { (lo + rng_u64(key) % (hi - lo + 1)) as u8 };
    let mut out = vec![(
        def.produce.as_str(),
        roll(&def.harvest_key, def.yield_range)
            + (fertile && rng_u64(&def.fertile_key).is_multiple_of(FERTILE_BONUS_IN)) as u8,
    )];
    if let Some(extra) = &def.extra_drop {
        let yields =
            extra.chance_percent >= 100 || rng_u64(&extra.chance_key) % 100 < extra.chance_percent;
        if yields {
            out.push((extra.item.as_str(), roll(&extra.count_key, extra.count)));
        }
    }
    if taking == Taking::Removed {
        let stock: u32 = out
            .iter()
            .filter(|(item, _)| *item == def.planting_stock.as_str())
            .map(|(_, n)| *n as u32)
            .sum();
        if stock == 0 {
            out.push((def.planting_stock.as_str(), 1));
        }
    }
    out
}

fn spawn_all(items: Vec<(&str, u8)>, center: [f64; 3]) {
    for (item, count) in items {
        if count > 0 {
            spawn_item(item, count, center);
        }
    }
}

pub fn on_block_broken(content: &Content, pos: [i32; 3], block: BlockId, harvested: bool) {
    let Some((def, stage)) = content.crop_stage(block) else {
        return;
    };
    if !harvested {
        return;
    }
    let center = [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 0.3,
        pos[2] as f64 + 0.5,
    ];
    if stage < 3 {
        spawn_item(&def.planting_stock, 1, center);
        return;
    }
    spawn_all(
        mature_yield(def, soil_is_fertile(content, pos), Taking::Removed),
        center,
    );
}

pub fn on_hook(
    content: &Content,
    growth: &mut Growth,
    rests: &mut Rests,
    sky: Option<&FieldParams>,
    kind: BlockHookKind,
    pos: [i32; 3],
) {
    match kind {
        BlockHookKind::ScheduledTick => attempt(content, growth, sky, pos),
        BlockHookKind::RandomTick => rearm_if_lost(content, growth, rests, pos),
        BlockHookKind::NeighborUpdate => support_check(content, growth, pos),
    }
}

fn attempt(content: &Content, growth: &mut Growth, sky: Option<&FieldParams>, pos: [i32; 3]) {
    match growth.pending.get(&pos) {
        Some(&due) if current_tick() >= due => {}
        _ => return,
    }
    growth.pending.remove(&pos);
    let Some(block) = get_block(pos) else {
        retry(growth, pos);
        return;
    };
    let Some((def, stage @ 0..=2)) = content.crop_stage(block) else {
        return;
    };
    let below = [pos[0], pos[1] - 1, pos[2]];
    let fertile = match get_block(below) {
        Some(b) if content.is_farmland(b) => content.is_fertile(b),
        Some(_) => return,
        None => {
            retry(growth, pos);
            return;
        }
    };
    if too_dark(pos) {
        retry(growth, pos);
        return;
    }
    match farmland::probe(content, sky, below) {
        Hydration::Hydrated => {
            let next = stage + 1;
            set_block(pos, def.stages[next as usize]);
            if next <= 2 {
                arm(growth, pos, next, fertile);
            }
        }
        Hydration::Dry | Hydration::Unknown => retry(growth, pos),
    }
}

fn rearm_if_lost(content: &Content, growth: &mut Growth, rests: &mut Rests, pos: [i32; 3]) {
    let Some(block) = get_block(pos) else {
        return;
    };
    let Some((def, stage)) = content.crop_stage(block) else {
        return;
    };
    if too_dark(pos) {
        pop_planting_stock(growth, def, pos);
        return;
    }
    crate::attract::on_random_tick(content, rests, pos, block);
    if let Some(&due) = growth.pending.get(&pos) {
        if current_tick() <= due + REARM_GRACE {
            return;
        }
    }
    if stage <= 2 {
        arm(growth, pos, stage, soil_is_fertile(content, pos));
    }
}

fn support_check(content: &Content, growth: &mut Growth, pos: [i32; 3]) {
    let Some(block) = get_block(pos) else {
        return;
    };
    let Some((def, _)) = content.crop_stage(block) else {
        return;
    };
    let Some(below) = get_block([pos[0], pos[1] - 1, pos[2]]) else {
        return;
    };
    if content.is_farmland(below) {
        return;
    }
    pop_planting_stock(growth, def, pos);
}

fn pop_planting_stock(growth: &mut Growth, def: &CropDef, pos: [i32; 3]) {
    set_block(pos, BlockId::AIR);
    spawn_item(
        &def.planting_stock,
        1,
        [
            pos[0] as f64 + 0.5,
            pos[1] as f64 + 0.3,
            pos[2] as f64 + 0.5,
        ],
    );
    growth.pending.remove(&pos);
}

fn arm(growth: &mut Growth, pos: [i32; 3], stage: u8, fertile: bool) {
    let mut delay = stage_delay(pos, stage);
    if fertile {
        delay = delay * FERTILE_DELAY_NUM / FERTILE_DELAY_DEN;
    }
    schedule_tick(pos, delay);
    growth.pending.insert(pos, current_tick() + delay);
}

fn soil_is_fertile(content: &Content, pos: [i32; 3]) -> bool {
    get_block([pos[0], pos[1] - 1, pos[2]]).is_some_and(|b| content.is_fertile(b))
}

fn retry(growth: &mut Growth, pos: [i32; 3]) {
    schedule_tick(pos, DRY_RETRY);
    growth.pending.insert(pos, current_tick() + DRY_RETRY);
}

fn stage_delay(pos: [i32; 3], stage: u8) -> u64 {
    let mut rng = GenRng::positional(
        0,
        JITTER_SALT ^ (stage as u64) << 56,
        pos[0],
        pos[1],
        pos[2],
    );
    rng.next_i32(STAGE_DELAY_MIN, STAGE_DELAY_MAX) as u64
}
