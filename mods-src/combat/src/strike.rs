//! Where a swing lands and how hard.
//!
//! The engine's own melee hits on the click, on whatever the crosshair held. Tools this pack
//! animates claim the press (`attack_attempt`) instead, and the hit lands when the swing's
//! authored IMPACT plays. It's aimed from wherever the attacker is looking at that moment, at
//! every body in the family's strike window. The crosshair isn't consulted. If the axe looks like
//! it hit, it hit; a body that stepped out of the arc was missed.
//!
//! How hard depends on how close the body is (full strength in the sweet spot, tapering toward
//! the end of reach) and how dead-on the swing is. Dead-on in the sweet spot beats the tool's
//! plain roll; a glancing hit at the edge of the window lands far less. Axes sweep wide and flat
//! and hit every body in the window. Pickaxes plunge tall and narrow, and only the best-placed
//! body takes it.
//!
//! [`judge`] is pure: the numbers, the attacker's aiming frame, the bodies. [`land`] runs it over
//! the live world and lands the verdicts through the engine's damage funnel. It names the
//! attacker, so the victim remembers them, knockback applies, and `mob_damage_pre` handlers see
//! the same strike the engine's own hit would have shown.

use mod_sdk::*;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Profile {
    pub reach: f32,
    pub sweet: f32,
    pub arc_yaw: f32,
    pub arc_pitch: f32,
    pub peak: f32,
    pub floor: f32,
    pub cleave: bool,
}

