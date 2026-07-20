//! Husbandry: grazing saturation, drinking, love mode, courtship, offspring.
//!
//! Breeding is earned through a well-kept pasture, never a one-item faucet
//! (the wild population stays a finite harvest — see the engine's spawn
//! design). The species that participate are [`Content::husbandry`] rows;
//! nothing here branches on a concrete species.
//!
//! The system is split across the two mod seams by responsibility:
//!
//! - The SIM SWEEP ([`on_tick`], a tick system after the Mobs stage) owns the
//!   whole state machine: the jittered saturation trickle, the periodic
//!   heartbeat (grass-seek and love-entry rolls), eating, pairing two mobs in
//!   love, the courtship timer, and spawning the newborn. It runs every
//!   [`SWEEP_EVERY`] ticks over the animals near players and touches the
//!   world only through host calls.
//! - The AI NODE (`farming:husbandry_goal`, [`decide`]) only STEERS and
//!   POSES: it reads the destination tags the sweep leaves on the mob and
//!   emits a navigation goal (courting partner first, then the trough, then
//!   grass), standing still when close; while a munch/sip is active it holds
//!   position and bobs the head down at grass level (a procedural
//!   `head_look`, no authored clip needed). It never rolls chances and never
//!   writes state.
//!
//! EATING AND DRINKING take [`CONSUME_TICKS`] of head-down munching before
//! the effect lands (the plant breaks / the sip counts / the meal lands).
//! Drinking drains the pasture's water trough: every [`TROUGH_SIPS`] sips
//! flip it to the empty block. A WHEAT-PACKED trough is the kept feed store:
//! a hungry animal ALWAYS seeks it before grazing the pasture down
//! ([`FEED_CELL`] — thirst alone outranks it), every meal restores the
//! species' `restore` saturation, and the [`TROUGH_MEALS`]th bite flips the
//! trough to empty — so keeping the flock means keeping water topped up,
//! feed stocked, and grass replanted. The love-mode gate still reads the
//! FILLED (water) trough only.
//!
//! ALL per-animal state rides the mob's own tag map, so it persists, travels,
//! and dies with the animal: `farming:saturation` (ABSENT = full — healthy
//! animals carry nothing), the two timer deadlines, the packed destination
//! cells, `farming:love_until`, `farming:partner` (+ courtship progress on
//! the lower-id partner), and `farming:breed_cool`. Partner links speak
//! SESSION mob ids, so a save/reload simply re-pairs — every other tag
//! survives the reload meaningfully.

use mod_sdk::*;

use crate::content::{Content, HusbandryDef};

/// Sweep cadence (ticks). Courtship progress accrues in these increments.
const SWEEP_EVERY: u64 = 5;
/// Husbandry is simulated for animals within this radius of any player.
const RANGE: f32 = 64.0;

/// Saturation is `0..=SAT_MAX`; an ABSENT tag reads as full.
const SAT_MAX: i64 = 10;
/// Saturation trickles down 1 every 200 + (0..=1000) ticks.
const TRICKLE_MIN: u64 = 200;
const TRICKLE_SPAN: u64 = 1001;
/// The per-animal heartbeat: grass-seek and love-entry chances roll on this
/// cadence, and a stale graze target expires with it.
const HEARTBEAT: u64 = 600;
/// Grass is sought within this box radius (blocks, the prompt's 8).
const SEEK_RADIUS: i32 = 8;
/// A filled trough within this box radius enables love mode.
const TROUGH_RADIUS: i32 = 8;
/// Saturation must exceed this to enter love mode.
const LOVE_SAT: i64 = 7;
/// Love-entry chance per heartbeat while eligible (1 in N).
const LOVE_CHANCE_IN: u64 = 4;
/// Love mode holds this long while no partner completes courtship.
const LOVE_TICKS: u64 = 2400;
/// Two mobs in love pair up within this distance.
const PAIR_RANGE: f32 = 24.0;
/// Courting partners must stay within this distance for the timer to run.
const COURT_NEAR: f32 = 2.5;
/// Consecutive closeness required before the newborn spawns.
const COURT_TICKS: i64 = 100;
/// Parents refuse re-breeding this long after a birth.
const BREED_COOLDOWN: u64 = 6000;
/// A newborn juvenile carries [`BABY`] this long (10 minutes) before it
/// grows into the adult species.
const BABY_TICKS: u64 = 12_000;
/// A grazer this close to its target plant (horizontally) starts eating.
const EAT_RANGE: f32 = 1.4;
/// Head-down munch/sip duration before the consumption lands.
const CONSUME_TICKS: u64 = 40;
/// Drinking cadence: every 1200 + (0..=4800) ticks while a trough is near.
const DRINK_MIN: u64 = 1200;
const DRINK_SPAN: u64 = 4801;
/// A due drink with no reachable filled trough retries this much later.
const DRINK_RETRY: u64 = 600;
/// This close to the trough cell (horizontally) starts the sip — the block
/// is solid, so the animal stands beside it.
const DRINK_RANGE: f32 = 1.75;
/// Sips a filled trough holds before it flips to the empty block.
const TROUGH_SIPS: u8 = 5;
/// Meals a packed wheat trough holds ([`MEALS_PER_WHEAT`] per fill wheat) —
/// the twelfth bite flips it to the empty block.
pub const TROUGH_MEALS: u8 = 12;
/// One wheat is worth this many meals — the take-out yields the un-eaten
/// wheat back at this rate, floored (partial nibbles are lost).
pub const MEALS_PER_WHEAT: u8 = 4;

