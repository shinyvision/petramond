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
use weather_core::FieldParams;

use crate::content::{Content, CropDef};
use crate::farmland::{self, Hydration};
use crate::keys;
use crate::rest::Rests;

/// Stage delay: 120–180 s at 20 TPS, jittered per (position, stage) so a
/// field planted in one sweep ripens staggered, not as one synchronized wave.
const STAGE_DELAY_MIN: i32 = 2400;
const STAGE_DELAY_MAX: i32 = 3600;

/// Fertile soil scales the stage delay by this fraction (~33% faster).
/// Fertility is read at ARM time — soil fertilized mid-delay speeds the
/// NEXT stage, matching how the wet/dry look also catches up lazily.
const FERTILE_DELAY_NUM: u64 = 2;
const FERTILE_DELAY_DEN: u64 = 3;

/// One-in-N chance of one bonus unit of primary produce per harvest on
/// fertile soil (N = 10 → the "10% more" rule).
const FERTILE_BONUS_IN: u64 = 10;

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

/// What the planting rule says about placing `block` at `pos`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Planting {
    /// Not a cultivated crop row — not this rule's business.
    NotACrop,
    /// Refused: a non-zero stage (no item places one — refused
    /// defensively), or soil that is not farmland.
    Refused,
    /// Farmland underneath: the crop may go in.
    Allowed,
    /// The soil cell cannot be inspected (unloaded / not stream-final).
    SoilUnknown,
}

/// The planting GATE, run by both instances: a stage-0 crop may only be
/// placed on farmland — dry or wet. The instances differ only in what they
/// do with a cell they cannot read (see [`on_place_pre`] and
/// [`predict_place_pre`]).
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

/// Placement gate (`block_place_pre`, server): the [`planting`] rule, plus
/// the light veto. Anything refused — including a half-streamed soil cell —
/// refuses WITHOUT consuming the seed; for the carrot the refusal is what
/// lets the contextual placeable-food rule fall back to eating.
pub fn on_place_pre(content: &Content, pos: [i32; 3], block: BlockId) -> Outcome {
    match planting(content, &SideWorld::Server, pos, block) {
        Planting::NotACrop => Outcome::Continue,
        // Planting in darkness quietly does nothing (a cancelled placement
        // never consumes the seed).
        Planting::Allowed if !too_dark(pos) => Outcome::Continue,
        Planting::Allowed | Planting::Refused | Planting::SoilUnknown => Outcome::Cancel,
    }
}

/// CLIENT prediction of [`on_place_pre`]: the same [`planting`] rule over
/// the replica. Known divergences, chosen over silent wrongness: no client
/// light read (a dark-cave planting over-jabs), and an uninspectable soil
/// cell never predicts a veto — the optimistic jab.
pub fn predict_place_pre(content: &Content, pos: [i32; 3], block: BlockId) -> Outcome {
    match planting(content, &SideWorld::Replica, pos, block) {
        Planting::Refused => Outcome::Cancel,
        Planting::NotACrop | Planting::Allowed | Planting::SoilUnknown => Outcome::Continue,
    }
}

/// The harvest's claim GATE, run by both instances (see [`crate::claims`]):
/// only a MATURE cultivated crop claims. Two deliberate PASSES (act-based
/// consumption — this consumer claims only what it harvests):
///
/// - An IMMATURE crop is only INSPECTED — checking maturity is free — so
///   its click falls through (fertilizer use, eating the held carrot,
///   placement against the face) and, if nothing acts, to no jab at all.
/// - A SNEAK click while holding a placeable block defers to the placement
///   consumer: sneak-to-build works against a ripe field. Sneaking with an
///   empty hand (or a non-block item) harvests like any other click.
///
/// Wild crops are not ours to handle here — they never right-click harvest.
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

/// Whether the crop cell is too dark to live (see [`MIN_GROW_LIGHT`]). Raw
/// light, deliberately not day/night-scaled — an open-sky field keeps its
/// skylight at night; darkness means burial or an unlit cave. An unresolved
/// read (`None` — unloaded / not stream-final) is never a dark verdict:
/// don't act on frozen state.
fn too_dark(pos: [i32; 3]) -> bool {
    light_at(pos).is_some_and(|l| l.combined < MIN_GROW_LIGHT)
}

/// A freshly planted crop schedules its first stage attempt.
pub fn on_placed(content: &Content, growth: &mut Growth, pos: [i32; 3], block: BlockId) {
    if let Some((_, stage @ 0..=2)) = content.crop_stage(block) {
        arm(growth, pos, stage, soil_is_fertile(content, pos));
    }
}

/// Harvest a gated (mature) crop (server): produce pops as nearby item
/// entities, the crop resets to its stage-0 block in the same tick (the
/// retained plant is one replanted seed/root), and the next growth attempt
/// is armed.
pub fn harvest(content: &Content, growth: &mut Growth, pos: [i32; 3], def: &CropDef) -> Outcome {
    let center = [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 0.4,
        pos[2] as f64 + 0.5,
    ];
    // Fertility read ONCE per interaction (a get_block crossing): it gates
    // the harvest bonus roll and shortens the re-arm delay below.
    let fertile = soil_is_fertile(content, pos);
    // The plant is RETAINED here (reset, not removed), so no replant is owed.
    spawn_all(mature_yield(def, fertile, Taking::Retained), center);
    emit_sound(keys::HARVEST_SOUND, Some(center));
    if let Some(emitter) = &def.harvest_emitter {
        emitter_burst(emitter, center, 1.0);
    }
    set_block(pos, def.stages[0]);
    arm(growth, pos, 0, fertile);
    Outcome::Cancel
}

