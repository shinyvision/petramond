//! Husbandry: grazing, drinking, love mode, courtship and offspring.
//!
//! Breeding should be earned with a well-kept pasture, not by feeding one item. The wild population
//! stays a finite harvest (see the engine's spawn design). Species opt in as [`Content::husbandry`]
//! rows; nothing here special-cases a species.
//!
//! The sim sweep ([`on_tick`], after the Mobs stage) owns the whole state machine: the saturation
//! trickle, the heartbeat rolls for grass-seeking and love, eating, pairing two mobs in love, the
//! courtship timer and spawning the newborn. It runs every [`SWEEP_EVERY`] ticks over animals near
//! players and only touches the world through host calls.
//!
//! The AI node (`farming:husbandry_goal`, [`decide`]) only steers and poses. It reads the
//! destination tags the sweep left and heads for the partner, then the trough, then grass, standing
//! still when close. While munching or sipping it holds position and bobs its head down with a
//! procedural `head_look`. It never rolls chances or writes state.
//!
//! Eating and drinking take [`CONSUME_TICKS`] of head-down munching before anything happens, and
//! every [`TROUGH_SIPS`] sips empty the water trough. A wheat-packed trough is the feed store.
//! Hungry animals always go there before grazing ([`FEED_CELL`]; only thirst comes first). Each
//! meal restores the species' `restore` saturation, and the [`TROUGH_MEALS`]th bite empties it.
//! Love mode still only looks at the filled water trough.
//!
//! Per-animal state all lives in the mob's tags, so it persists, travels and dies with the animal.
//! That's `farming:saturation` (missing means full), two timer deadlines, the packed destination
//! cells, `farming:love_until`, `farming:partner` and `farming:breed_cool`; courtship progress sits
//! on the lower-id partner. Partner links are session mob ids, so after a reload they just re-pair.

use std::collections::HashMap;

use mod_sdk::*;

use crate::content::{Content, Eaten, HusbandryDef};

const SWEEP_EVERY: u64 = 5;
const RANGE: f32 = 64.0;

const SAT_MAX: i64 = 10;
const TRICKLE_MIN: u64 = 200;
const TRICKLE_SPAN: u64 = 1001;
const HEARTBEAT: u64 = 600;
const SEEK_RADIUS: i32 = 8;
const TROUGH_RADIUS: i32 = 8;
const LOVE_SAT: i64 = 7;
const LOVE_CHANCE_IN: u64 = 4;
const LOVE_TICKS: u64 = 2400;
const PAIR_RANGE: f32 = 24.0;
const COURT_NEAR: f32 = 2.5;
const COURT_TICKS: i64 = 100;
const BREED_COOLDOWN: u64 = 6000;
const BABY_TICKS: u64 = 12_000;
const EAT_RANGE: f32 = 1.4;
const CONSUME_TICKS: u64 = 40;
const DRINK_MIN: u64 = 1200;
const DRINK_SPAN: u64 = 4801;
const DRINK_RETRY: u64 = 600;
const DRINK_RANGE: f32 = 1.75;
const TROUGH_SIPS: u8 = 5;
pub const TROUGH_MEALS: u8 = 12;
pub const MEALS_PER_WHEAT: u8 = 4;

pub fn wheat_yield(meals: u8) -> u8 {
    TROUGH_MEALS.saturating_sub(meals) / MEALS_PER_WHEAT
}

const EAT_ANIM: &str = "eat";

const HEAD_DOWN: f32 = -0.9;
const HEAD_RAISED: f32 = -0.45;
const BOB_HALF_PERIOD: u64 = 8;

const FACE_TOL: f32 = 0.5;
const TURN_STEP: f32 = 1.0;

const SATURATION: &str = "farming:saturation";
const SAT_NEXT: &str = "farming:sat_next";
const HEART_NEXT: &str = "farming:husbandry_next";
const GRAZE_CELL: &str = "farming:graze_cell";
const FEED_CELL: &str = "farming:feed_cell";
const DRINK_NEXT: &str = "farming:drink_next";
const DRINK_CELL: &str = "farming:drink_cell";
const CONSUME_UNTIL: &str = "farming:consume_until";
const LOVE_UNTIL: &str = "farming:love_until";
const PARTNER: &str = "farming:partner";
const COURT_CELL: &str = "farming:court_cell";
const COURT: &str = "farming:court_ticks";
const BREED_COOL: &str = "farming:breed_cool";