/// The wheat a take-out returns for `meals` bites already eaten: the
/// remaining meals over [`MEALS_PER_WHEAT`], floored — a fresh trough gives
/// its full fill back, a nearly-finished one can give nothing.
pub fn wheat_yield(meals: u8) -> u8 {
    TROUGH_MEALS.saturating_sub(meals) / MEALS_PER_WHEAT
}

/// The authored feeding clip (a named ONE-SHOT `.bbmodel` animation — the
/// sheep's head-dip-and-chew). Activation is FIRE-AND-FORGET: the engine
/// retires a played-through one-shot layer on its own, and this module never
/// deactivates it — a started bite always finishes, even when the consume
/// completes or cancels first (per Rachel: animations are never cut short).
/// Every husbandry species' model is expected to carry one; a model without
/// it shows no clip (the engine draws nothing for unknown names) and the
/// [`HEAD_DOWN`] fallback pose still reads as feeding.
const EAT_ANIM: &str = "eat";

// The fallback munch/sip pose (radians, head pitch relative to the body —
// negative is down). The renderer SUPPRESSES head-look while an active
// animation drives the head bone, so with the [`EAT_ANIM`] clip playing
// this is inert; it only shows on a model lacking the clip.
const HEAD_DOWN: f32 = -0.9;
const HEAD_RAISED: f32 = -0.45;
/// Ticks per half bob (down ↔ half-raised) of the fallback pose.
const BOB_HALF_PERIOD: u64 = 8;

/// An animal must face its plant/trough within this angle to start
/// consuming (radians, ~30°); a close-but-turned animal is yawed toward it
/// first ([`TURN_STEP`] per sweep, the kinematic-drive assist).
const FACE_TOL: f32 = 0.5;
const TURN_STEP: f32 = 1.0;

// Tag keys — all state this module owns on an animal.
const SATURATION: &str = "farming:saturation";
const SAT_NEXT: &str = "farming:sat_next";
const HEART_NEXT: &str = "farming:husbandry_next";
const GRAZE_CELL: &str = "farming:graze_cell";
/// The packed wheat-trough cell a hungry animal is walking to / munching —
/// the trough feed errand (outranks grass, loses to thirst).
const FEED_CELL: &str = "farming:feed_cell";
const DRINK_NEXT: &str = "farming:drink_next";
const DRINK_CELL: &str = "farming:drink_cell";
const CONSUME_UNTIL: &str = "farming:consume_until";
const LOVE_UNTIL: &str = "farming:love_until";
const PARTNER: &str = "farming:partner";
const COURT_CELL: &str = "farming:court_cell";
const COURT: &str = "farming:court_ticks";
const BREED_COOL: &str = "farming:breed_cool";

/// Cell-KV sip counter on a filled trough's member cells (kept in sync
/// across the group — either cell may be the drink target).
const SIPS_KEY: &str = "farming:sips";
/// Cell-KV meal counter on a wheat trough's member cells (same sync rule —
/// either cell may be targeted, by the flock or by the take-out).
const MEALS_KEY: &str = "farming:meals";

/// `Int` absolute tick a juvenile stays a baby until. The sweep deletes the
/// tag when due; the REMOVAL (whoever performs it — the timer, a future
/// early-grow mechanic, a debug hand) is what triggers the growth in
/// [`crate::growth`], via the engine's `mob_tag_removed` post event.
pub const BABY: &str = "farming:baby";

// --- packed cells ----------------------------------------------------------
// A destination cell rides ONE Int tag: x and z in 26 signed bits, y in 12
// (bit layout x:38..64, y:26..38, z:0..26) — comfortably the world's range.

fn pack_cell(c: [i32; 3]) -> i64 {
    const M26: i64 = (1 << 26) - 1;
    const M12: i64 = (1 << 12) - 1;
    ((c[0] as i64 & M26) << 38) | ((c[1] as i64 & M12) << 26) | (c[2] as i64 & M26)
}

fn unpack_cell(v: i64) -> [i32; 3] {
    [
        (v >> 38) as i32,
        ((v << 26) >> 52) as i32,
        ((v << 38) >> 38) as i32,
    ]
}

// --- tag views -------------------------------------------------------------

fn tag_i64(tags: &[(String, MobTagValue)], key: &str) -> Option<i64> {
    tags.iter().find_map(|(k, v)| match v {
        MobTagValue::I64(i) if k == key => Some(*i),
        _ => None,
    })
}

