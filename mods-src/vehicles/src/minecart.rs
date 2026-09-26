//! Minecarts and rails: the pack's policy over three engine seams.
//!
//! - **Rails are box-shaped block rows** — one row per form
//!   (`vehicles:rail_ns`, `rail_curve_ne`, `rail_slope_w`, …, and the booster
//!   twins), each a single authored plane: flat at one texel for the level
//!   forms, tilted 45° about the cell centre for the slopes. A placed rail
//!   arrives as the item's straight row; the `block_placed` handler runs the
//!   connection rule ([`crate::rail`]) over the neighbourhood and swaps the
//!   placed cell — and any neighbour that turns to meet it — to the resolved
//!   rows (`swap_block`). Breaking a rail changes nothing around it.
//! - **The cart is a mob** with an empty brain, a single seat and a spawn
//!   tag that lets the tick enumerate every live cart (`mobs_with_tag`).
//!   Its whole state is on the mob: pose from the snapshot, signed speed in
//!   the `vehicles:cart_speed` tag (persisted with the mob, so a saved cart
//!   resumes its roll). The tick integrates one step of [`crate::cart`] and
//!   hands the engine the resulting pose through `mob_kinematic`; the
//!   engine presents it, seats the rider, blocks players and pushes sheep.
//! - **Off the rails** the cart is the engine's body: it flies off a track
//!   end with the speed it left at, lands by gravity, skids to a halt under
//!   `mob_drive`, and snaps back onto the first rail its feet come to rest
//!   on. A rider can push a derailed cart along the ground back to a rail.
//! - **The rolling sound** is one looping spatial play per moving cart,
//!   pinned to the mob (`sound_play_on_mob` on the pack's `loop` row) and
//!   retuned every tick its speed moves (`sound_set`): louder the faster it
//!   rolls, silent when parked, and it travels with the cart past whoever
//!   is standing by the track. The engine replays a live loop to a player
//!   who joins mid-ride and ends it with the mob; the pack only starts,
//!   retunes and stops.
//! - **Placing** is an `interact_attempt` on a rail while holding the cart
//!   item; **boarding** an `interact_attempt` on the cart; a **punch**
//!   (`mob_damage_pre`) shoves the cart away from the puncher along its rail
//!   and still counts toward breaking it (the row's loot drops the item).

use std::collections::{BTreeMap, BTreeSet};

use mod_sdk::*;

use crate::cart::{self, overlaps, Aabb, Body, Cart, ContactGrid, Controls, Step};
use crate::keys;
use crate::rail::{resolve_placement, Form, Rail, RailMap};
use crate::track::{dot2, xz, yaw_facing, Path, RAIL_TOP};

/// A block collision box in its own cell space.
type LocalBox = ([f32; 3], [f32; 3]);

/// Signed speed along the facing (m/s), persisted with the mob.
const SPEED_TAG: &str = "vehicles:cart_speed";
/// The wheels' looping clip in `minecart.bbmodel` and the authored wheel
/// diameter in blocks (4 px) it rolls at.
const ROLL_ANIM: &str = "roll";
const WHEEL_DIAMETER: f32 = 4.0 / 16.0;
/// Rail rows are read in a box this many cells around a cart or a placed
/// rail: two rings, so the connection rule sees a neighbour's own links and
/// a top-speed cart can chain through the cells it crosses in one tick.
const RAIL_REACH: i32 = 2;
/// Speed changes smaller than this are not written back to the tag.
const SPEED_EPS: f32 = 1e-3;
/// How far the rolling loop's volume must move before the live play is
/// retuned.
const ROLL_VOLUME_EPS: f32 = 0.02;

/// One batched read of the cells in a cube around a centre.
struct CellBox {
    min: [i32; 3],
    side: i32,
    blocks: Vec<Option<BlockId>>,
}

impl CellBox {
    fn around(center: [i32; 3], reach: i32) -> Self {
        Self::around_each(&[center], reach)
            .pop()
            .expect("one box per centre")
    }

    /// One box around each of `centers`, from ONE batched read of all their
    /// cells (split only at the host's per-call cap) — a rail network costs
    /// a call per few dozen carts per tick, not a call per cart.
    fn around_each(centers: &[[i32; 3]], reach: i32) -> Vec<Self> {
        let side = 2 * reach + 1;
        let per_box = (side * side * side) as usize;
        let mins: Vec<[i32; 3]> = centers
            .iter()
            .map(|c| [c[0] - reach, c[1] - reach, c[2] - reach])
            .collect();
        let mut positions = Vec::with_capacity(per_box * centers.len());
        for min in &mins {
            for y in 0..side {
                for z in 0..side {
                    for x in 0..side {
                        positions.push([min[0] + x, min[1] + y, min[2] + z]);
                    }
                }
            }
        }
        let mut blocks = paged(positions, get_blocks).into_iter();
        mins.into_iter()
            .map(|min| CellBox {
                min,
                side,
                blocks: blocks.by_ref().take(per_box).collect(),
            })
            .collect()
    }