const SIPS_KEY: &str = "farming:sips";
const MEALS_KEY: &str = "farming:meals";

pub const BABY: &str = "farming:baby";

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

fn tag_i64(tags: &[(String, MobTagValue)], key: &str) -> Option<i64> {
    tags.iter().find_map(|(k, v)| match v {
        MobTagValue::I64(i) if k == key => Some(*i),
        _ => None,
    })
}

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
            sat: tag_i64(tags, SATURATION)
                .unwrap_or(SAT_MAX)
                .clamp(0, SAT_MAX),
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

const REACH_PROBES: usize = 5;

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

fn can_stand_by(a: &Animal, p: [i32; 3]) -> bool {
    [[0, 0], [1, 0], [-1, 0], [0, 1], [0, -1]]
        .iter()
        .any(|d| mob_can_reach(a.snap.id, [p[0] + d[0], p[1], p[2] + d[1]]))
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f32 {
    let d = [
        (a[0] - b[0]) as f32,
        (a[1] - b[1]) as f32,
        (a[2] - b[2]) as f32,
    ];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

pub fn on_tick(content: &Content) {
    let tick = current_tick();
    if !tick.is_multiple_of(SWEEP_EVERY) {
        return;
    }
    let anchors: Vec<[f64; 3]> = players().iter().map(|p| p.state.pos).collect();
    let mut animals: Vec<Animal> = Vec::new();
    for snap in mobs_near_any(&anchors, RANGE) {
        let Some(def) = content.husbandry.iter().position(|d| d.kind == snap.kind) else {
            continue;
        };
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
    for a in &mut animals {
        step_animal(content, &content.husbandry[a.def], a, tick);
    }
    court(content, &mut animals, tick);
    for a in &animals {
        a.now.commit(&a.was, a.snap.id);
    }
    for baby in mobs_with_tag(BABY, None) {
        if let MobTagLookup::Value(MobTagValue::I64(until)) = mob_tag_get(baby.id, BABY) {
            if tick as i64 >= until {
                mob_tag_delete(baby.id, BABY);
            }
        }
    }
}

fn step_animal(content: &Content, def: &HusbandryDef, a: &mut Animal, tick: u64) {
    let s = &mut a.now;
    match s.sat_next {
        None => {
            s.sat_next =
                Some((tick + TRICKLE_MIN + rng_u64("husbandry_trickle") % TRICKLE_SPAN) as i64)
        }
        Some(due) if tick as i64 >= due => {
            s.sat = (s.sat - 1).max(0);
            s.sat_next =
                Some((tick + TRICKLE_MIN + rng_u64("husbandry_trickle") % TRICKLE_SPAN) as i64);
        }
        Some(_) => {}
    }
    if s.love_until.is_some() && !s.in_love(tick) {
        s.end_love();
    }
    if s.breed_cool.is_some_and(|until| tick as i64 >= until) {
        s.breed_cool = None;
    }
    let due = match s.heart_next {
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
                if def.kept() {
                    roll_love(content, a, tick);
                }
                roll_graze(content, def, a);
            }
        }
    }
    if def.kept() {
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
    }
    consume_step(content, def, a, tick);
}

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
            a.now.graze = None;
            a.now.feed_cell = None;
        }
        None => a.now.drink_next = Some((tick + DRINK_RETRY) as i64),
    }
}

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
    if troughs.is_some_and(|t| !t.is_empty()) {
        a.now.love_until = Some((tick + LOVE_TICKS) as i64);
        a.now.graze = None;
    }
}