/// How a mature plant was taken. The ONLY thing that may differ between the
/// two ways of taking a crop.
#[derive(Copy, Clone, PartialEq)]
enum Taking {
    /// Right-click harvest: the plant stays and resets to stage 0.
    Retained,
    /// Broken: the plant is gone and the taker has to plant again.
    Removed,
}

/// What ONE mature plant gives up — the single definition of a crop's yield,
/// shared by BOTH ways of taking it.
///
/// Harvesting and breaking a mature crop pay the SAME produce, the same extra
/// roll, and the same fertile-soil bonus. A player who never learns that
/// mature crops can be right-clicked must not be quietly taxed for it; the
/// interaction is a convenience, not a reward tier.
///
/// The one honest difference is [`Taking::Removed`]: a broken plant is gone, so
/// the yield guarantees at least one PLANTING STOCK back, or breaking a field
/// would strand the player with nothing to replant. That is a floor, not a
/// bonus — a crop whose stock is its own produce (the carrot, the potato)
/// already clears it and gets nothing extra.
///
/// Every crop inherits this from its `CropSpec` row. Adding crop #N changes
/// nothing here.
fn mature_yield(def: &CropDef, fertile: bool, taking: Taking) -> Vec<(&str, u8)> {
    let roll =
        |key: &str, (lo, hi): (u64, u64)| -> u8 { (lo + rng_u64(key) % (hi - lo + 1)) as u8 };
    let mut out = vec![(
        def.produce.as_str(),
        roll(&def.harvest_key, def.yield_range)
            + (fertile && rng_u64(&def.fertile_key).is_multiple_of(FERTILE_BONUS_IN)) as u8,
    )];
    if let Some(extra) = &def.extra_drop {
        // A 100% row draws NO chance roll, so the crops that always threw
        // seeds keep the stream they have always had.
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

/// Drop a yield list at `center`, skipping the empty entries a roll can produce.
fn spawn_all(items: Vec<(&str, u8)>, center: [f64; 3]) {
    for (item, count) in items {
        if count > 0 {
            spawn_item(item, count, center);
        }
    }
}

/// Breaking a crop instead of harvesting it.
///
/// The pack owns this outright — every cultivated stage row declares
/// `"drops": []` — because the yield has to be the SAME one
/// [`mature_yield`] computes for a harvest, and row data cannot read the
/// fertility of the soil underneath.
///
/// Fires for NATURAL breaks too (water washing a field away is the classic
/// water-harvest, and it takes the plant just as thoroughly), gated only on
/// `harvested` so a break that yields nothing keeps yielding nothing.
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
        // An unripe plant is worth exactly what was put into it.
        spawn_item(&def.planting_stock, 1, center);
        return;
    }
    spawn_all(
        mature_yield(def, soil_is_fertile(content, pos), Taking::Removed),
        center,
    );
}

/// The crop block hooks.
/// `sky` is the weather field heard this tick (`None` = clear sky).
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

/// One due growth attempt.
fn attempt(content: &Content, growth: &mut Growth, sky: Option<&FieldParams>, pos: [i32; 3]) {
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
    let Some((def, stage @ 0..=2)) = content.crop_stage(block) else {
        return; // broken, replaced, or already mature — nothing owed
    };
    let below = [pos[0], pos[1] - 1, pos[2]];
    let fertile = match get_block(below) {
        Some(b) if content.is_farmland(b) => content.is_fertile(b),
        Some(_) => return, // support gone; the neighbor hook owns the pop
        None => {
            retry(growth, pos);
            return;
        }
    };
    // Growth needs light: a due attempt in the dark pauses like dryness (the
    // next RANDOM tick is what breaks a dark crop — see `rearm_if_lost`).
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
    // A stand that survived the darkness check is a stand a pest could smell:
    // the same heartbeat carries the attraction roll (see `attract`).
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

/// Supporting-soil invalidation: a crop whose ground is no longer farmland
/// (broken OR replaced by something else) pops its planting stock rather
/// than vanishing or floating. `None` below = streaming; leave it alone.
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

/// A crop dying in place (soil invalidated, or left in the dark): the plant
/// goes and one planting stock pops — never lost, never a free harvest.
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

/// Schedule the next stage attempt after the deterministic jittered delay,
/// shortened on fertile soil (fertility is read at arm time — soil
/// fertilized mid-delay speeds the NEXT stage; callers that already hold
/// the below-block thread the verdict in instead of re-reading it).
fn arm(growth: &mut Growth, pos: [i32; 3], stage: u8, fertile: bool) {
    let mut delay = stage_delay(pos, stage);
    if fertile {
        delay = delay * FERTILE_DELAY_NUM / FERTILE_DELAY_DEN;
    }
    schedule_tick(pos, delay);
    growth.pending.insert(pos, current_tick() + delay);
}

/// Whether the soil under a crop cell is fertile farmland.
fn soil_is_fertile(content: &Content, pos: [i32; 3]) -> bool {
    get_block([pos[0], pos[1] - 1, pos[2]]).is_some_and(|b| content.is_fertile(b))
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
