//! Cultivated crops: planting validation, scheduled four-stage growth with a
//! non-destructive dry pause, right-click harvesting, and supporting-soil
//! invalidation.
//!
//! GROWTH MODEL. Stage identity IS the block id (persists through the normal
//! name-addressed palette; no per-crop age store). A planted or advanced crop
//! schedules its next attempt after a deterministic position/stage-jittered
//! delay (see [`STAGE_DELAY_MIN`]). At a due attempt, hydrated soil advances
//! one stage; dry soil retries on a short interval WITHOUT restarting the
//! stage delay — the crop retains its readiness and resumes promptly when
//! water returns. Crops never lose a stage and never die.
//!
//! RE-ARMING. Scheduled work dies with an unload (there is deliberately no
//! offline catch-up), so the in-memory armed set is only an optimization
//! ledger: RANDOM ticks (which every mod block receives while in active
//! range) re-arm any growable crop whose entry is missing or overdue. A
//! reload therefore never freezes a crop — the next random tick re-arms it.
//! Stale scheduled fires (a duplicate schedule racing a re-arm) are ignored
//! by checking the ledger's due tick before acting.

use std::collections::HashMap;

use mod_sdk::*;

use crate::content::{Content, Crop};
use crate::farmland::{self, Hydration};

/// Stage delay: 120–180 s at 20 TPS, jittered per (position, stage) so a
/// field planted in one sweep ripens staggered, not as one synchronized wave.
const STAGE_DELAY_MIN: i32 = 2400;
const STAGE_DELAY_MAX: i32 = 3600;

/// Minimum combined light (0..=63) for planting and growth: classic light
/// level 9 (levels sit ≈4.2 apart on the 6-bit scale — level 8 reads ≈34,
/// level 9 ≈38). "Light level 8 or lower" refuses planting, pauses due
/// growth, and a random tick BREAKS a crop left in the dark.
const MIN_GROW_LIGHT: u8 = 36;
/// Dry / unreadable-soil retry: 5 s. Keeps a ready crop responsive to
/// restored irrigation without hot-looping.
const DRY_RETRY: u64 = 100;
/// How long past its due tick an armed attempt is still considered "in
/// flight" (sim-guard retries can delay a scheduled fire) before a random
/// tick concludes the schedule was lost and re-arms.
const REARM_GRACE: u64 = 200;
/// Positional-jitter salt (the growth analog of a worldgen feature salt).
const JITTER_SALT: u64 = 0x00FA_3417_6A0B_11ED;

/// The armed-attempt ledger: crop cell → due tick.
#[derive(Default)]
pub struct Growth {
    pending: HashMap<[i32; 3], u64>,
}

/// Placement gate (`block_place_pre`): a stage-0 crop may only be placed on
/// farmland — dry or wet. Anything else (including a half-streamed cell that
/// reads `None`) refuses WITHOUT consuming the seed; for the carrot the
/// refusal is what lets the contextual placeable-food rule fall back to
/// eating. Non-zero stages have no placing item, but refuse them defensively.
pub fn on_place_pre(content: &Content, pos: [i32; 3], block: BlockId) -> Outcome {
    let Some((_, stage)) = content.crop_stage(block) else {
        return Outcome::Continue;
    };
    if stage != 0 {
        return Outcome::Cancel;
    }
    // Planting in darkness quietly does nothing (a cancelled placement never
    // consumes the seed).
    if too_dark(pos) {
        return Outcome::Cancel;
    }
    let below = get_block([pos[0], pos[1] - 1, pos[2]]);
    match below {
        Some(b) if content.is_farmland(b) => Outcome::Continue,
        _ => Outcome::Cancel,
    }
}

/// Whether the crop cell is too dark to live (see [`MIN_GROW_LIGHT`]). Raw
/// light, deliberately not day/night-scaled — an open-sky field keeps its
/// skylight at night; darkness means burial or an unlit cave.
fn too_dark(pos: [i32; 3]) -> bool {
    let (combined, _, _) = light_at(pos);
    combined < MIN_GROW_LIGHT
}

/// A freshly planted crop schedules its first stage attempt.
pub fn on_placed(content: &Content, growth: &mut Growth, pos: [i32; 3], block: BlockId) {
    if let Some((_, stage @ 0..=2)) = content.crop_stage(block) {
        arm(growth, pos, stage);
    }
}

/// Right-click interaction. A MATURE cultivated crop harvests with any held
/// item or empty hand: produce pops as nearby item entities, the crop resets
/// to its stage-0 block in the same tick (the retained plant is one replanted
/// seed/root), and the next growth attempt is armed. An IMMATURE cultivated
/// crop consumes the click without acting, so aiming at it can never plant
/// another crop or begin eating the held carrot. Wild crops are not ours to
/// handle here — they never right-click harvest.
pub fn on_interact(
    content: &Content,
    growth: &mut Growth,
    pos: [i32; 3],
    block: BlockId,
) -> Outcome {
    let Some((crop, stage)) = content.crop_stage(block) else {
        return Outcome::Continue;
    };
    if stage < 3 {
        return Outcome::Cancel;
    }
    let center = [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 0.4,
        pos[2] as f32 + 0.5,
    ];
    // Yield ranges are balance data; the invariant is that the plant itself
    // is retained (reset, not removed).
    let roll = |key: &str, lo: u64, hi: u64| -> u8 { (lo + rng_u64(key) % (hi - lo + 1)) as u8 };
    match crop {
        Crop::Wheat => {
            spawn_item("farming:wheat", roll("harvest_wheat", 1, 2), center);
            let seeds = roll("harvest_wheat_seeds", 0, 2);
            if seeds > 0 {
                spawn_item("farming:wheat_seeds", seeds, center);
            }
            emit_sound("farming:harvest", Some(center));
            emitter_burst("farming:wheat_harvest", center, 1.0);
        }
        Crop::Carrots => {
            spawn_item("farming:carrot", roll("harvest_carrot", 2, 3), center);
            emit_sound("farming:harvest", Some(center));
            emitter_burst("farming:carrot_harvest", center, 1.0);
        }
    }
    set_block(pos, content.stage_block(crop, 0));
    arm(growth, pos, 0);
    Outcome::Cancel
}