/// One animal's whole husbandry state, read once per sweep and committed as
/// a diff — only actual transitions write tags (the copy-on-write contract).
#[derive(Clone, PartialEq)]
struct State {
    sat: i64,
    sat_next: Option<i64>,
    heart_next: Option<i64>,
    graze: Option<i64>,
    feed_cell: Option<i64>,
    drink_next: Option<i64>,
    drink_cell: Option<i64>,
    consume_until: Option<i64>,
    love_until: Option<i64>,
    partner: Option<i64>,
    court_cell: Option<i64>,
    court: Option<i64>,
    breed_cool: Option<i64>,
}

impl State {
    fn read(tags: &[(String, MobTagValue)]) -> State {
        State {
            sat: tag_i64(tags, SATURATION).unwrap_or(SAT_MAX).clamp(0, SAT_MAX),
            sat_next: tag_i64(tags, SAT_NEXT),
            heart_next: tag_i64(tags, HEART_NEXT),
            graze: tag_i64(tags, GRAZE_CELL),
            feed_cell: tag_i64(tags, FEED_CELL),
            drink_next: tag_i64(tags, DRINK_NEXT),
            drink_cell: tag_i64(tags, DRINK_CELL),
            consume_until: tag_i64(tags, CONSUME_UNTIL),
            love_until: tag_i64(tags, LOVE_UNTIL),
            partner: tag_i64(tags, PARTNER),
            court_cell: tag_i64(tags, COURT_CELL),
            court: tag_i64(tags, COURT),
            breed_cool: tag_i64(tags, BREED_COOL),
        }
    }

    fn in_love(&self, tick: u64) -> bool {
        self.love_until.is_some_and(|until| (tick as i64) < until)
    }

    fn end_love(&mut self) {
        self.love_until = None;
        self.partner = None;
        self.court_cell = None;
        self.court = None;
    }

    /// Write only what changed against `was`. A full-saturation animal's
    /// saturation tag is DELETED, so healthy animals carry no state.
    fn commit(&self, was: &State, mob_id: u64) {
        let int = |key, old: Option<i64>, new: Option<i64>| {
            if old == new {
                return;
            }
            match new {
                Some(v) => {
                    mob_tag_set(mob_id, key, MobTagValue::I64(v));
                }
                None => {
                    mob_tag_delete(mob_id, key);
                }
            }
        };
        let sat = |s: &State| (s.sat < SAT_MAX).then_some(s.sat);
        int(SATURATION, sat(was), sat(self));
        int(SAT_NEXT, was.sat_next, self.sat_next);
        int(HEART_NEXT, was.heart_next, self.heart_next);
        int(GRAZE_CELL, was.graze, self.graze);
        int(FEED_CELL, was.feed_cell, self.feed_cell);
        int(DRINK_NEXT, was.drink_next, self.drink_next);
        int(DRINK_CELL, was.drink_cell, self.drink_cell);
        int(CONSUME_UNTIL, was.consume_until, self.consume_until);
        int(LOVE_UNTIL, was.love_until, self.love_until);
        int(PARTNER, was.partner, self.partner);
        int(COURT_CELL, was.court_cell, self.court_cell);
        int(COURT, was.court, self.court);
        int(BREED_COOL, was.breed_cool, self.breed_cool);
    }
}

/// One animal in this sweep's worklist.
struct Animal {
    def: usize,
    snap: MobSnapshot,
    was: State,
    now: State,
}

impl Animal {
    fn cell(&self) -> [i32; 3] {
        [
            self.snap.pos[0].floor() as i32,
            self.snap.pos[1].floor() as i32,
            self.snap.pos[2].floor() as i32,
        ]
    }
}

/// How many nearest candidates a destination roll may reachability-probe
/// before giving up this round (mirrors the engine wander's bounded
/// retries). The heartbeat re-rolls, so a round that finds nothing
/// reachable just waits.
const REACH_PROBES: usize = 5;

/// The nearest of `cells` the animal can genuinely WALK to, probing at most
/// [`REACH_PROBES`] candidates nearest-first. Nearest-by-distance alone is a
/// trap: the engine walks best-effort partial routes toward unreachable
/// goals (chases must crowd their target), so grass beyond the fence — or a
/// trough in the neighbouring pen — pins a penned animal against the fence
/// forever, re-picked every heartbeat.
fn nearest_reachable(a: &Animal, mut cells: Vec<[i32; 3]>) -> Option<[i32; 3]> {
    let c = a.cell();
    cells.sort_by_key(|p| {
        let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
    });
    cells
        .into_iter()
        .take(REACH_PROBES)
        .find(|&p| can_stand_by(a, p))
}

