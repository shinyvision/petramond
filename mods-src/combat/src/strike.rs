//! The strike law: WHERE a swing lands and HOW HARD.
//!
//! The engine's own melee lands a hit on the click, on whatever the
//! crosshair held. A tool this pack animates does neither: its press is
//! claimed (`attack_attempt`), and the hit lands the instant the swing's
//! authored IMPACT plays — from where the attacker is looking THEN, at
//! every body the family's strike window reaches. If the axe looks like it
//! hit, it hit; a body that stepped out of the arc was missed; the crosshair
//! is not consulted at all.
//!
//! How hard is a product of two things a fighter controls: how CLOSE the
//! body is (full strength inside the family's sweet spot, tapering toward
//! the reach's end) and how DEAD-ON the swing is (the angular miss of the
//! look ray past the body, across and up-down, each against the family's
//! window). A dead-on hit in the sweet spot lands more than the tool's
//! plain damage roll; a glancing one at the edge of the window, far less.
//! An axe SWEEPS — every body in its wide, flat window is struck; a pickaxe
//! PLUNGES — tall and narrow, and only the best-placed body takes it.
//!
//! [`judge`] is pure: the numbers, the attacker's aiming frame, the bodies.
//! [`land`] runs it over the live world and lands the verdicts through the
//! engine's damage funnel, naming the attacker so the victim remembers, the
//! knockback shoves, and every `mob_damage_pre` handler sees exactly the
//! strike the engine's own hit would have shown it.

use crate::swing::Style;
use mod_sdk::*;

/// How a family's swing reaches, and what it does when it does.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Profile {
    /// Farthest a body's closest point may be from the EYE and still be
    /// struck (blocks).
    pub reach: f32,
    /// Up to this distance the hit is full strength; past it the strength
    /// tapers linearly to `floor` at `reach`.
    pub sweet: f32,
    /// Half-angles (radians) of the strike window either side of the look
    /// ray: ACROSS the body and UP-DOWN. A wide, flat window is a sweep; a
    /// narrow, tall one a plunge.
    pub arc_yaw: f32,
    pub arc_pitch: f32,
    /// The damage multiplier of a dead-on hit inside the sweet spot.
    pub peak: f32,
    /// The multiplier at the very edge of the window or of the reach.
    pub floor: f32,
    /// Whether one swing strikes EVERY body in its window (a sweep) or only
    /// the best-placed one (a plunge).
    pub cleave: bool,
}

const fn radians(degrees: f32) -> f32 {
    degrees * (std::f32::consts::PI / 180.0)
}

/// The axe: a flat sweep across the body — wide enough to cleave a huddle
/// in front of you, not so wide that something at the edge of your vision
/// takes a glancing hit (2026-09-02: ±55° was that; tightened) — strongest
/// with the target a stride away rather than at the fist.
const AXE: Profile = Profile {
    reach: 3.5,
    sweet: 2.0,
    arc_yaw: radians(32.0),
    arc_pitch: radians(22.0),
    peak: 1.25,
    floor: 0.35,
    cleave: true,
};

/// The pickaxe: a narrow, tall plunge — dead-on and close, or barely at
/// all — on one body.
const PICKAXE: Profile = Profile {
    reach: 3.2,
    sweet: 1.4,
    arc_yaw: radians(14.0),
    arc_pitch: radians(40.0),
    peak: 1.35,
    floor: 0.35,
    cleave: false,
};

/// The sword: a level cut — the axe's sweep, a little tighter and a little
/// shorter (a blade, not a haft), and it lands more evenly across its reach
/// because a fast weapon is meant to be swung often, not lined up.
const SWORD: Profile = Profile {
    reach: 3.0,
    sweet: 1.8,
    arc_yaw: radians(28.0),
    arc_pitch: radians(20.0),
    peak: 1.15,
    floor: 0.45,
    cleave: true,
};

/// The family's profile.
pub fn profile(style: Style) -> Profile {
    match style {
        Style::Axe => AXE,
        Style::Pickaxe => PICKAXE,
        Style::Sword => SWORD,
    }
}

/// Nearer than this along the look ray the angular miss is measured as if
/// the body were here: a body standing inside the attacker names no angle.
const NEAR: f32 = 0.3;

/// The attacker's aiming frame at the instant of the strike: the eye and an
/// orthonormal look basis. The player yaw convention (forward is
/// `(sin yaw, cos yaw)`, pitch tips it) is decided here and nowhere else.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Aim {
    pub eye: [f32; 3],
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
            eye: [state.pos[0], state.pos[1] + state.eye_height, state.pos[2]],
            forward,
            across,
            up: cross(across, forward),
        }
    }
}

