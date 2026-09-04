//! Minecart motion: one tick of a cart constrained to the rails.
//!
//! A cart's state is its pose plus one SIGNED speed along its own facing
//! (negative = rolling backwards — a cart never turns around, it rolls back
//! the way it came). Each tick the cart is projected onto the rail under it,
//! the forces act on the speed measured along the path (grade, booster,
//! rider push, drag), and the cart advances that far along the path, crossing
//! into whichever cell the rail's exit links to. The pose it ends at is what
//! the mod hands the engine as the tick's kinematic placement.
//!
//! Everything here is pure over a [`RailMap`] and an obstacle probe (does
//! any collision box overlap a world-space box), so the rules are testable
//! without a world; the mod supplies both from batched block reads and the
//! registry's per-block collision.

use crate::rail::{add, link, Dir, Rail, RailMap};
use crate::track::{dot2, grade, xz, yaw_facing, Path, RAIL_TOP};

/// A world-space axis-aligned box as `(min, max)` corners.
pub type Aabb = ([f32; 3], [f32; 3]);

/// Whether two boxes overlap with positive volume.
pub fn overlaps(a: Aabb, b: Aabb) -> bool {
    (0..3).all(|i| a.0[i] < b.1[i] && b.0[i] < a.1[i])
}

/// The whole box of a cell.
pub fn cell_box(cell: [i32; 3]) -> Aabb {
    let min = [cell[0] as f32, cell[1] as f32, cell[2] as f32];
    (min, [min[0] + 1.0, min[1] + 1.0, min[2] + 1.0])
}

/// The engine's fixed tick.
pub const DT: f32 = 1.0 / 20.0;

/// Top speed on rails (m/s) — twelve blocks a second, half again the classic
/// cart pace (Rachel's call after the first ride).
pub const MAX_SPEED: f32 = 12.0;
/// Acceleration along the track on a slope (m/s²): a gentle roll, not free
/// fall — a cart released at the top of a ten-block drop reaches top speed
/// near the bottom.
pub const SLOPE_ACCEL: f32 = 3.5;
/// Per-tick retention of speed on plain rails: nearly frictionless, so a
/// cart coasts a long way.
pub const DRAG_PER_TICK: f32 = 0.997;
/// Rolling resistance on plain rails (m/s²) — what finally parks a coasting
/// cart instead of leaving it creeping forever.
pub const ROLL_RESIST: f32 = 0.25;
/// Below this a cart with nothing pushing it on level track is parked.
pub const STOP_SPEED: f32 = 0.05;
/// Booster rail acceleration along the direction of travel (m/s²).
pub const BOOST_ACCEL: f32 = 14.0;
/// A cart standing on a booster launches at this speed: away from a solid
/// block at one end of a level booster, uphill on a sloped one.
pub const BOOST_LAUNCH: f32 = 1.5;
/// A booster considers a cart below this speed to be standing.
pub const BOOST_IDLE: f32 = 0.1;
/// A rider's push (m/s²) and the fastest a rider alone can drive a cart —
/// enough to start off and creep along the flat, well short of what
/// boosters and gravity give.
pub const RIDER_ACCEL: f32 = 3.0;
pub const RIDER_MAX: f32 = 4.5;
/// How fast the body's pitch follows the track's grade (rad/s): the nose
/// dips into a slope over a few frames instead of snapping.
pub const PITCH_RATE: f32 = 9.0;
/// Carts closer than this along the ground shove each other apart.
pub const CART_LENGTH: f32 = 1.0;
/// Shove strength between overlapping carts: speed per second per block of
/// overlap, applied along each cart's own facing — a moving cart hands its
/// momentum to a parked one over a few ticks instead of passing through it.
pub const NUDGE_ACCEL: f32 = 12.0;
/// Speed a punch gives a cart (m/s), away from the puncher.
pub const PUNCH_SPEED: f32 = 2.5;
/// Ground friction per tick for a derailed cart skidding on its wheels.
pub const SKID_RETENTION: f32 = 0.85;
/// Furthest a single tick may carry a cart along the rails, in cells: a
/// bound on the cell chain, well above what `MAX_SPEED` needs.
const MAX_CELLS_PER_TICK: usize = 6;