const fn radians(degrees: f32) -> f32 {
    degrees * (std::f32::consts::PI / 180.0)
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSpec {
    reach: f32,
    sweet: f32,
    arc_yaw: f32,
    arc_pitch: f32,
    peak: f32,
    floor: f32,
    cleave: bool,
}

impl Profile {
    pub fn from_spec(spec: &ProfileSpec) -> Profile {
        Profile {
            reach: spec.reach,
            sweet: spec.sweet,
            arc_yaw: radians(spec.arc_yaw),
            arc_pitch: radians(spec.arc_pitch),
            peak: spec.peak,
            floor: spec.floor,
            cleave: spec.cleave,
        }
    }
}

const NEAR: f32 = 0.3;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aim {
    pub eye: [f64; 3],
    pub forward: [f32; 3],
    pub across: [f32; 3],
    pub up: [f32; 3],
}

impl Aim {
    pub fn of(state: &PlayerSnapshot) -> Aim {
        let (cp, sp) = (state.pitch.cos(), state.pitch.sin());
        let (sy, cy) = (state.yaw.sin(), state.yaw.cos());
        let forward = [sy * cp, sp, cy * cp];
        let across = [cy, 0.0, -sy];
        Aim {
            eye: [
                state.pos[0],
                state.pos[1] + f64::from(state.eye_height),
                state.pos[2],
            ],
            forward,
            across,
            up: cross(across, forward),
        }
    }
}

pub type Box3 = ([f64; 3], [f64; 3]);

pub fn mob_boxes(m: &MobSnapshot) -> Vec<Box3> {
    let hw = m.half_width;
    let segments = (m.half_length / hw).ceil().max(1.0) as usize;
    let reach = m.half_length - hw;
    let facing = [-m.yaw.sin(), 0.0, -m.yaw.cos()];
    (0..segments)
        .map(|i| {
            let offset = if segments == 1 {
                0.0
            } else {
                -reach + 2.0 * reach * i as f32 / (segments - 1) as f32
            };
            let c = offset_by(m.pos, scale(facing, offset));
            let (hw, height) = (f64::from(hw), f64::from(m.height));
            (
                [c[0] - hw, c[1], c[2] - hw],
                [c[0] + hw, c[1] + height, c[2] + hw],
            )
        })
        .collect()
}

pub fn player_box(p: &PlayerSnapshot) -> Box3 {
    let (hw, height) = (f64::from(p.half_width), f64::from(p.height));
    (
        [p.pos[0] - hw, p.pos[1], p.pos[2] - hw],
        [p.pos[0] + hw, p.pos[1] + height, p.pos[2] + hw],
    )
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Hit {
    pub multiplier: f32,
    pub point: [f64; 3],
    pub distance: f32,
}

pub fn judge(profile: &Profile, aim: &Aim, boxes: &[Box3]) -> Option<Hit> {
    boxes
        .iter()
        .filter_map(|&(min, max)| judge_box(profile, aim, min, max))
        .max_by(|a, b| a.multiplier.total_cmp(&b.multiplier))
}

fn judge_box(profile: &Profile, aim: &Aim, min: [f64; 3], max: [f64; 3]) -> Option<Hit> {
    let (min, max) = (relative(min, aim.eye), relative(max, aim.eye));
    let nearest = clamp3([0.0; 3], min, max);
    let distance = length(nearest);
    if distance > profile.reach || dot(nearest, aim.forward) < 0.0 {
        return None;
    }
    let centre = scale(add(min, max), 0.5);
    let mut t = dot(centre, aim.forward).clamp(NEAR, profile.reach);
    for _ in 0..2 {
        let on_ray = scale(aim.forward, t);
        let on_box = clamp3(on_ray, min, max);
        t = dot(on_box, aim.forward).clamp(NEAR, profile.reach);
    }
    let on_ray = scale(aim.forward, t);
    let miss = sub(clamp3(on_ray, min, max), on_ray);
    let yaw_err = dot(miss, aim.across).abs().atan2(t);
    let pitch_err = dot(miss, aim.up).abs().atan2(t);
    if yaw_err > profile.arc_yaw || pitch_err > profile.arc_pitch {
        return None;
    }
    let inside = |err: f32, arc: f32| 1.0 - (err / arc) * (err / arc);
    let aim_term = inside(yaw_err, profile.arc_yaw) * inside(pitch_err, profile.arc_pitch);
    let dist_term = if distance <= profile.sweet {
        1.0
    } else {
        1.0 - (distance - profile.sweet) / (profile.reach - profile.sweet).max(1e-3)
    };
    let multiplier = profile.floor + (profile.peak - profile.floor) * aim_term * dist_term;
    Some(Hit {
        multiplier,
        point: offset_by(aim.eye, nearest),
        distance,
    })
}

fn in_sight(aim: &Aim, hit: &Hit) -> bool {
    if hit.distance <= NEAR {
        return true;
    }
    let dir = relative(hit.point, aim.eye);
    raycast(aim.eye, dir, hit.distance, RayFilter::Collidable)
        .is_none_or(|block| block.distance >= hit.distance - 0.05)
}

struct Candidate {
    who: EntityRef,
    hit: Hit,
}

pub fn roll(range: [f32; 2], word: u64) -> f32 {
    let u = (word >> 11) as f32 / (1u64 << 53) as f32;
    range[0] + (range[1] - range[0]) * u
}

fn roll_damage(me: PlayerId) -> f32 {
    let range = player_held(me)
        .and_then(|stack| stack_info(&stack))
        .and_then(|info| info.tool.map(|t| t.damage))
        .unwrap_or([1.0, 1.0]);
    roll(range, rng_u64("strike"))
}

pub fn land(me: PlayerId, profile: &Profile, state: &PlayerSnapshot) {
    if state.spectator || state.health <= 0 {
        return;
    }
    let aim = Aim::of(state);
    let mut candidates: Vec<Candidate> = Vec::new();
    let radius = profile.reach + 4.0;
    for mob in mobs_in_radius(state.pos, radius) {
        if let Some(hit) = judge(profile, &aim, &mob_boxes(&mob)) {
            let own_mount = mob_riders(mob.id)
                .is_some_and(|riders| riders.riders.iter().any(|r| r.player_id == me));
            if own_mount {
                continue;
            }
            candidates.push(Candidate {
                who: EntityRef::Mob(mob.id),
                hit,
            });
        }
    }
    for entry in players() {
        let other = &entry.state;
        if entry.id == me || other.spectator || other.health <= 0 {
            continue;
        }
        if let Some(hit) = judge(profile, &aim, &[player_box(other)]) {
            candidates.push(Candidate {
                who: EntityRef::Player(entry.id),
                hit,
            });
        }
    }
    candidates.retain(|c| in_sight(&aim, &c.hit));
    if candidates.is_empty() {
        return;
    }
    if !profile.cleave {
        let best = candidates
            .iter()
            .enumerate()
            .max_by(|(ia, a), (ib, b)| {
                a.hit
                    .multiplier
                    .total_cmp(&b.hit.multiplier)
                    .then(b.hit.distance.total_cmp(&a.hit.distance))
                    .then(ib.cmp(ia))
            })
            .map(|(i, _)| i)
            .expect("non-empty");
        let keep = candidates.swap_remove(best);
        candidates = vec![keep];
    }
    let base = roll_damage(me);
    let origin = Some([
        state.pos[0],
        state.pos[1] + f64::from(state.height * 0.5),
        state.pos[2],
    ]);
    let attacker = Some(EntityRef::Player(me));
    for c in candidates {
        let amount = base * c.hit.multiplier;
        match c.who {
            EntityRef::Mob(id) => damage_mob(id, amount, origin, attacker),
            EntityRef::Player(id) => damage_player(id, amount.round() as i32, origin, attacker),
        }
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub(crate) fn offset_by(a: [f64; 3], b: [f32; 3]) -> [f64; 3] {
    [
        a[0] + f64::from(b[0]),
        a[1] + f64::from(b[1]),
        a[2] + f64::from(b[2]),
    ]
}

pub(crate) fn relative(a: [f64; 3], origin: [f64; 3]) -> [f32; 3] {
    [
        (a[0] - origin[0]) as f32,
        (a[1] - origin[1]) as f32,
        (a[2] - origin[2]) as f32,
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn clamp3(p: [f32; 3], min: [f32; 3], max: [f32; 3]) -> [f32; 3] {
    [
        p[0].clamp(min[0], max[0]),
        p[1].clamp(min[1], max[1]),
        p[2].clamp(min[2], max[2]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Profile = Profile {
        reach: 3.0,
        sweet: 2.0,
        arc_yaw: radians(30.0),
        arc_pitch: radians(20.0),
        peak: 1.5,
        floor: 0.5,
        cleave: true,
    };

    fn aim() -> Aim {
        Aim {
            eye: [0.0, 1.6, 0.0],
            forward: [0.0, 0.0, 1.0],
            across: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        }
    }

    fn body(x: f64, z: f64) -> Box3 {
        ([x - 0.4, 0.0, z - 0.4], [x + 0.4, 1.8, z + 0.4])
    }

    #[test]
    fn dead_on_and_close_is_the_peak_and_the_edges_are_the_floor() {
        let at = |x, z| judge(&WINDOW, &aim(), &[body(x, z)]);

        let dead_on = at(0.0, 1.5).expect("in front, in the sweet spot");
        assert!(
            (dead_on.multiplier - WINDOW.peak).abs() < 1e-4,
            "{dead_on:?}"
        );

        let far = at(0.0, 3.2).expect("still within reach");
        assert!(far.multiplier < dead_on.multiplier && far.multiplier > WINDOW.floor);
        assert!(at(0.0, 3.5).is_none(), "past the reach");

        let side = at(1.0, 2.0).expect("inside the window");
        assert!(side.multiplier < dead_on.multiplier && side.multiplier > WINDOW.floor);
        assert!(at(2.0, 2.0).is_none(), "outside the window across");
        assert!(at(0.0, 0.5).is_some(), "a tall body still fills the window");
        let low = judge(&WINDOW, &aim(), &[([-0.4, 0.0, 1.2], [0.4, 0.3, 1.8])]);
        assert!(
            low.is_none(),
            "a rabbit-sized body underfoot is below the window"
        );

        assert!(at(0.0, -1.0).is_none(), "behind the eye");

        let mut last = f32::INFINITY;
        for z in [1.0, 2.0, 2.4, 2.8] {
            let m = at(0.0, z).expect("along the ray").multiplier;
            assert!(m <= last + 1e-6, "no closer body lands softer: {z}");
            last = m;
        }
    }

    #[test]
    fn a_long_body_is_struck_along_its_whole_length() {
        let hull = MobSnapshot {
            target: None,
            conditions: Vec::new(),
            entombed: false,
            index: 0,
            kind: MobId(0),
            pos: [0.0, 0.0, 2.0],
            health: 4.0,
            id: 1,
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: 0.0,
            roll: 0.0,
            vel: [0.0; 3],
            on_ground: true,
            moving: false,
            half_width: 0.4,
            height: 0.6,
            half_length: 1.4,
        };
        let boxes = mob_boxes(&hull);
        assert!(boxes.len() >= 3, "a run of squares: {boxes:?}");
        let bow = boxes.iter().map(|b| b.0[0]).fold(f64::INFINITY, f64::min);
        let stern = boxes
            .iter()
            .map(|b| b.1[0])
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            (bow + 1.4).abs() < 1e-4 && (stern - 1.4).abs() < 1e-4,
            "{bow} {stern}"
        );
        let square = mob_boxes(&MobSnapshot {
            conditions: Vec::new(),
            half_length: 0.4,
            ..hull
        });
        assert_eq!(square.len(), 1);
    }

    #[test]
    fn a_roll_stays_inside_its_range() {
        for word in [0, u64::MAX, 0x9E37_79B9_7F4A_7C15] {
            let d = roll([2.0, 5.0], word);
            assert!((2.0..=5.0).contains(&d), "{d}");
        }
    }
}