/// [`mob_can_reach`] loosened to CONSUME semantics: every husbandry act
/// happens standing WITHIN RANGE of its target (a sip beside the trough — a
/// trough's basin can never be walked into — a bite beside the plant, a
/// courtship beside the partner), so the target cell or any cardinal
/// neighbour being walkable is enough. Deliberately generous at pen borders:
/// a plant right against the far side of the fence is honestly consumable
/// from the near side, and a trough embedded in a shared fence line serves
/// both pens.
fn can_stand_by(a: &Animal, p: [i32; 3]) -> bool {
    [[0, 0], [1, 0], [-1, 0], [0, 1], [0, -1]]
        .iter()
        .any(|d| mob_can_reach(a.snap.id, [p[0] + d[0], p[1], p[2] + d[1]]))
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

// --- the sweep -------------------------------------------------------------

pub fn on_tick(content: &Content) {
    let tick = current_tick();
    if !tick.is_multiple_of(SWEEP_EVERY) {
        return;
    }
    let mut animals: Vec<Animal> = Vec::new();
    for player in players() {
        for snap in mobs_in_radius(player.state.pos, RANGE) {
            let Some(def) = content.husbandry.iter().position(|d| d.kind == snap.kind) else {
                continue;
            };
            if animals.iter().any(|a| a.snap.id == snap.id) {
                continue;
            }
            let Some(tags) = mob_tags_get(snap.id) else {
                continue;
            };
            let was = State::read(&tags);
            let now = was.clone();
            animals.push(Animal {
                def,
                snap,
                was,
                now,
            });
        }
    }
    for a in &mut animals {
        step_animal(content, &content.husbandry[a.def], a, tick);
    }
    court(content, &mut animals, tick);
    for a in &animals {
        a.now.commit(&a.was, a.snap.id);
    }
    // Juvenile growth: expire due baby tags. The DELETE is the whole trigger
    // — the engine announces the removal and growth.rs turns the juvenile
    // into its adult species there, so any other remover grows it the same
    // way.
    for baby in mobs_with_tag(BABY, None) {
        if let MobTagLookup::Value(MobTagValue::I64(until)) = mob_tag_get(baby.id, BABY) {
            if tick as i64 >= until {
                mob_tag_delete(baby.id, BABY);
            }
        }
    }
}

/// The per-animal state machine: trickle, heartbeat rolls, eating, love
/// expiry. Pairing and courtship need the whole worklist and run after.
fn step_animal(content: &Content, def: &HusbandryDef, a: &mut Animal, tick: u64) {
    let s = &mut a.now;
    // Saturation trickles down on a jittered per-animal deadline.
    match s.sat_next {
        None => s.sat_next = Some((tick + TRICKLE_MIN + rng_u64("husbandry_trickle") % TRICKLE_SPAN) as i64),
        Some(due) if tick as i64 >= due => {
            s.sat = (s.sat - 1).max(0);
            s.sat_next = Some((tick + TRICKLE_MIN + rng_u64("husbandry_trickle") % TRICKLE_SPAN) as i64);
        }
        Some(_) => {}
    }
    // Love mode expires quietly; the pair re-forms if both re-enter.
    if s.love_until.is_some() && !s.in_love(tick) {
        s.end_love();
    }
    if s.breed_cool.is_some_and(|until| tick as i64 >= until) {
        s.breed_cool = None;
    }
    // The heartbeat: seek/love chances, and stale walk targets expire (an
    // unreachable plant or trough must not pin the animal forever). An
    // ACTIVE munch/sip is never disturbed.
    let due = match s.heart_next {
        // First sight of this animal: phase its heartbeat so a herd doesn't
        // scan the world in lockstep.
        None => {
            s.heart_next = Some((tick + rng_u64("husbandry_phase") % HEARTBEAT) as i64);
            false
        }
        Some(due) => tick as i64 >= due,
    };
    if due {
        s.heart_next = Some((tick + HEARTBEAT) as i64);
        if s.consume_until.is_none() {
            s.graze = None;
            s.feed_cell = None;
            if s.drink_cell.take().is_some() {
                s.drink_next = Some((tick + DRINK_RETRY) as i64);
            }
            if !s.in_love(tick) {
                roll_love(content, a, tick);
                roll_graze(content, def, a);
            }
        }
    }
    // Drinking runs on its own long cadence, whenever a filled trough is
    // near — thirst is upkeep, not hunger.
    match a.now.drink_next {
        None => {
            a.now.drink_next =
                Some((tick + DRINK_MIN + rng_u64("husbandry_drink") % DRINK_SPAN) as i64)
        }
        Some(due)
            if tick as i64 >= due
                && a.now.consume_until.is_none()
                && a.now.drink_cell.is_none() =>
        {
            roll_drink(content, a, tick);
        }
        Some(_) => {}
    }
    consume_step(content, def, a, tick);
}

/// A due drink: target the nearest filled trough, or push the deadline back
/// a little when none is readable in range.
fn roll_drink(content: &Content, a: &mut Animal, tick: u64) {
    let c = a.cell();
    let r = TROUGH_RADIUS;
    let found = find_blocks(
        [c[0] - r, c[1] - r, c[2] - r],
        [c[0] + r, c[1] + r, c[2] + r],
        vec![content.trough_filled],
    );
    let nearest = found.and_then(|cells| nearest_reachable(a, cells));
    match nearest {
        Some(p) => {
            a.now.drink_cell = Some(pack_cell(p));
            // The walk to water outranks a pending grass or feed errand.
            a.now.graze = None;
            a.now.feed_cell = None;
        }
        None => a.now.drink_next = Some((tick + DRINK_RETRY) as i64),
    }
}

/// Love-mode entry: saturation above [`LOVE_SAT`], no cooldown, the chance
/// roll, and a FILLED trough nearby.
fn roll_love(content: &Content, a: &mut Animal, tick: u64) {
    if a.now.sat <= LOVE_SAT || a.now.breed_cool.is_some() {
        return;
    }
    if !rng_u64("husbandry_love").is_multiple_of(LOVE_CHANCE_IN) {
        return;
    }
    let c = a.cell();
    let r = TROUGH_RADIUS;
    let troughs = find_blocks(
        [c[0] - r, c[1] - r, c[2] - r],
        [c[0] + r, c[1] + r, c[2] + r],
        vec![content.trough_filled],
    );
    // None = terrain still streaming: no verdict, retry next heartbeat.
    if troughs.is_some_and(|t| !t.is_empty()) {
        a.now.love_until = Some((tick + LOVE_TICKS) as i64);
        a.now.graze = None;
    }
}

/// The hunger roll: chance grows one step per missing saturation point
/// ((SAT_MAX - sat) / 10 per heartbeat). A hit targets the nearest REACHABLE
/// food in the seek box — a packed wheat trough ALWAYS beats grazing (a kept
/// feed store saves the pasture), grass plants only when no trough serves.
fn roll_graze(content: &Content, def: &HusbandryDef, a: &mut Animal) {
    if a.now.sat >= SAT_MAX || a.now.graze.is_some() || a.now.feed_cell.is_some() {
        return;
    }
    if (rng_u64("husbandry_graze") % 10) as i64 >= SAT_MAX - a.now.sat {
        return;
    }
    let c = a.cell();
    let r = SEEK_RADIUS;
    // The trough first. None = terrain still streaming: no verdict, retry
    // next heartbeat — never a false "no trough" that falls back to grass.
    let Some(troughs) = find_blocks(
        [c[0] - r, c[1] - r, c[2] - r],
        [c[0] + r, c[1] + r, c[2] + r],
        vec![content.trough_wheat],
    ) else {
        return;
    };
    if let Some(p) = nearest_reachable(a, troughs) {
        a.now.feed_cell = Some(pack_cell(p));
        return;
    }
    let Some(found) = find_blocks(
        [c[0] - r, c[1] - r, c[2] - r],
        [c[0] + r, c[1] + r, c[2] + r],
        def.food.clone(),
    ) else {
        return;
    };
    a.now.graze = nearest_reachable(a, found).map(pack_cell);
}

/// Horizontally within `range` of the cell's centre, feet within `dy_tol`
/// of its base.
fn near_cell(a: &Animal, cell: [i32; 3], range: f32, dy_tol: f32) -> bool {
    let dx = a.snap.pos[0] - (cell[0] as f32 + 0.5);
    let dz = a.snap.pos[2] - (cell[2] as f32 + 0.5);
    let dy = a.snap.pos[1] - cell[1] as f32;
    dx * dx + dz * dz <= range * range && dy.abs() <= dy_tol
}

fn wrap_angle(v: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (v + PI).rem_euclid(TAU) - PI
}

/// Whether the animal faces `cell` within [`FACE_TOL`] — required to start
/// a munch/sip. A close-but-turned animal is yawed toward the target
/// instead (a standing kinematic-drive turn, one [`TURN_STEP`] per sweep;
/// the drive claims locomotion only for this tick and plays no walk).
/// Standing ON the target cell (grazing underfoot) needs no facing.
fn facing_or_turn(a: &Animal, cell: [i32; 3]) -> bool {
    let dx = (cell[0] as f32 + 0.5) - a.snap.pos[0];
    let dz = (cell[2] as f32 + 0.5) - a.snap.pos[2];
    if dx * dx + dz * dz < 0.36 {
        return true;
    }
    // Mob convention: yaw 0 faces -Z, facing = (-sin yaw, -cos yaw).
    let bearing = (-dx).atan2(-dz);
    let err = wrap_angle(bearing - a.snap.yaw);
    if err.abs() <= FACE_TOL {
        return true;
    }
    mob_drive(
        a.snap.id,
        [0.0, 0.0],
        Some(a.snap.yaw + err.clamp(-TURN_STEP, TURN_STEP)),
    );
    false
}

/// Drinking happens standing on the GROUND beside the trough — never from
/// on top of it (feet planted on the trough block itself).
fn standing_on_trough(content: &Content, a: &Animal) -> bool {
    let feet = a.cell();
    let below = [feet[0], feet[1] - 1, feet[2]];
    [feet, below]
        .into_iter()
        .any(|c| get_block(c).is_some_and(|b| is_any_trough(content, b)))
}

/// Any trough block, whatever it currently holds.
fn is_any_trough(content: &Content, b: BlockId) -> bool {
    b == content.trough || b == content.trough_filled || b == content.trough_wheat
}

/// Eating and drinking: an arrival starts a [`CONSUME_TICKS`] head-down
/// munch/sip (the AI node poses it), and the effect lands only when it runs
/// out — the plant breaks / the sip counts. A target that vanishes mid-walk
/// or mid-munch (grazed by a flockmate, drained, washed away) cancels
/// cleanly; an unloaded read holds and retries.
fn consume_step(content: &Content, def: &HusbandryDef, a: &mut Animal, tick: u64) {
    if let Some(until) = a.now.consume_until {
        let done = tick as i64 >= until;
        if let Some(packed) = a.now.drink_cell {
            let cell = unpack_cell(packed);
            match get_block(cell) {
                None => return,
                Some(b) if b == content.trough_filled => {}
                Some(_) => {
                    a.now.drink_cell = None;
                    a.now.consume_until = None;
                    a.now.drink_next = Some((tick + DRINK_RETRY) as i64);
                    return;
                }
            }
            if done {
                finish_drink(content, a, cell, tick);
            }
        } else if let Some(packed) = a.now.feed_cell {
            let cell = unpack_cell(packed);
            match get_block(cell) {
                None => return,
                Some(b) if b == content.trough_wheat => {}
                Some(_) => {
                    a.now.feed_cell = None;
                    a.now.consume_until = None;
                    return;
                }
            }
            if done {
                finish_feed(content, def, a, cell);
            }
        } else if let Some(packed) = a.now.graze {
            let cell = unpack_cell(packed);
            match get_block(cell) {
                None => return,
                Some(b) if def.food.contains(&b) => {}
                Some(_) => {
                    a.now.graze = None;
                    a.now.consume_until = None;
                    return;
                }
            }
            if done && set_block(cell, BlockId::AIR) {
                a.now.sat = (a.now.sat + def.restore).min(SAT_MAX);
                a.now.graze = None;
                a.now.consume_until = None;
            }
        } else {
            a.now.consume_until = None;
        }
        return;
    }
    // No active munch/sip: an arrival at the current target starts one —
    // close enough, FACING it (a turned animal is yawed toward it first),
    // and for the trough, standing on the ground beside it, never on it.
    // Water outranks feed, feed outranks grass — the targeting order.
    let start = |a: &mut Animal| {
        a.now.consume_until = Some((tick + CONSUME_TICKS) as i64);
        mob_anim_set(a.snap.id, EAT_ANIM, true);
    };
    if let Some(packed) = a.now.drink_cell {
        let cell = unpack_cell(packed);
        match get_block(cell) {
            None => {}
            Some(b) if b == content.trough_filled => {
                if near_cell(a, cell, DRINK_RANGE, 0.75)
                    && !standing_on_trough(content, a)
                    && facing_or_turn(a, cell)
                {
                    start(a);
                }
            }
            Some(_) => {
                a.now.drink_cell = None;
                a.now.drink_next = Some((tick + DRINK_RETRY) as i64);
            }
        }
        return;
    }
    if let Some(packed) = a.now.feed_cell {
        let cell = unpack_cell(packed);
        match get_block(cell) {
            None => {}
            Some(b) if b == content.trough_wheat => {
                if near_cell(a, cell, DRINK_RANGE, 0.75)
                    && !standing_on_trough(content, a)
                    && facing_or_turn(a, cell)
                {
                    start(a);
                }
            }
            Some(_) => a.now.feed_cell = None,
        }
        return;
    }
    if let Some(packed) = a.now.graze {
        let cell = unpack_cell(packed);
        match get_block(cell) {
            None => {}
            Some(b) if def.food.contains(&b) => {
                if near_cell(a, cell, EAT_RANGE, 2.0) && facing_or_turn(a, cell) {
                    start(a);
                }
            }
            Some(_) => a.now.graze = None,
        }
    }
}

/// The sip lands: bump the trough's counter (kept in sync across the
/// group's member cells), flip it to the empty block at [`TROUGH_SIPS`],
/// and schedule the animal's next drink.
fn finish_drink(content: &Content, a: &mut Animal, cell: [i32; 3], tick: u64) {
    let sips = crate::kv_counter::kv_counter_bump(cell, SIPS_KEY);
    if sips >= TROUGH_SIPS {
        // The swap carries cell KV across, so the spent counter is scrubbed
        // explicitly — a refill must start on fresh water.
        swap_model_block(cell, content.trough);
        clear_sips(content, cell);
    } else {
        for member in trough_members(content, cell) {
            section_kv_set(member, SIPS_KEY, vec![sips]);
        }
    }
    a.now.drink_cell = None;
    a.now.consume_until = None;
    a.now.drink_next = Some((tick + DRINK_MIN + rng_u64("husbandry_drink") % DRINK_SPAN) as i64);
}

/// The meal lands: saturation restored, the trough's meal counter bumped
/// (same member-cell sync as the sips) — and at [`TROUGH_MEALS`] the feed is
/// gone and the trough flips to the empty block.
fn finish_feed(content: &Content, def: &HusbandryDef, a: &mut Animal, cell: [i32; 3]) {
    let meals = crate::kv_counter::kv_counter_bump(cell, MEALS_KEY);
    if meals >= TROUGH_MEALS {
        swap_model_block(cell, content.trough);
        clear_meals(content, cell);
    } else {
        for member in trough_members(content, cell) {
            section_kv_set(member, MEALS_KEY, vec![meals]);
        }
    }
    a.now.sat = (a.now.sat + def.restore).min(SAT_MAX);
    a.now.feed_cell = None;
    a.now.consume_until = None;
}

/// The trough group's member cells around (and including) `cell` — the
/// [2,1,1] footprint read from the world, any fill state.
fn trough_members(content: &Content, cell: [i32; 3]) -> Vec<[i32; 3]> {
    let mut cells = vec![cell];
    for d in [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]] {
        let n = [cell[0] + d[0], cell[1] + d[1], cell[2] + d[2]];
        if get_block(n).is_some_and(|b| is_any_trough(content, b)) {
            cells.push(n);
        }
    }
    cells
}