/// A cart's authoritative state: feet pose plus its signed speed.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Cart {
    pub pos: [f32; 3],
    /// Mob convention: yaw 0 faces `-Z`.
    pub yaw: f32,
    /// Body tilt, positive nose-up.
    pub pitch: f32,
    /// Speed along the facing (m/s); negative rolls backwards.
    pub speed: f32,
}

/// What acts on the cart this tick besides the track.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Controls {
    /// The rider's push along the cart's facing, `-1..1`.
    pub push: f32,
}

/// The cart's body, as the engine's row declares it: what the wall test
/// sweeps against the terrain's collision.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Body {
    pub half_width: f32,
    pub height: f32,
}

impl Body {
    /// The box of the body's UPPER half at `pos`: what a wall must meet. The
    /// upper half, because on a slope the cart leans into the hill: its
    /// lower front corner is legitimately inside the block that carries the
    /// next rail, while anything meeting the body above its axle is a wall.
    pub fn upper_half(self, pos: [f32; 3]) -> Aabb {
        (
            [
                pos[0] - self.half_width,
                pos[1] + self.height * 0.5,
                pos[2] - self.half_width,
            ],
            [
                pos[0] + self.half_width,
                pos[1] + self.height,
                pos[2] + self.half_width,
            ],
        )
    }
}

/// One tick's outcome.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// Still on the rails, at this pose.
    Railed(Cart),
    /// Ran off the end of the track this tick: the pose to hand the engine
    /// (carried on past the end at rail speed, so the implied velocity is the
    /// cart's) before it takes over with its own physics.
    Derailed(Cart),
    /// No rail under the cart — it is the engine's body until it lands on one.
    Off,
}

/// The facing direction of a mob yaw in the horizontal plane.
pub fn facing_xz(yaw: f32) -> [f32; 2] {
    let (s, c) = yaw.sin_cos();
    [-s, -c]
}

/// The rail cell a cart at `pos` rides: the cell its feet are in, or the one
/// below — a cart topping a slope is a hair into the cell above, and a
/// derailed cart standing on the block under a rail is in the rail's cell.
pub fn rail_cell(map: &impl RailMap, pos: [f32; 3]) -> Option<([i32; 3], Rail)> {
    let x = pos[0].floor() as i32;
    let z = pos[2].floor() as i32;
    let y = (pos[1] + 0.02).floor() as i32;
    [y, y - 1]
        .into_iter()
        .find_map(|cy| map.rail([x, cy, z]).map(|r| ([x, cy, z], r)))
}

fn approach(from: f32, to: f32, max_step: f32) -> f32 {
    from + (to - from).clamp(-max_step, max_step)
}