/// One world-space box of a body: `(min, max)`.
pub type Box3 = ([f32; 3], [f32; 3]);

/// A mob's body as the engine collides and targets it: one square box, or —
/// for a long body — a run of overlapping squares along its facing, the
/// same run the engine's own ray validator tests.
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
            let c = add(m.pos, scale(facing, offset));
            (
                [c[0] - hw, c[1], c[2] - hw],
                [c[0] + hw, c[1] + m.height, c[2] + hw],
            )
        })
        .collect()
}

/// A player's body box.
pub fn player_box(p: &PlayerSnapshot) -> Box3 {
    (
        [p.pos[0] - p.half_width, p.pos[1], p.pos[2] - p.half_width],
        [
            p.pos[0] + p.half_width,
            p.pos[1] + p.height,
            p.pos[2] + p.half_width,
        ],
    )
}

/// The verdict on one body inside the window.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Hit {
    /// What the tool's damage roll is scaled by.
    pub multiplier: f32,
    /// The body's closest point to the eye — where the sightline is tested.
    pub point: [f32; 3],
    pub distance: f32,
}

/// Judge one body (its boxes) against the profile from the aiming frame:
/// the best of its boxes, or `None` when no box is inside the window and
/// the reach.
pub fn judge(profile: &Profile, aim: &Aim, boxes: &[Box3]) -> Option<Hit> {
    boxes
        .iter()
        .filter_map(|&(min, max)| judge_box(profile, aim, min, max))
        .max_by(|a, b| a.multiplier.total_cmp(&b.multiplier))
}

fn judge_box(profile: &Profile, aim: &Aim, min: [f32; 3], max: [f32; 3]) -> Option<Hit> {
    let nearest = clamp3(aim.eye, min, max);
    let distance = length(sub(nearest, aim.eye));
    // Out of reach, or not in FRONT at all: the angular miss below is
    // measured across the look ray and cannot see a body behind the eye.
    if distance > profile.reach || dot(sub(nearest, aim.eye), aim.forward) < 0.0 {
        return None;
    }
    // Where the look ray passes the box, by alternating projection: the
    // ray's point nearest the box, the box's point nearest that, again.
    // Two rounds settle it to well under a degree for a body-sized box.
    let centre = scale(add(min, max), 0.5);
    let mut t = dot(sub(centre, aim.eye), aim.forward).clamp(NEAR, profile.reach);
    for _ in 0..2 {
        let on_ray = add(aim.eye, scale(aim.forward, t));
        let on_box = clamp3(on_ray, min, max);
        t = dot(sub(on_box, aim.eye), aim.forward).clamp(NEAR, profile.reach);
    }
    let on_ray = add(aim.eye, scale(aim.forward, t));
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
        point: nearest,
        distance,
    })
}

/// Whether a straight line from the eye to `hit.point` is clear of anything
/// a body collides with. A mob's closest point can sit a hair inside the
/// block it leans on; a hit that far along the line is the body, not a wall.
fn in_sight(aim: &Aim, hit: &Hit) -> bool {
    if hit.distance <= NEAR {
        return true;
    }
    let dir = sub(hit.point, aim.eye);
    raycast(aim.eye, dir, hit.distance, RayFilter::Collidable)
        .is_none_or(|block| block.distance >= hit.distance - 0.05)
}

/// One body the swing could strike, judged.
struct Candidate {
    who: EntityRef,
    hit: Hit,
}

/// One hit's damage: a uniform roll in `range` off a host random word —
/// the pack's one derivation, shared by every strike that rolls.
pub fn roll(range: [f32; 2], word: u64) -> f32 {
    let u = (word >> 11) as f32 / (1u64 << 53) as f32;
    range[0] + (range[1] - range[0]) * u
}

/// The weapon's damage roll for this strike: the HELD stack's tool range —
/// resolved as the stack carries it, so an augment's override counts
/// exactly as it does for the engine's own hit — rolled off the pack's
/// seeded stream. A hand with no tool row punches for one.
fn roll_damage(me: PlayerId) -> f32 {
    let range = player_held(me)
        .and_then(|stack| stack_info(&stack))
        .and_then(|info| info.tool.map(|t| t.damage))
        .unwrap_or([1.0, 1.0]);
    roll(range, rng_u64("strike"))
}