    fn block(&self, cell: [i32; 3]) -> Option<BlockId> {
        let l = [
            cell[0] - self.min[0],
            cell[1] - self.min[1],
            cell[2] - self.min[2],
        ];
        if l.iter().any(|&c| c < 0 || c >= self.side) {
            return None;
        }
        self.blocks[((l[1] * self.side + l[2]) * self.side + l[0]) as usize]
    }
}

/// A cell box answering the pure rail rules.
struct RailBox<'a> {
    table: &'a BTreeMap<u16, Rail>,
    cells: &'a CellBox,
}

impl RailMap for RailBox<'_> {
    fn rail(&self, cell: [i32; 3]) -> Option<Rail> {
        self.cells
            .block(cell)
            .and_then(|b| self.table.get(&b.0).copied())
    }
}

#[derive(Default)]
pub struct Minecarts {
    cart_item: Option<ItemId>,
    cart_kind: Option<MobId>,
    /// Every rail row (by block id) → its form, and the reverse for the swaps.
    rails: BTreeMap<u16, Rail>,
    rows: BTreeMap<(Form, bool), BlockId>,
    /// Carts whose wheel clip has been activated, and the rate it plays at.
    /// Transient: a reloaded cart's wheels start again on its first tick.
    rolling: BTreeMap<u64, f32>,
    /// Carts whose rolling loop is playing: the session sound handle and
    /// the volume it was last set to. Transient like `rolling`; a cart that
    /// leaves the live list (broken, unloaded) has its loop stopped.
    humming: BTreeMap<u64, (u64, f32)>,
    /// A block id's cell-local collision boxes — the registry's answer,
    /// cached per id, so the wall test costs no host call once a block has
    /// been seen. Empty for anything a body passes through (air, plants, a
    /// rail).
    collision: BTreeMap<u16, Vec<LocalBox>>,
}

impl Minecarts {
    pub fn init(&mut self) {
        self.cart_item = resolve_item_logged(keys::MINECART_ITEM);
        self.cart_kind = resolve_mob_logged(keys::MINECART_MOB);
        for (form, booster, name) in keys::RAIL_ROWS {
            if let Some(id) = resolve_block_logged(name) {
                self.rails.insert(id.0, Rail { form, booster });
                self.rows.insert((form, booster), id);
            }
        }
        log(&format!("{} rail rows", self.rails.len()));
    }

    fn is_cart(&self, kind: MobId) -> bool {
        Some(kind) == self.cart_kind
    }

    fn row(&self, form: Form, booster: bool) -> Option<BlockId> {
        self.rows.get(&(form, booster)).copied()
    }

    /// A rail was placed by a player: resolve its form from the neighbours
    /// and swap every row the connection rule changed.
    pub fn on_block_placed(&mut self, pos: [i32; 3], block: BlockId) {
        let Some(placed) = self.rails.get(&block.0).copied() else {
            return;
        };
        let cells = CellBox::around(pos, RAIL_REACH);
        let map = RailBox {
            table: &self.rails,
            cells: &cells,
        };
        let res = resolve_placement(&map, pos, placed);
        let mut swaps: Vec<([i32; 3], Form, bool)> = vec![(pos, res.form, placed.booster)];
        for (cell, form) in &res.turns {
            if let Some(rail) = map.rail(*cell) {
                swaps.push((*cell, *form, rail.booster));
            }
        }
        for (cell, form, booster) in swaps {
            if let Some(row) = self.row(form, booster) {
                swap_block(cell, row);
            }
        }
    }

    /// A use click on a live mob: a cart seats the player in its free seat.
    /// Act-based claim: nothing seated consumes nothing.
    pub fn on_interact_mob(&self, mob_id: u64, kind: MobId, player: PlayerId) -> Outcome {
        if self.is_cart(kind) && self.board(mob_id, player) {
            Outcome::Cancel
        } else {
            Outcome::Continue
        }
    }