/// Advance `cart` one tick along the rails. `blocked` answers whether any
/// terrain collision overlaps a world-space box.
pub fn step(
    map: &impl RailMap,
    cart: Cart,
    body: Body,
    controls: Controls,
    blocked: &dyn Fn(Aabb) -> bool,
) -> Step {
    let Some((mut cell, rail)) = rail_cell(map, cart.pos) else {
        return Step::Off;
    };
    let mut path = Path::of(rail.form);
    let mut s = path.project([cart.pos[0] - cell[0] as f32, cart.pos[2] - cell[2] as f32]);

    // Which way along the path the cart FACES: +1 when its nose points
    // toward increasing `s`. A cart dropped crosswise onto a rail picks +1.
    let facing = facing_xz(cart.yaw);
    let mut faces_forward: f32 = if dot2(facing, xz(path.tangent(s))) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    // Velocity along increasing `s`.
    let mut v = cart.speed * faces_forward;

    // --- Forces, on the along-path velocity. ---
    let g = grade(&path, s);
    if g != 0.0 {
        v -= SLOPE_ACCEL * g.signum() * DT;
    }
    if rail.booster {
        if v.abs() > BOOST_IDLE {
            v += BOOST_ACCEL * v.signum() * DT;
        } else {
            v = booster_launch(&path, cell, blocked);
        }
    }
    let push = controls.push * faces_forward;
    if push != 0.0 && (v.abs() < RIDER_MAX || push.signum() != v.signum()) {
        v += RIDER_ACCEL * push * DT;
    }
    v *= DRAG_PER_TICK;
    if !rail.booster && controls.push == 0.0 {
        v = approach(v, 0.0, ROLL_RESIST * DT);
        if g == 0.0 && v.abs() < STOP_SPEED {
            v = 0.0;
        }
    }
    v = v.clamp(-MAX_SPEED, MAX_SPEED);

    // --- Advance along the path, chaining through linked cells. ---
    let mut remaining = v.abs() * DT;
    let mut dir = v.signum();
    let mut derailed = false;
    for _ in 0..MAX_CELLS_PER_TICK {
        if dir == 0.0 {
            break;
        }
        let room = if dir > 0.0 { path.len() - s } else { s };
        if remaining <= room + 1e-6 {
            s = (s + dir * remaining).clamp(0.0, path.len());
            break;
        }
        remaining -= room;
        let exit = path.exits()[if dir > 0.0 { 1 } else { 0 }];
        match link(map, cell, exit) {
            Some(l) => {
                let next = map.rail(l.cell).expect("a link names a rail");
                let next_path = Path::of(next.form);
                let enters_at_start = next_path
                    .starts_at(l.entry)
                    .expect("a link enters by an exit");
                cell = l.cell;
                path = next_path;
                let new_dir = if enters_at_start { 1.0 } else { -1.0 };
                // `s` may run the other way through the next cell: the
                // along-path velocity and the nose both re-sign with it, so
                // the cart keeps moving — and facing — the way it was.
                let flip = dir * new_dir;
                faces_forward *= flip;
                v *= flip;
                dir = new_dir;
                s = if enters_at_start { 0.0 } else { path.len() };
            }
            None => {
                s = if dir > 0.0 { path.len() } else { 0.0 };
                let beyond = add(cell, exit.dir.offset());
                if map.rail(beyond).is_some() || map.rail(add(beyond, [0, -1, 0])).is_some() {
                    // A rail that does not join ours is a bumper: stop dead at it.
                    v = 0.0;
                } else {
                    derailed = true;
                }
                break;
            }
        }
    }

    let p = path.point(s);
    let t = path.tangent(s);
    let mut pos = [
        cell[0] as f32 + p[0],
        cell[1] as f32 + p[1] + RAIL_TOP,
        cell[2] as f32 + p[2],
    ];
    if derailed {
        // Carry the unspent motion past the end of the track, level, so the
        // engine inherits exactly the speed the cart left the rails with.
        pos[0] += t[0] * dir * remaining;
        pos[2] += t[2] * dir * remaining;
    }
    let nose = [t[0] * faces_forward, t[2] * faces_forward];
    let yaw = if dot2(nose, nose) > 1e-8 {
        yaw_facing(nose)
    } else {
        cart.yaw
    };
    let climb = t[1] * faces_forward;
    let pitch_target = climb.atan2((t[0] * t[0] + t[2] * t[2]).sqrt());
    let pitch = approach(cart.pitch, pitch_target, PITCH_RATE * DT);
    // A rail may run straight into a wall: the track constrains the wheels,
    // the wall still stops the body. Moving into terrain parks the cart
    // where it was, dead.
    if pos != cart.pos && blocked(body.upper_half(pos)) {
        return Step::Railed(Cart {
            pos: cart.pos,
            yaw: cart.yaw,
            pitch,
            speed: 0.0,
        });
    }
    let out = Cart {
        pos,
        yaw,
        pitch,
        speed: v * faces_forward,
    };
    if derailed {
        Step::Derailed(out)
    } else {
        Step::Railed(out)
    }
}

/// The speed a booster gives a standing cart: uphill on a slope; on level
/// track away from a block butted against one end (a station bumper),
/// nothing when both or neither end is blocked.
fn booster_launch(path: &Path, cell: [i32; 3], blocked: &dyn Fn(Aabb) -> bool) -> f32 {
    let [a, b] = path.exits();
    if b.up {
        return BOOST_LAUNCH;
    }
    let butted = |d: Dir| blocked(cell_box(add(cell, d.offset())));
    match (butted(a.dir), butted(b.dir)) {
        (true, false) => BOOST_LAUNCH,
        (false, true) => -BOOST_LAUNCH,
        _ => 0.0,
    }
}

/// What one other cart in contact does to this cart's speed this tick.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Contact {
    /// The other cart is approaching: an elastic hit between equal masses
    /// hands this cart the other's along-track speed (the mover stops dead,
    /// the cart it hit rolls on; a chain shunts down the line one cart at a
    /// time). `closing` is how hard, for ranking several hits.
    Exchange { speed: f32, closing: f32 },
    /// The two merely rest in overlap: a shove apart along this cart's own
    /// facing, harder the deeper, as a speed delta.
    Shove(f32),
}