/// Scrub the sip counter off every member cell of the trough at `pos`.
/// Also called by the bucket paths ([`crate::trough`]): cell KV rides model
/// swaps, so both fill and drain must reset the count — fresh water holds
/// fresh sips, and collected water can't leave a stale count behind.
pub fn clear_sips(content: &Content, pos: [i32; 3]) {
    for member in trough_members(content, pos) {
        section_kv_delete(member, SIPS_KEY);
    }
}

/// The meal count on the trough at `pos` (absent = untouched feed). Synced
/// across the group's member cells, so either cell answers.
pub fn meals_at(pos: [i32; 3]) -> u8 {
    section_kv_get(pos, MEALS_KEY)
        .and_then(|b| b.first().copied())
        .unwrap_or(0)
}

/// Scrub the meal counter off every member cell — called by the take-out
/// swap ([`crate::trough`]) and by the last meal's own swap: an emptied
/// trough must not bank a stale count for the next fill.
pub fn clear_meals(content: &Content, pos: [i32; 3]) {
    for member in trough_members(content, pos) {
        section_kv_delete(member, MEALS_KEY);
    }
}

/// Pairing and courtship over the whole worklist: mobs in love pair up
/// nearest-first within their own species, keep each other's cell as a
/// steering target, and a pair that stays close long enough gets a newborn
/// spawned between them.
fn court(content: &Content, animals: &mut [Animal], tick: u64) {
    // Validate existing partner links: the partner must be in this sweep, in
    // love, same species, and pointing back. Anything else unlinks (love
    // itself holds — the animal re-pairs when a candidate appears).
    let lover = |a: &Animal| a.now.in_love(tick);
    for i in 0..animals.len() {
        let Some(pid) = animals[i].now.partner else {
            continue;
        };
        let ok = lover(&animals[i])
            && animals.iter().any(|b| {
                b.snap.id as i64 == pid
                    && lover(b)
                    && b.def == animals[i].def
                    && b.now.partner == Some(animals[i].snap.id as i64)
            });
        if !ok {
            let a = &mut animals[i].now;
            a.partner = None;
            a.court_cell = None;
            a.court = None;
        }
    }
    // Pair the unpaired: first unpaired lover takes its nearest unpaired
    // same-species lover in range (worklist order — deterministic).
    for i in 0..animals.len() {
        if !lover(&animals[i]) || animals[i].now.partner.is_some() {
            continue;
        }
        let mut near: Vec<(usize, f32)> = (0..animals.len())
            .filter(|&j| j != i)
            .filter(|&j| animals[j].def == animals[i].def)
            .filter(|&j| lover(&animals[j]) && animals[j].now.partner.is_none())
            .map(|j| (j, dist2(animals[i].snap.pos, animals[j].snap.pos)))
            .filter(|&(_, d2)| d2 <= PAIR_RANGE * PAIR_RANGE)
            .collect();
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        // A lover the animal cannot WALK to (the fence between two pens) is
        // no candidate: courtship needs consecutive closeness, so an
        // unreachable pairing just presses both against the fence until
        // love expires.
        let near = near
            .into_iter()
            .take(REACH_PROBES)
            .find(|&(j, _)| can_stand_by(&animals[i], animals[j].cell()));
        if let Some((j, _)) = near {
            animals[i].now.partner = Some(animals[j].snap.id as i64);
            animals[j].now.partner = Some(animals[i].snap.id as i64);
        }
    }
    // Courtship: visit each pair once (lower worklist index leads and holds
    // the shared progress counter).
    for i in 0..animals.len() {
        let Some(pid) = animals[i].now.partner else {
            continue;
        };
        let Some(j) = animals.iter().position(|b| b.snap.id as i64 == pid) else {
            continue;
        };
        if j <= i {
            continue;
        }
        // Each steers toward the other's current cell.
        let (ci, cj) = (animals[i].cell(), animals[j].cell());
        animals[i].now.court_cell = Some(pack_cell(cj));
        animals[j].now.court_cell = Some(pack_cell(ci));
        let close = dist2(animals[i].snap.pos, animals[j].snap.pos) <= COURT_NEAR * COURT_NEAR;
        if !close {
            // Separated: courtship progress restarts (closeness must be
            // consecutive), the pairing holds.
            animals[i].now.court = None;
            continue;
        }
        let progress = animals[i].now.court.unwrap_or(0) + SWEEP_EVERY as i64;
        if progress < COURT_TICKS {
            animals[i].now.court = Some(progress);
            continue;
        }
        birth(content, animals, i, j, tick);
    }
}