    /// A use click on a block: on a rail while holding a cart, place one
    /// there facing away from the player. Act-based claim: nothing spawned
    /// consumes nothing.
    pub fn on_interact_block(&self, block: Option<[i32; 3]>) -> Outcome {
        let (Some(cell), Some(item)) = (block, self.cart_item) else {
            return Outcome::Continue;
        };
        let actor = player_state();
        if actor.held != Some(item) {
            return Outcome::Continue;
        }
        let Some(rail) = get_block(cell).and_then(|b| self.rails.get(&b.0).copied()) else {
            return Outcome::Continue;
        };
        // Mid-rail, nose along the track and away from the player.
        let path = Path::of(rail.form);
        let mid = path.len() * 0.5;
        let p = path.point(mid);
        let t = xz(path.tangent(mid));
        let away = if dot2(t, player_facing_xz(actor.yaw)) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        let yaw = yaw_facing([t[0] * away, t[1] * away]);
        let pos = [
            cell[0] as f64 + f64::from(p[0]),
            cell[1] as f64 + f64::from(p[1] + RAIL_TOP),
            cell[2] as f64 + f64::from(p[2]),
        ];
        if !consume_held(item, 1) {
            return Outcome::Continue;
        }
        if spawn_mob_checked(keys::MINECART_MOB, pos, yaw).is_none() {
            give_item(keys::MINECART_ITEM, 1);
            return Outcome::Continue;
        }
        Outcome::Cancel
    }

    fn board(&self, mob_id: u64, player: PlayerId) -> bool {
        let Some(seat) = mob_riders(mob_id).and_then(|r| r.first_free_seat()) else {
            return false;
        };
        mob_mount(mob_id, player, seat)
    }

    /// A hit on a cart shoves it away from the puncher along its rail; the
    /// damage itself still lands (six punches break a cart into its item).
    pub fn on_mob_damage(&mut self, mob_id: u64, kind: MobId, origin: Option<[f64; 3]>) {
        if !self.is_cart(kind) {
            return;
        }
        let (Some(origin), Some(m)) = (origin, mob_info(mob_id)) else {
            return;
        };
        let state = Cart {
            pos: m.pos,
            yaw: m.yaw,
            pitch: m.pitch,
            speed: read_speed(mob_id),
        };
        write_speed(mob_id, state.speed + cart::punch(&state, origin));
    }

    /// One step for every live cart.
    pub fn tick(&mut self) {
        let carts = mobs_with_tag(keys::CART_TAG, None);
        // Every cart's pre-tick state, so a contact resolves the same from
        // both sides whichever cart steps first.
        let before: Vec<Cart> = carts
            .iter()
            .map(|m| Cart {
                pos: m.pos,
                yaw: m.yaw,
                pitch: m.pitch,
                speed: read_speed(m.id),
            })
            .collect();
        let contacts = ContactGrid::new(&before);
        // Every cart's rail box in one batched read: stepping moves bodies,
        // never blocks, so the whole tick reads the same terrain.
        let centers: Vec<[i32; 3]> = carts.iter().map(|m| cell_of(m.pos)).collect();
        let boxes = CellBox::around_each(&centers, RAIL_REACH);
        let mut seen = BTreeSet::new();
        for (i, ((m, start), cells)) in carts.iter().zip(&before).zip(&boxes).enumerate() {
            seen.insert(m.id);
            let mut state = *start;
            let others = contacts.neighbours(&before, i);
            state.speed = cart::resolve_contacts(start, others.iter().map(|&j| &before[j]));
            let push = rider_push(m.id, state.yaw);
            learn_collision(&mut self.collision, cells);
            let blocked = |probe: Aabb| blocked_by(&self.collision, cells, probe);
            let body = Body {
                half_width: m.half_width,
                height: m.height,
            };
            let map = RailBox {
                table: &self.rails,
                cells,
            };
            match cart::step(&map, state, body, Controls { push }, &blocked) {
                Step::Railed(next) | Step::Derailed(next) => {
                    // A cart never banks: roll stays level on every rail.
                    if !mob_kinematic(m.id, next.pos, next.yaw, next.pitch, 0.0) {
                        continue;
                    }
                    // Against the PERSISTED speed: a contact may have
                    // rewritten `state` before the step.
                    if (next.speed - start.speed).abs() > SPEED_EPS {
                        write_speed(m.id, next.speed);
                    }
                    self.present(m.id, next.speed);
                }
                Step::Off => {
                    // The engine's body: airborne it flies; on the ground it
                    // skids out under its wheels, or a rider walks it along.
                    if !m.on_ground {
                        self.present(m.id, 0.0);
                        continue;
                    }
                    let mut v =
                        state.speed * cart::SKID_RETENTION + push * cart::RIDER_ACCEL * cart::DT;
                    if v.abs() < cart::STOP_SPEED {
                        v = 0.0;
                    }
                    if v != 0.0 {
                        let f = cart::facing_xz(state.yaw);
                        mob_drive(m.id, [f[0] * v, f[1] * v], None);
                    }
                    if (v - start.speed).abs() > SPEED_EPS {
                        write_speed(m.id, v);
                    }
                    self.present(m.id, v);
                }
            }
        }
        self.rolling.retain(|id, _| seen.contains(id));
        self.humming.retain(|id, &mut (handle, _)| {
            let live = seen.contains(id);
            if !live {
                sound_stop(handle);
            }
            live
        });
    }