/// Carts within a cart length of each other on the same level are in
/// contact; `None` otherwise. Decided from both carts' PRE-tick states so
/// the pair agrees on the exchange whichever steps first.
pub fn collide(cart: &Cart, other: &Cart) -> Option<Contact> {
    if (cart.pos[1] - other.pos[1]).abs() > 1.0 {
        return None;
    }
    let d = [cart.pos[0] - other.pos[0], cart.pos[2] - other.pos[2]];
    let dist = dot2(d, d).sqrt();
    if !(1e-4..CART_LENGTH).contains(&dist) {
        return None;
    }
    let away = [d[0] / dist, d[1] / dist];
    let f = facing_xz(cart.yaw);
    let g = facing_xz(other.yaw);
    let vel = [f[0] * cart.speed, f[1] * cart.speed];
    let other_vel = [g[0] * other.speed, g[1] * other.speed];
    let closing = dot2([other_vel[0] - vel[0], other_vel[1] - vel[1]], away);
    if closing > STOP_SPEED {
        return Some(Contact::Exchange {
            speed: dot2(other_vel, f),
            closing,
        });
    }
    let overlap = CART_LENGTH - dist;
    Some(Contact::Shove(dot2(away, f) * overlap * NUDGE_ACCEL * DT))
}

/// This cart's speed after every contact it has this tick: the hardest hit
/// (the largest closing speed) decides an exchange outright — one elastic
/// hit is one exchange, never a sum — and failing any hit, the resting
/// shoves add up.
pub fn resolve_contacts<'a>(cart: &Cart, others: impl Iterator<Item = &'a Cart>) -> f32 {
    let mut hardest: Option<(f32, f32)> = None;
    let mut shove = 0.0;
    for contact in others.filter_map(|other| collide(cart, other)) {
        match contact {
            Contact::Exchange { speed, closing } => {
                if hardest.is_none_or(|(_, c)| closing > c) {
                    hardest = Some((speed, closing));
                }
            }
            Contact::Shove(delta) => shove += delta,
        }
    }
    match hardest {
        Some((speed, _)) => speed,
        None => cart.speed + shove,
    }
}

/// The signed speed a punch from `origin` gives the cart: away from the
/// puncher, along the facing.
pub fn punch(cart: &Cart, origin: [f32; 3]) -> f32 {
    let d = [cart.pos[0] - origin[0], cart.pos[2] - origin[2]];
    if dot2(d, facing_xz(cart.yaw)) >= 0.0 {
        PUNCH_SPEED
    } else {
        -PUNCH_SPEED
    }
}