/// Spawn the newborn JUVENILE between the parents and retire the pair's
/// love state. A failed spawn (no clear midpoint) keeps the courtship
/// complete and retries next sweep.
fn birth(content: &Content, animals: &mut [Animal], i: usize, j: usize, tick: u64) {
    let def = &content.husbandry[animals[i].def];
    let (pa, pb) = (animals[i].snap.pos, animals[j].snap.pos);
    let mid = [
        (pa[0] + pb[0]) * 0.5,
        pa[1].max(pb[1]),
        (pa[2] + pb[2]) * 0.5,
    ];
    let yaw = animals[i].snap.yaw;
    let newborn = [mid, pa, pb]
        .into_iter()
        .find_map(|pos| spawn_mob_checked(def.offspring_key, pos, yaw));
    let Some(newborn) = newborn else {
        animals[i].now.court = Some(COURT_TICKS);
        return;
    };
    // The baby phase is the maturity gate: the juvenile grows into the
    // breedable adult when this tag expires (see [`crate::growth`]).
    mob_tag_set(newborn, BABY, MobTagValue::I64((tick + BABY_TICKS) as i64));
    for k in [i, j] {
        animals[k].now.end_love();
        animals[k].now.breed_cool = Some((tick + BREED_COOLDOWN) as i64);
    }
}