/// The crop block hooks.
pub fn on_hook(content: &Content, growth: &mut Growth, kind: BlockHookKind, pos: [i32; 3]) {
    match kind {
        BlockHookKind::ScheduledTick => attempt(content, growth, pos),
        BlockHookKind::RandomTick => rearm_if_lost(content, growth, pos),
        BlockHookKind::NeighborUpdate => support_check(content, growth, pos),
    }
}

/// One due growth attempt.
fn attempt(content: &Content, growth: &mut Growth, pos: [i32; 3]) {
    // Only act on the attempt we armed: a stale duplicate schedule (or a
    // foreign scheduled tick on this cell) must not double-advance a stage.
    match growth.pending.get(&pos) {
        Some(&due) if current_tick() >= due => {}
        _ => return,
    }
    growth.pending.remove(&pos);
    let Some(block) = get_block(pos) else {
        retry(growth, pos);
        return;
    };
    let Some((crop, stage @ 0..=2)) = content.crop_stage(block) else {
        return; // broken, replaced, or already mature — nothing owed
    };
    let below = [pos[0], pos[1] - 1, pos[2]];
    match get_block(below) {
        Some(b) if content.is_farmland(b) => {}
        Some(_) => return, // support gone; the neighbor hook owns the pop
        None => {
            retry(growth, pos);
            return;
        }
    }
    // Growth needs light: a due attempt in the dark pauses like dryness (the
    // next RANDOM tick is what breaks a dark crop — see `rearm_if_lost`).
    if too_dark(pos) {
        retry(growth, pos);
        return;
    }
    match farmland::probe(content, below) {
        Hydration::Hydrated => {
            let next = stage + 1;
            set_block(pos, content.stage_block(crop, next));
            if next <= 2 {
                arm(growth, pos, next);
            }
        }
        // The dry pause: readiness is retained (short retry), the full stage
        // delay never restarts, and the crop never regresses.
        Hydration::Dry | Hydration::Unknown => retry(growth, pos),
    }
}

/// Random ticks: first the darkness check — a crop random-ticked in light
/// level 8 or lower BREAKS (its planting stock pops, like losing its soil).
/// Otherwise they are the re-arm heartbeat: an unarmed or long-overdue
/// growable crop (its scheduled attempt died with an unload) schedules a
/// fresh attempt. Armed-and-not-yet-due crops are left alone.
fn rearm_if_lost(content: &Content, growth: &mut Growth, pos: [i32; 3]) {
    let Some(block) = get_block(pos) else {
        return;
    };
    let Some((crop, stage)) = content.crop_stage(block) else {
        return;
    };
    if too_dark(pos) {
        pop_planting_stock(growth, crop, pos);
        return;
    }
    if let Some(&due) = growth.pending.get(&pos) {
        if current_tick() <= due + REARM_GRACE {
            return;
        }
    }
    if stage <= 2 {
        arm(growth, pos, stage);
    }
}

/// Supporting-soil invalidation: a crop whose ground is no longer farmland
/// (broken OR replaced by something else) pops its planting stock rather
/// than vanishing or floating. `None` below = streaming; leave it alone.
fn support_check(content: &Content, growth: &mut Growth, pos: [i32; 3]) {
    let Some(block) = get_block(pos) else {
        return;
    };
    let Some((crop, _)) = content.crop_stage(block) else {
        return;
    };
    let Some(below) = get_block([pos[0], pos[1] - 1, pos[2]]) else {
        return;
    };
    if content.is_farmland(below) {
        return;
    }
    pop_planting_stock(growth, crop, pos);
}

/// A crop dying in place (soil invalidated, or left in the dark): the plant
/// goes and one planting stock pops — never lost, never a free harvest.
fn pop_planting_stock(growth: &mut Growth, crop: Crop, pos: [i32; 3]) {
    set_block(pos, BlockId::AIR);
    spawn_item(
        crop.planting_stock(),
        1,
        [
            pos[0] as f32 + 0.5,
            pos[1] as f32 + 0.3,
            pos[2] as f32 + 0.5,
        ],
    );
    growth.pending.remove(&pos);
}

/// Schedule the next stage attempt after the deterministic jittered delay.
fn arm(growth: &mut Growth, pos: [i32; 3], stage: u8) {
    let delay = stage_delay(pos, stage);
    schedule_tick(pos, delay);
    growth.pending.insert(pos, current_tick() + delay);
}

fn retry(growth: &mut Growth, pos: [i32; 3]) {
    schedule_tick(pos, DRY_RETRY);
    growth.pending.insert(pos, current_tick() + DRY_RETRY);
}

/// The jittered stage delay, a pure function of (position, stage) — stable
/// across sessions, no visit-order state.
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