/// SERVER: land `me`'s swing of a `style` tool — its impact just played —
/// on every body the family's window reaches and can be seen: mobs and
/// other players alike, through the engine's funnel with `me` named as the
/// attacker. Nothing to land is an ordinary miss.
pub fn land(me: PlayerId, style: Style, state: &PlayerSnapshot) {
    if state.spectator || state.health <= 0 {
        return;
    }
    let profile = profile(style);
    let aim = Aim::of(state);
    let mut candidates: Vec<Candidate> = Vec::new();
    // A generous radius of FEET positions; the window and reach do the
    // real judging against the bodies themselves.
    let radius = profile.reach + 4.0;
    for mob in mobs_in_radius(state.pos, radius) {
        if let Some(hit) = judge(&profile, &aim, &mob_boxes(&mob)) {
            // A swing from the saddle is not a swing AT the saddle: the
            // engine's own crosshair never targets the attacker's mount
            // either.
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
        if let Some(hit) = judge(&profile, &aim, &[player_box(other)]) {
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
        // The plunge takes ONE body: the best-placed. Ties break toward
        // the nearer, then the first enumerated (deterministic).
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
        state.pos[1] + state.height * 0.5,
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

// ---- small vector arithmetic ---------------------------------------------

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
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

    /// A synthetic window: 3 blocks of reach, full strength inside 2, 30°
    /// across and 20° up-down, peak 1.5, floor 0.5.
    const WINDOW: Profile = Profile {
        reach: 3.0,
        sweet: 2.0,
        arc_yaw: radians(30.0),
        arc_pitch: radians(20.0),
        peak: 1.5,
        floor: 0.5,
        cleave: true,
    };

    /// Looking straight down +Z from an eye at (0, 1.6, 0).
    fn aim() -> Aim {
        Aim {
            eye: [0.0, 1.6, 0.0],
            forward: [0.0, 0.0, 1.0],
            across: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        }
    }

    /// A 0.8-wide, 1.8-tall body with its feet at `(x, 0, z)`.
    fn body(x: f32, z: f32) -> Box3 {
        ([x - 0.4, 0.0, z - 0.4], [x + 0.4, 1.8, z + 0.4])
    }

    /// The law's shape, on synthetic numbers: dead-on inside the sweet spot
    /// is the PEAK; the same body sidestepped to the window's edge, or
    /// pushed to the reach's end, drops toward the FLOOR; outside either the
    /// swing misses; and a body behind the eye is never struck however
    /// close. The multipliers between are what the fight is about — a
    /// rule that stopped grading distance or aim would flatten it.
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

        // Sidestepping the same body out toward the window's edge grades
        // down and finally misses.
        let side = at(1.0, 2.0).expect("inside the window");
        assert!(side.multiplier < dead_on.multiplier && side.multiplier > WINDOW.floor);
        assert!(at(2.0, 2.0).is_none(), "outside the window across");
        // A body at the feet, right in front, is a steep pitch miss for a
        // flat window.
        assert!(at(0.0, 0.5).is_some(), "a tall body still fills the window");
        let low = judge(&WINDOW, &aim(), &[([-0.4, 0.0, 1.2], [0.4, 0.3, 1.8])]);
        assert!(
            low.is_none(),
            "a rabbit-sized body underfoot is below the window"
        );

        assert!(at(0.0, -1.0).is_none(), "behind the eye");

        // Monotone in distance along the ray.
        let mut last = f32::INFINITY;
        for z in [1.0, 2.0, 2.4, 2.8] {
            let m = at(0.0, z).expect("along the ray").multiplier;
            assert!(m <= last + 1e-6, "no closer body lands softer: {z}");
            last = m;
        }
    }

    /// A long body is judged as the run of boxes the engine collides it
    /// with: its bow is as strikeable as its middle.
    #[test]
    fn a_long_body_is_struck_along_its_whole_length() {
        let hull = MobSnapshot {
            index: 0,
            kind: MobId(0),
            pos: [0.0, 0.0, 2.0],
            health: 4.0,
            id: 1,
            yaw: std::f32::consts::FRAC_PI_2, // facing -X
            vel: [0.0; 3],
            on_ground: true,
            moving: false,
            half_width: 0.4,
            height: 0.6,
            half_length: 1.4,
        };
        let boxes = mob_boxes(&hull);
        assert!(boxes.len() >= 3, "a run of squares: {boxes:?}");
        let bow = boxes.iter().map(|b| b.0[0]).fold(f32::INFINITY, f32::min);
        let stern = boxes
            .iter()
            .map(|b| b.1[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            (bow + 1.4).abs() < 1e-4 && (stern - 1.4).abs() < 1e-4,
            "{bow} {stern}"
        );
        let square = mob_boxes(&MobSnapshot {
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