    /// What a cart at `speed` looks and sounds like this tick: the wheels
    /// turn at it, and the rolling loop follows it.
    fn present(&mut self, id: u64, speed: f32) {
        self.roll(id, speed);
        self.hum(id, speed);
    }

    /// Keep the rolling loop at the cart's speed: start it when the cart
    /// gets going, retune it as the speed moves, stop it when the cart
    /// parks.
    fn hum(&mut self, id: u64, speed: f32) {
        let volume = cart::roll_volume(speed);
        match self.humming.get(&id).copied() {
            None if volume > 0.0 => {
                let handle = sound_play_on_mob(id, keys::ROLL_SOUND, volume, 1.0);
                if handle != 0 {
                    self.humming.insert(id, (handle, volume));
                }
            }
            Some((handle, _)) if volume <= 0.0 => {
                sound_stop(handle);
                self.humming.remove(&id);
            }
            Some((handle, current)) if (current - volume).abs() > ROLL_VOLUME_EPS => {
                sound_set(handle, volume, 1.0);
                self.humming.insert(id, (handle, volume));
            }
            _ => {}
        }
    }

    /// Keep the wheels turning at the cart's speed: activate the clip once,
    /// then retune its rate only when the speed moved.
    fn roll(&mut self, id: u64, speed: f32) {
        let rate = cart::wheel_roll_rate(speed, WHEEL_DIAMETER);
        match self.rolling.get(&id) {
            None => {
                if mob_anim_set(id, ROLL_ANIM, true) && mob_anim_rate(id, ROLL_ANIM, rate) {
                    self.rolling.insert(id, rate);
                }
            }
            Some(&current) if (current - rate).abs() > 0.02 => {
                if mob_anim_rate(id, ROLL_ANIM, rate) {
                    self.rolling.insert(id, rate);
                }
            }
            Some(_) => {}
        }
    }
}

/// Ask the registry, once per block id, for the collision of every block the
/// batched read holds — so the closures over the read never cross the ABI.
fn learn_collision(collision: &mut BTreeMap<u16, Vec<LocalBox>>, cells: &CellBox) {
    for block in cells.blocks.iter().flatten() {
        if block.0 == BlockId::AIR.0 || collision.contains_key(&block.0) {
            continue;
        }
        let boxes = block_info(*block).map_or(Vec::new(), |i| i.collision);
        collision.insert(block.0, boxes);
    }
}

/// Whether any terrain collision overlaps the world-space `probe` — from
/// the batched read and the learnt per-id boxes; unloaded and unknown cells
/// read as open.
fn blocked_by(collision: &BTreeMap<u16, Vec<LocalBox>>, cells: &CellBox, probe: Aabb) -> bool {
    let (lo, hi) = probe;
    let span = |i: usize| (lo[i].floor() as i32)..=((hi[i] - 1e-4).floor() as i32);
    span(1).any(|y| {
        span(2).any(|z| {
            span(0).any(|x| {
                let cell = [x, y, z];
                cells
                    .block(cell)
                    .and_then(|b| collision.get(&b.0))
                    .is_some_and(|boxes| {
                        boxes.iter().any(|(mn, mx)| {
                            let at = |c: [f32; 3]| {
                                [
                                    f64::from(c[0]) + x as f64,
                                    f64::from(c[1]) + y as f64,
                                    f64::from(c[2]) + z as f64,
                                ]
                            };
                            overlaps(probe, (at(*mn), at(*mx)))
                        })
                    })
            })
        })
    })
}

/// The driver's push along the cart's facing: forward input pushes the cart
/// the way the rider is LOOKING, so a rider facing backwards over the tail
/// still drives toward what they see.
fn rider_push(mob_id: u64, cart_yaw: f32) -> f32 {
    let Some(riders) = mob_riders(mob_id) else {
        return 0.0;
    };
    let Some(driver) = riders.riders.iter().min_by_key(|r| r.seat) else {
        return 0.0;
    };
    let Some(input) = player_input(driver.player_id) else {
        return 0.0;
    };
    let look = player_facing_xz(input.yaw);
    let facing = cart::facing_xz(cart_yaw);
    let toward_look = if dot2(look, facing) >= 0.0 { 1.0 } else { -1.0 };
    (input.forward * toward_look).clamp(-1.0, 1.0)
}

fn read_speed(mob_id: u64) -> f32 {
    match mob_tag_get(mob_id, SPEED_TAG) {
        MobTagLookup::Value(MobTagValue::F64(v)) if v.is_finite() => v as f32,
        _ => 0.0,
    }
}

fn write_speed(mob_id: u64, speed: f32) {
    mob_tag_set(mob_id, SPEED_TAG, MobTagValue::F64(speed as f64));
}