fn roll_graze(content: &Content, def: &HusbandryDef, a: &mut Animal) {
    if a.now.sat >= SAT_MAX || a.now.graze.is_some() || a.now.feed_cell.is_some() {
        return;
    }
    if (rng_u64("husbandry_graze") % 10) as i64 >= SAT_MAX - a.now.sat {
        return;
    }
    let c = a.cell();
    let r = SEEK_RADIUS;
    let box_min = [c[0] - r, c[1] - r, c[2] - r];
    let box_max = [c[0] + r, c[1] + r, c[2] + r];
    if def.kept() {
        let Some(troughs) = find_blocks(box_min, box_max, vec![content.trough_wheat]) else {
            return;
        };
        if let Some(p) = nearest_reachable(a, troughs) {
            a.now.feed_cell = Some(pack_cell(p));
            return;
        }
    }
    for group in &def.food {
        let Some(found) = find_blocks(box_min, box_max, group.blocks.clone()) else {
            return;
        };
        if let Some(p) = nearest_reachable(a, found) {
            a.now.graze = Some(pack_cell(p));
            return;
        }
    }
}

fn near_cell(a: &Animal, cell: [i32; 3], range: f32, dy_tol: f32) -> bool {
    let dx = (a.snap.pos[0] - (cell[0] as f64 + 0.5)) as f32;
    let dz = (a.snap.pos[2] - (cell[2] as f64 + 0.5)) as f32;
    let dy = (a.snap.pos[1] - cell[1] as f64) as f32;
    dx * dx + dz * dz <= range * range && dy.abs() <= dy_tol
}

fn wrap_angle(v: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (v + PI).rem_euclid(TAU) - PI
}

fn facing_or_turn(a: &Animal, cell: [i32; 3]) -> bool {
    let dx = ((cell[0] as f64 + 0.5) - a.snap.pos[0]) as f32;
    let dz = ((cell[2] as f64 + 0.5) - a.snap.pos[2]) as f32;
    if dx * dx + dz * dz < 0.36 {
        return true;
    }
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

fn standing_on_trough(content: &Content, a: &Animal) -> bool {
    let feet = a.cell();
    let below = [feet[0], feet[1] - 1, feet[2]];
    [feet, below]
        .into_iter()
        .any(|c| get_block(c).is_some_and(|b| is_any_trough(content, b)))
}

fn is_any_trough(content: &Content, b: BlockId) -> bool {
    b == content.trough || b == content.trough_filled || b == content.trough_wheat
}

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
            let plant = match get_block(cell) {
                None => return,
                Some(b) if def.food_group(b).is_some() => b,
                Some(_) => {
                    a.now.graze = None;
                    a.now.consume_until = None;
                    return;
                }
            };
            if done && finish_graze(content, def, cell, plant) {
                a.now.sat = (a.now.sat + def.restore).min(SAT_MAX);
                a.now.graze = None;
                a.now.consume_until = None;
            }
        } else {
            a.now.consume_until = None;
        }
        return;
    }
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
            Some(b) if def.food_group(b).is_some() => {
                if near_cell(a, cell, EAT_RANGE, 2.0) && facing_or_turn(a, cell) {
                    start(a);
                }
            }
            Some(_) => a.now.graze = None,
        }
    }
}

fn finish_graze(content: &Content, def: &HusbandryDef, cell: [i32; 3], plant: BlockId) -> bool {
    match def.food_group(plant).map(|g| g.eaten) {
        Some(Eaten::Clear) => set_block(cell, BlockId::AIR),
        Some(Eaten::Regress) => match content.crop_regressed(plant) {
            Some(below) => set_block(cell, below),
            None => true,
        },
        Some(Eaten::Keep) => true,
        None => false,
    }
}