// --- the AI node -----------------------------------------------------------

/// `farming:husbandry_goal`: steer toward the sweep's destination tags —
/// courting partner first, then the water trough, the feed trough, and the
/// grass target last — standing in place once close (which also keeps wander
/// from strolling the animal away). While a munch/sip is active it holds
/// position and plays the procedural feeding pose: head down at grass level,
/// bobbing between [`HEAD_DOWN`] and [`HEAD_RAISED`] (the engine eases the
/// head toward each target, so the square wave renders as a smooth chew).
/// Reads only the baseline ctx; all state belongs to the sweep.
pub fn decide(ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
    let consuming =
        tag_i64(&ctx.tags, CONSUME_UNTIL).is_some_and(|until| (ctx.tick as i64) < until);
    if consuming {
        let lowered = (ctx.tick / BOB_HALF_PERIOD) % 2 == 0;
        return Some(AiNodeDecision {
            goal: Some(ctx.cell),
            head_look: Some([0.0, if lowered { HEAD_DOWN } else { HEAD_RAISED }]),
            ..Default::default()
        });
    }
    let goal_toward = |packed: i64, hold: f32| {
        let cell = unpack_cell(packed);
        let dx = ctx.pos[0] - (cell[0] as f32 + 0.5);
        let dz = ctx.pos[2] - (cell[2] as f32 + 0.5);
        let close = dx * dx + dz * dz <= hold * hold;
        Some(AiNodeDecision {
            goal: Some(if close { ctx.cell } else { cell }),
            ..Default::default()
        })
    };
    if let Some(packed) = tag_i64(&ctx.tags, COURT_CELL) {
        return goal_toward(packed, COURT_NEAR * 0.8);
    }
    if let Some(packed) = tag_i64(&ctx.tags, DRINK_CELL) {
        return goal_toward(packed, DRINK_RANGE * 0.8);
    }
    if let Some(packed) = tag_i64(&ctx.tags, FEED_CELL) {
        return goal_toward(packed, DRINK_RANGE * 0.8);
    }
    if let Some(packed) = tag_i64(&ctx.tags, GRAZE_CELL) {
        return goal_toward(packed, EAT_RANGE * 0.7);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{pack_cell, unpack_cell};

    #[test]
    fn cell_packing_roundtrips_signed_coordinates() {
        for c in [
            [0, 0, 0],
            [1, -2, 3],
            [-33_000_000, 2047, 33_000_000],
            [12_345_678, -2048, -12_345_678],
        ] {
            assert_eq!(unpack_cell(pack_cell(c)), c);
        }
    }
}