/// Playback rate of the wheels' `roll` clip (one revolution per second of
/// clip) for a speed, given the authored wheel diameter in blocks.
pub fn wheel_roll_rate(speed: f32, wheel_diameter: f32) -> f32 {
    speed / (std::f32::consts::PI * wheel_diameter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rail::{Axis, Corner, Dir, Form};
    use std::collections::BTreeMap;

    struct Map(BTreeMap<[i32; 3], Rail>);

    impl Map {
        fn new(cells: &[([i32; 3], Form)]) -> Self {
            Map(cells
                .iter()
                .map(|&(c, f)| {
                    (
                        c,
                        Rail {
                            form: f,
                            booster: false,
                        },
                    )
                })
                .collect())
        }
        fn booster(mut self, cell: [i32; 3]) -> Self {
            self.0.get_mut(&cell).expect("booster on a rail").booster = true;
            self
        }
    }

    impl RailMap for Map {
        fn rail(&self, cell: [i32; 3]) -> Option<Rail> {
            self.0.get(&cell).copied()
        }
    }

    const NS: Form = Form::Straight(Axis::NS);
    const EW: Form = Form::Straight(Axis::EW);
    const OPEN: &dyn Fn(Aabb) -> bool = &|_| false;

    /// A full block in one cell and nothing else.
    fn block_at(cell: [i32; 3]) -> impl Fn(Aabb) -> bool {
        move |probe| overlaps(probe, cell_box(cell))
    }
    const BODY: Body = Body {
        half_width: 0.45,
        height: 0.78,
    };

    fn at(x: f32, y: f32, z: f32, yaw: f32, speed: f32) -> Cart {
        Cart {
            pos: [x, y + RAIL_TOP, z],
            yaw,
            pitch: 0.0,
            speed,
        }
    }

    fn railed(s: Step) -> Cart {
        match s {
            Step::Railed(c) => c,
            other => panic!("expected railed, got {other:?}"),
        }
    }

    fn run(map: &Map, mut cart: Cart, ticks: usize, controls: Controls) -> Cart {
        for _ in 0..ticks {
            cart = railed(step(map, cart, BODY, controls, OPEN));
        }
        cart
    }

    #[test]
    fn a_cart_coasts_along_a_run_and_parks_from_rolling_resistance() {
        let map = Map::new(&(0..40).map(|z| ([0, 0, z], NS)).collect::<Vec<_>>());
        // Facing south (yaw π), rolling forward.
        let cart = at(0.5, 0.0, 0.5, std::f32::consts::PI, 4.0);
        let later = run(&map, cart, 20, Controls::default());
        assert!(later.pos[2] > 4.0, "moved south: {:?}", later.pos);
        assert!(
            later.speed < 4.0 && later.speed > 3.0,
            "eased: {}",
            later.speed
        );
        assert!((later.pos[0] - 0.5).abs() < 1e-4, "held to the centre line");
        let parked = run(&map, later, 20 * 60, Controls::default());
        assert_eq!(parked.speed, 0.0, "resistance parks it");
        assert!(parked.pos[2] < 40.0);
    }

    #[test]
    fn a_cart_rolls_down_a_slope_and_a_rider_can_push_it_back_up() {
        // A slope at z=1 climbing north to a straight at (0,1,0); flat run south of it.
        let map = Map::new(&[
            ([0, 1, -3], NS),
            ([0, 1, -2], NS),
            ([0, 1, -1], NS),
            ([0, 1, 0], NS),
            ([0, 0, 1], Form::Slope(Dir::N)),
            ([0, 0, 2], NS),
            ([0, 0, 3], NS),
            ([0, 0, 4], NS),
        ]);
        // Parked halfway up, facing north (uphill).
        let start = Cart {
            pos: [0.5, 0.5 + RAIL_TOP, 1.5],
            yaw: 0.0,
            pitch: 0.0,
            speed: 0.0,
        };
        let c = railed(step(&map, start, BODY, Controls::default(), OPEN));
        assert!(
            c.speed < 0.0,
            "gravity rolls it backwards down the slope: {}",
            c.speed
        );
        assert!(c.pos[1] < start.pos[1], "and it descends: {:?}", c.pos);
        assert!(c.pitch > 0.0, "nose up while facing uphill: {}", c.pitch);
        let c = run(&map, c, 40, Controls::default());
        assert!(
            c.pos[2] > 2.0 && c.pos[1] < 0.2,
            "reached the flat: {:?}",
            c.pos
        );
        assert!((c.pitch).abs() < 0.05, "levelled out: {}", c.pitch);
        // Push forward (uphill) from the flat: it climbs.
        let pushed = run(&map, c, 60, Controls { push: 1.0 });
        assert!(pushed.speed > 0.0 && pushed.pos[2] < c.pos[2], "{pushed:?}");
    }

    #[test]
    fn a_cart_rounds_a_corner_keeping_its_nose_ahead_and_speed_sign() {
        // North-south run into a curve turning east, then an east-west run.
        let map = Map::new(&[
            ([0, 0, 3], NS),
            ([0, 0, 2], NS),
            ([0, 0, 1], NS),
            ([0, 0, 0], Form::Curve(Corner::SE)),
            ([1, 0, 0], EW),
            ([2, 0, 0], EW),
            ([3, 0, 0], EW),
        ]);
        // Facing north at top speed.
        let cart = at(0.5, 0.0, 3.5, 0.0, 8.0);
        let mut c = cart;
        let mut turned = false;
        for _ in 0..40 {
            c = railed(step(&map, c, BODY, Controls::default(), OPEN));
            if c.pos[0] > 1.0 {
                turned = true;
                break;
            }
        }
        assert!(turned, "came out of the corner heading east: {c:?}");
        let f = facing_xz(c.yaw);
        assert!(f[0] > 0.99, "nose points east now: {f:?}");
        assert!(c.speed > 7.0, "speed is still forward: {}", c.speed);

        // The same corner taken BACKWARDS: nose stays behind, speed stays negative.
        let back = at(0.5, 0.0, 3.5, std::f32::consts::PI, -8.0);
        let mut c = back;
        for _ in 0..40 {
            c = railed(step(&map, c, BODY, Controls::default(), OPEN));
            if c.pos[0] > 1.0 {
                break;
            }
        }
        assert!(c.pos[0] > 1.0);
        let f = facing_xz(c.yaw);
        assert!(
            f[0] < -0.99,
            "rolling east tail-first, the nose points west: {f:?}"
        );
        assert!(c.speed < -7.0);
    }

    #[test]
    fn the_end_of_the_track_derails_and_a_crossing_rail_is_a_bumper() {
        let map = Map::new(&[([0, 0, 0], NS), ([0, 0, 1], NS)]);
        let cart = at(0.5, 0.0, 1.8, std::f32::consts::PI, 8.0);
        let out = step(&map, cart, BODY, Controls::default(), OPEN);
        let Step::Derailed(c) = out else {
            panic!("{out:?}")
        };
        assert!(c.pos[2] > 2.0, "carried past the end: {:?}", c.pos);
        assert!(
            (c.pos[2] - (1.8 + 8.0 * DT)).abs() < 0.01,
            "at rail speed: {:?}",
            c.pos
        );

        let bumper = Map::new(&[([0, 0, 0], NS), ([0, 0, 1], NS), ([0, 0, 2], EW)]);
        let c = railed(step(&bumper, cart, BODY, Controls::default(), OPEN));
        assert_eq!(c.speed, 0.0, "stopped at the crossing rail");
        assert!((c.pos[2] - 2.0).abs() < 1e-4, "on the edge: {:?}", c.pos);

        let off = step(
            &map,
            at(5.5, 0.0, 5.5, 0.0, 1.0),
            BODY,
            Controls::default(),
            OPEN,
        );
        assert_eq!(off, Step::Off);
    }

    #[test]
    fn a_booster_accelerates_a_rolling_cart_and_launches_a_standing_one_off_a_block() {
        let map =
            Map::new(&(0..12).map(|z| ([0, 0, z], NS)).collect::<Vec<_>>()).booster([0, 0, 2]);
        let slow = at(0.5, 0.0, 2.5, std::f32::consts::PI, 1.0);
        let c = railed(step(&map, slow, BODY, Controls::default(), OPEN));
        assert!(c.speed > 1.5, "boosted: {}", c.speed);

        let standing = at(0.5, 0.0, 2.5, std::f32::consts::PI, 0.0);
        let c = railed(step(&map, standing, BODY, Controls::default(), OPEN));
        assert_eq!(c.speed, 0.0, "nothing to push off");
        let wall_north = block_at([0, 0, 1]);
        let c = railed(step(&map, standing, BODY, Controls::default(), &wall_north));
        assert!(
            c.speed > 0.0,
            "launched south, away from the wall: {}",
            c.speed
        );
        let wall_south = block_at([0, 0, 3]);
        let c = railed(step(&map, standing, BODY, Controls::default(), &wall_south));
        assert!(
            c.speed < 0.0,
            "launched north (backwards for a south-facing cart): {}",
            c.speed
        );
    }

    #[test]
    fn a_mover_hands_its_speed_to_the_cart_it_hits_and_resting_overlap_shoves_apart() {
        let south = std::f32::consts::PI;
        let after = |cart: &Cart, others: &[Cart]| resolve_contacts(cart, others.iter());
        let parked = at(0.5, 0.0, 4.5, south, 0.0);
        let mover = at(0.5, 0.0, 3.8, south, 6.0);
        let parked_after = after(&parked, &[mover]);
        let mover_after = after(&mover, &[parked]);
        assert!(
            (parked_after - 6.0).abs() < 1e-4,
            "parked rolls on at {parked_after}"
        );
        assert!(
            mover_after.abs() < 1e-4,
            "mover stops dead at {mover_after}"
        );
        // Head-on: two carts facing each other, each rolling forward — each
        // takes the other's velocity, i.e. bounces back at the other's pace.
        let north_bound = at(0.5, 0.0, 4.5, 0.0, 2.0);
        let south_bound = at(0.5, 0.0, 3.8, south, 5.0);
        assert!((after(&north_bound, &[south_bound]) + 5.0).abs() < 1e-4);
        assert!((after(&south_bound, &[north_bound]) + 2.0).abs() < 1e-4);
        // Parting or resting carts do not re-trade; a resting overlap shoves
        // apart along each cart's own facing.
        let leaving = at(0.5, 0.0, 4.5, south, 6.0);
        let stopped = at(0.5, 0.0, 3.8, south, 0.0);
        assert!(
            after(&stopped, &[leaving]) < 0.0,
            "shoved back off the leaving cart"
        );
        assert!(
            after(&leaving, &[stopped]) > 6.0,
            "shoved on ahead of the stopped one"
        );
        assert!(
            collide(&stopped, &at(0.5, 0.0, 6.0, south, 0.0)).is_none(),
            "out of reach"
        );
        // Hit from both sides at once, the HARDER hit alone decides the
        // exchange: a parked cart between a 6 m/s mover behind it and a
        // 2 m/s mover ahead takes the 6, not a sum or whichever came last.
        let fast_behind = at(0.5, 0.0, 3.8, south, 6.0);
        let slow_ahead = at(0.5, 0.0, 5.2, 0.0, 2.0);
        let hit = after(&parked, &[slow_ahead, fast_behind]);
        assert!((hit - 6.0).abs() < 1e-4, "the harder hit decides: {hit}");
        assert_eq!(
            hit,
            after(&parked, &[fast_behind, slow_ahead]),
            "in any order"
        );
        // Resting in overlap with two neighbours, the shoves add up.
        let between = at(0.5, 0.0, 4.5, south, 0.0);
        let left = at(0.5, 0.0, 3.9, south, 0.0);
        let right = at(0.5, 0.0, 5.1, south, 0.0);
        let both = after(&between, &[left, right]);
        assert!(
            (both - (after(&between, &[left]) + after(&between, &[right]))).abs() < 1e-5,
            "shoves sum: {both}"
        );
    }

    #[test]
    fn a_punch_sends_a_cart_away_from_the_puncher() {
        let a = at(0.5, 0.0, 0.5, std::f32::consts::PI, 0.0);
        assert!(punch(&a, [0.5, 0.0, -1.0]) > 0.0);
        assert!(punch(&a, [0.5, 0.0, 2.0]) < 0.0);
    }

    #[test]
    fn a_wall_across_the_track_stops_the_cart_but_a_slopes_own_hill_does_not() {
        let map = Map::new(&[([0, 0, 0], NS), ([0, 0, 1], NS), ([0, 0, 2], NS)]);
        // Rolling south into a wall cell at z=3 (the track ends there too):
        // the wall wins before the derail.
        let wall = block_at([0, 0, 3]);
        let cart = at(0.5, 0.0, 2.3, std::f32::consts::PI, 8.0);
        let c = railed(step(&map, cart, BODY, Controls::default(), &wall));
        assert_eq!(c.speed, 0.0, "parked at the wall");
        assert_eq!(c.pos, cart.pos, "did not enter it");

        // Climbing north: the block under the upper rail is level with the
        // slope cell, and the leaning body's lower half overlaps it — that
        // is not a wall.
        let map = Map::new(&[
            ([0, 0, 1], Form::Slope(Dir::N)),
            ([0, 1, 0], NS),
            ([0, 1, -1], NS),
        ]);
        let hill = block_at([0, 0, 0]);
        let mut c = Cart {
            pos: [0.5, 0.3 + RAIL_TOP, 1.7],
            yaw: 0.0,
            pitch: 0.0,
            speed: 6.0,
        };
        for _ in 0..6 {
            c = railed(step(&map, c, BODY, Controls { push: 1.0 }, &hill));
        }
        assert!(
            c.pos[1] > 1.0 && c.pos[2] < 0.5,
            "climbed onto the upper run: {c:?}"
        );
        // A wall at the upper run's level, ahead of the cart, does stop it.
        let overhang = block_at([0, 1, -1]);
        let c2 = railed(step(&map, c, BODY, Controls { push: 1.0 }, &overhang));
        assert_eq!(c2.speed, 0.0, "{c2:?}");
    }
}