fn finish_drink(content: &Content, a: &mut Animal, cell: [i32; 3], tick: u64) {
    let sips = crate::kv_counter::kv_counter_bump(cell, SIPS_KEY);
    if sips >= TROUGH_SIPS {
        swap_block(cell, content.trough);
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

fn finish_feed(content: &Content, def: &HusbandryDef, a: &mut Animal, cell: [i32; 3]) {
    let meals = crate::kv_counter::kv_counter_bump(cell, MEALS_KEY);
    if meals >= TROUGH_MEALS {
        swap_block(cell, content.trough);
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

pub fn clear_sips(content: &Content, pos: [i32; 3]) {
    for member in trough_members(content, pos) {
        section_kv_delete(member, SIPS_KEY);
    }
}

pub fn meals_at(pos: [i32; 3]) -> u8 {
    section_kv_get(pos, MEALS_KEY)
        .and_then(|b| b.first().copied())
        .unwrap_or(0)
}

pub fn clear_meals(content: &Content, pos: [i32; 3]) {
    for member in trough_members(content, pos) {
        section_kv_delete(member, MEALS_KEY);
    }
}

fn court(content: &Content, animals: &mut [Animal], tick: u64) {
    let by_id: HashMap<i64, usize> = animals
        .iter()
        .enumerate()
        .map(|(i, a)| (a.snap.id as i64, i))
        .collect();
    let lover = |a: &Animal| a.now.in_love(tick);
    let lovers: Vec<usize> = (0..animals.len()).filter(|&i| lover(&animals[i])).collect();
    for i in 0..animals.len() {
        let Some(pid) = animals[i].now.partner else {
            continue;
        };
        let ok = lover(&animals[i])
            && by_id.get(&pid).is_some_and(|&j| {
                let b = &animals[j];
                lover(b)
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
    for &i in &lovers {
        if animals[i].now.partner.is_some() {
            continue;
        }
        let mut near: Vec<(usize, f32)> = lovers
            .iter()
            .copied()
            .filter(|&j| j != i)
            .filter(|&j| animals[j].def == animals[i].def)
            .filter(|&j| animals[j].now.partner.is_none())
            .map(|j| (j, dist2(animals[i].snap.pos, animals[j].snap.pos)))
            .filter(|&(_, d2)| d2 <= PAIR_RANGE * PAIR_RANGE)
            .collect();
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        let near = near
            .into_iter()
            .take(REACH_PROBES)
            .find(|&(j, _)| can_stand_by(&animals[i], animals[j].cell()));
        if let Some((j, _)) = near {
            animals[i].now.partner = Some(animals[j].snap.id as i64);
            animals[j].now.partner = Some(animals[i].snap.id as i64);
        }
    }
    for i in 0..animals.len() {
        let Some(pid) = animals[i].now.partner else {
            continue;
        };
        let Some(&j) = by_id.get(&pid) else {
            continue;
        };
        if j <= i {
            continue;
        }
        let (ci, cj) = (animals[i].cell(), animals[j].cell());
        animals[i].now.court_cell = Some(pack_cell(cj));
        animals[j].now.court_cell = Some(pack_cell(ci));
        let close = dist2(animals[i].snap.pos, animals[j].snap.pos) <= COURT_NEAR * COURT_NEAR;
        if !close {
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

fn birth(content: &Content, animals: &mut [Animal], i: usize, j: usize, tick: u64) {
    let def = &content.husbandry[animals[i].def];
    let Some((offspring_key, _)) = &def.offspring else {
        return;
    };
    let (pa, pb) = (animals[i].snap.pos, animals[j].snap.pos);
    let mid = [
        (pa[0] + pb[0]) * 0.5,
        pa[1].max(pb[1]),
        (pa[2] + pb[2]) * 0.5,
    ];
    let yaw = animals[i].snap.yaw;
    let newborn = [mid, pa, pb]
        .into_iter()
        .find_map(|pos| spawn_mob_checked(offspring_key, pos, yaw));
    let Some(newborn) = newborn else {
        animals[i].now.court = Some(COURT_TICKS);
        return;
    };
    mob_tag_set(newborn, BABY, MobTagValue::I64((tick + BABY_TICKS) as i64));
    for k in [i, j] {
        animals[k].now.end_love();
        animals[k].now.breed_cool = Some((tick + BREED_COOLDOWN) as i64);
    }
}

pub fn decide(ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
    let consuming =
        tag_i64(&ctx.tags, CONSUME_UNTIL).is_some_and(|until| (ctx.tick as i64) < until);
    if consuming {
        let lowered = (ctx.tick / BOB_HALF_PERIOD).is_multiple_of(2);
        return Some(AiNodeDecision {
            goal: Some(ctx.cell),
            head_look: Some([0.0, if lowered { HEAD_DOWN } else { HEAD_RAISED }]),
            ..Default::default()
        });
    }
    let goal_toward = |packed: i64, hold: f32| {
        let cell = unpack_cell(packed);
        let dx = (ctx.pos[0] - (cell[0] as f64 + 0.5)) as f32;
        let dz = (ctx.pos[2] - (cell[2] as f64 + 0.5)) as f32;
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
