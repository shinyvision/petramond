use std::collections::BTreeMap;

use crate::rail::{add, link, Dir, Rail, RailMap};
use crate::track::{dot2, grade, xz, yaw_facing, Path, RAIL_TOP};

pub type Aabb = ([f64; 3], [f64; 3]);

pub fn overlaps(a: Aabb, b: Aabb) -> bool {
    (0..3).all(|i| a.0[i] < b.1[i] && b.0[i] < a.1[i])
}

pub fn cell_box(cell: [i32; 3]) -> Aabb {
    let min = [cell[0] as f64, cell[1] as f64, cell[2] as f64];
    (min, [min[0] + 1.0, min[1] + 1.0, min[2] + 1.0])
}

pub const DT: f32 = 1.0 / 20.0;

pub const MAX_SPEED: f32 = 12.0;
pub const SLOPE_ACCEL: f32 = 3.5;
pub const DRAG_PER_TICK: f32 = 0.997;
pub const ROLL_RESIST: f32 = 0.25;
pub const STOP_SPEED: f32 = 0.05;
pub const BOOST_ACCEL: f32 = 14.0;
pub const BOOST_LAUNCH: f32 = 1.5;
pub const BOOST_IDLE: f32 = 0.1;
pub const RIDER_ACCEL: f32 = 3.0;
pub const RIDER_MAX: f32 = 4.5;
pub const PITCH_RATE: f32 = 9.0;
pub const CART_LENGTH: f32 = 1.0;
pub const NUDGE_ACCEL: f32 = 12.0;
pub const PUNCH_SPEED: f32 = 2.5;
pub const SKID_RETENTION: f32 = 0.85;
pub const ROLL_SOUND_START: f32 = 0.15;
pub const ROLL_SOUND_CURVE: f32 = 0.7;
const MAX_CELLS_PER_TICK: usize = 6;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Cart {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub speed: f32,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Controls {
    pub push: f32,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Body {
    pub half_width: f32,
    pub height: f32,
}

impl Body {
    pub fn upper_half(self, pos: [f64; 3]) -> Aabb {
        let (hw, height) = (f64::from(self.half_width), f64::from(self.height));
        (
            [pos[0] - hw, pos[1] + height * 0.5, pos[2] - hw],
            [pos[0] + hw, pos[1] + height, pos[2] + hw],
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Railed(Cart),
    Derailed(Cart),
    Off,
}

pub fn facing_xz(yaw: f32) -> [f32; 2] {
    let (s, c) = yaw.sin_cos();
    [-s, -c]
}

pub fn rail_cell(map: &impl RailMap, pos: [f64; 3]) -> Option<([i32; 3], Rail)> {
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
    let mut s = path.project([
        (cart.pos[0] - cell[0] as f64) as f32,
        (cart.pos[2] - cell[2] as f64) as f32,
    ]);

    let facing = facing_xz(cart.yaw);
    let mut faces_forward: f32 = if dot2(facing, xz(path.tangent(s))) >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let mut v = cart.speed * faces_forward;

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
        cell[0] as f64 + f64::from(p[0]),
        cell[1] as f64 + f64::from(p[1] + RAIL_TOP),
        cell[2] as f64 + f64::from(p[2]),
    ];
    if derailed {
        pos[0] += f64::from(t[0] * dir * remaining);
        pos[2] += f64::from(t[2] * dir * remaining);
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

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Contact {
    Exchange { speed: f32, closing: f32 },
    Shove(f32),
}

pub fn collide(cart: &Cart, other: &Cart) -> Option<Contact> {
    if (cart.pos[1] - other.pos[1]).abs() > 1.0 {
        return None;
    }
    let d = [
        (cart.pos[0] - other.pos[0]) as f32,
        (cart.pos[2] - other.pos[2]) as f32,
    ];
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

pub struct ContactGrid {
    cells: BTreeMap<(i64, i64), Vec<usize>>,
}

impl ContactGrid {
    pub fn new(carts: &[Cart]) -> Self {
        let mut cells: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        for (i, cart) in carts.iter().enumerate() {
            cells.entry(Self::cell(cart)).or_default().push(i);
        }
        Self { cells }
    }

    fn cell(cart: &Cart) -> (i64, i64) {
        let size = f64::from(CART_LENGTH);
        (
            (cart.pos[0] / size).floor() as i64,
            (cart.pos[2] / size).floor() as i64,
        )
    }

    pub fn neighbours(&self, carts: &[Cart], i: usize) -> Vec<usize> {
        let (cx, cz) = Self::cell(&carts[i]);
        let mut near: Vec<usize> = (cx - 1..=cx + 1)
            .flat_map(|x| (cz - 1..=cz + 1).map(move |z| (x, z)))
            .filter_map(|cell| self.cells.get(&cell))
            .flatten()
            .copied()
            .filter(|&j| j != i)
            .collect();
        near.sort_unstable();
        near
    }
}

pub fn punch(cart: &Cart, origin: [f64; 3]) -> f32 {
    let d = [
        (cart.pos[0] - origin[0]) as f32,
        (cart.pos[2] - origin[2]) as f32,
    ];
    if dot2(d, facing_xz(cart.yaw)) >= 0.0 {
        PUNCH_SPEED
    } else {
        -PUNCH_SPEED
    }
}

pub fn wheel_roll_rate(speed: f32, wheel_diameter: f32) -> f32 {
    speed / (std::f32::consts::PI * wheel_diameter)
}

pub fn roll_volume(speed: f32) -> f32 {
    let t = (speed.abs() - ROLL_SOUND_START) / (MAX_SPEED - ROLL_SOUND_START);
    if t <= 0.0 {
        0.0
    } else {
        t.min(1.0).powf(ROLL_SOUND_CURVE)
    }
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

    fn block_at(cell: [i32; 3]) -> impl Fn(Aabb) -> bool {
        move |probe| overlaps(probe, cell_box(cell))
    }
    const BODY: Body = Body {
        half_width: 0.45,
        height: 0.78,
    };

    fn at(x: f32, y: f32, z: f32, yaw: f32, speed: f32) -> Cart {
        Cart {
            pos: [f64::from(x), f64::from(y + RAIL_TOP), f64::from(z)],
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
        let start = Cart {
            pos: [0.5, 0.5 + f64::from(RAIL_TOP), 1.5],
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
        let pushed = run(&map, c, 60, Controls { push: 1.0 });
        assert!(pushed.speed > 0.0 && pushed.pos[2] < c.pos[2], "{pushed:?}");
    }

    #[test]
    fn a_cart_rounds_a_corner_keeping_its_nose_ahead_and_speed_sign() {
        let map = Map::new(&[
            ([0, 0, 3], NS),
            ([0, 0, 2], NS),
            ([0, 0, 1], NS),
            ([0, 0, 0], Form::Curve(Corner::SE)),
            ([1, 0, 0], EW),
            ([2, 0, 0], EW),
            ([3, 0, 0], EW),
        ]);
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
            (c.pos[2] - f64::from(1.8 + 8.0 * DT)).abs() < 0.01,
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
        let north_bound = at(0.5, 0.0, 4.5, 0.0, 2.0);
        let south_bound = at(0.5, 0.0, 3.8, south, 5.0);
        assert!((after(&north_bound, &[south_bound]) + 5.0).abs() < 1e-4);
        assert!((after(&south_bound, &[north_bound]) + 2.0).abs() < 1e-4);
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
        let fast_behind = at(0.5, 0.0, 3.8, south, 6.0);
        let slow_ahead = at(0.5, 0.0, 5.2, 0.0, 2.0);
        let hit = after(&parked, &[slow_ahead, fast_behind]);
        assert!((hit - 6.0).abs() < 1e-4, "the harder hit decides: {hit}");
        assert_eq!(
            hit,
            after(&parked, &[fast_behind, slow_ahead]),
            "in any order"
        );
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
        let wall = block_at([0, 0, 3]);
        let cart = at(0.5, 0.0, 2.3, std::f32::consts::PI, 8.0);
        let c = railed(step(&map, cart, BODY, Controls::default(), &wall));
        assert_eq!(c.speed, 0.0, "parked at the wall");
        assert_eq!(c.pos, cart.pos, "did not enter it");

        let map = Map::new(&[
            ([0, 0, 1], Form::Slope(Dir::N)),
            ([0, 1, 0], NS),
            ([0, 1, -1], NS),
        ]);
        let hill = block_at([0, 0, 0]);
        let mut c = Cart {
            pos: [0.5, 0.3 + f64::from(RAIL_TOP), 1.7],
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
        let overhang = block_at([0, 1, -1]);
        let c2 = railed(step(&map, c, BODY, Controls { push: 1.0 }, &overhang));
        assert_eq!(c2.speed, 0.0, "{c2:?}");
    }

    #[test]
    fn the_contact_grid_resolves_exactly_like_scanning_every_cart() {
        let mut carts = Vec::new();
        for i in 0..60u32 {
            let h = i.wrapping_mul(0x9E37_79B9);
            carts.push(Cart {
                pos: [
                    f64::from(h % 23) * 0.37 - 4.0,
                    f64::from((h >> 8) % 3),
                    f64::from((h >> 16) % 29) * 0.29 - 3.5,
                ],
                yaw: (h % 628) as f32 / 100.0,
                pitch: 0.0,
                speed: ((h >> 4) % 90) as f32 / 10.0 - 4.5,
            });
        }
        let grid = ContactGrid::new(&carts);
        let mut touched = 0;
        for (i, cart) in carts.iter().enumerate() {
            let everyone = carts
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, other)| other);
            let near = grid.neighbours(&carts, i);
            assert!(near.windows(2).all(|w| w[0] < w[1]), "ascending, unique");
            let brute = resolve_contacts(cart, everyone);
            let bucketed = resolve_contacts(cart, near.iter().map(|&j| &carts[j]));
            assert_eq!(brute.to_bits(), bucketed.to_bits(), "cart {i}");
            if brute != cart.speed {
                touched += 1;
            }
        }
        assert!(touched > 5, "the yard really has contacts ({touched})");
    }
}
