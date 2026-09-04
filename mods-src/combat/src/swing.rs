//! The tool swing law, and every tuned number behind it.
//!
//! While a body's MAIN hand swings one of this pack's tools, this module
//! animates the hand. In first person the ITEM carries the whole motion
//! (there is no arm mesh there) through the held-pose seam; in third person
//! bone rotations carry it — a tool's authored [`BodyCurve`] when it ships
//! one, else COMPOSE-mode rotations on the main shoulder and elbow from the
//! compiled family table, layered over the walk stride. The engine's own
//! vanilla punch for that hand is silenced by the Swing-motion claim the
//! pack publishes beside the poses — two swings layered on one hand fight
//! each other, so the claim is what makes the pack's curve the whole
//! motion.
//!
//! One pure law, both mirrors: the server tick system animates every body at
//! 20 Hz (observers replicate the answer) and the client frame hook runs the
//! same function for the local player at frame rate — a round trip earlier,
//! exactly the shield's prediction shape. The engine's eased pose lane
//! smooths both clocks toward one curve, so the tick and frame rates never
//! visibly disagree.
//!
//! Use jabs (place / throw / interact) are deliberately NOT animated here:
//! the pack claims only the Swing motion, leaving the engine's default jab
//! playing on a claimed hand, so a tool interacts exactly like any item.
//!
//! The curve DATA below is this pack's tunable surface; the state machine
//! above it is the law. Nothing outside mirrors these numbers.

use mod_sdk::animation::{self, mix3, BodyCurve, PoseCurve};
use mod_sdk::*;

/// Which tool a claimed hand is swinging.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Style {
    Pickaxe,
    Axe,
    Sword,
}

impl Style {
    pub const ALL: [Style; 3] = [Style::Pickaxe, Style::Axe, Style::Sword];

    /// The family a tool row's `kind` names — the engine's own tool
    /// vocabulary, so any pack's pickaxe, axe or sword of any tier swings
    /// here. Other kinds (shovel, shears) are nobody's in this pack.
    pub fn of_kind(kind: &str) -> Option<Style> {
        match kind {
            "pickaxe" => Some(Style::Pickaxe),
            "axe" => Some(Style::Axe),
            "sword" => Some(Style::Sword),
            _ => None,
        }
    }
}

/// What a claimed hand is doing — which curve its phase runs along. Use
/// jabs (place / throw / interact) are deliberately NOT here: the pack
/// leaves the Jab motion unclaimed, so the engine's own default jab plays
/// on a claimed hand exactly as on any other.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Act {
    /// The pickaxe's strike: mining loop, block breaks, pick attacks.
    Strike,
    /// The axe's chop: mining, breaks, attacks — the right-to-left swing.
    Chop,
    /// The sword's slash: a flat, fast cut across the body.
    Slash,
}

// ---- timing ---------------------------------------------------------------

/// The DEFAULT work window: the mining loop and its break impacts play the
/// swing over this many seconds unless the tool's first export authors a
/// `window_mine` of its own ([`Pace::mine`]). The default matches the
/// engine's dig-thunk timer (the audio layer retriggers every 0.300 s while
/// mining), so at default pace the impact frame lands on the sound. An
/// authored work window drifts from that fixed-rate thunk — accepted
/// (2026-08-31): a tenth of a second of audio drift is below notice, and
/// the pacing being authorable is worth more. Mining runs the
/// work window ALWAYS — including the first swing of a hold, which the
/// press-echo guard in [`Clock::step`] protects.
pub const MINE_SECONDS: f32 = 0.300;

/// The DEFAULT attack window — 0.75× the work speed, for weight
/// (2026-08-30) — played by a step whose export carries no `window_attack`
/// of its own (and by the compiled curves). Attack pace is authored data
/// ([`Pace::attack`], positional per combo step), because the arc itself IS
/// the attack rate — while a tool is paced the pack claims the engine
/// cooldown to zero and publishes [`Clock::bars_attack`] as the Attack
/// denial, so a slower authored arc is a slower weapon, by construction and
/// not by accident.
pub const ATTACK_SECONDS: f32 = MINE_SECONDS / 0.75;

/// How long after an attack a follow-up attack still CHAINS — plays the
/// next step of the tool's combo instead of restarting at the first.
/// Playtested at 0.6 (2026-08-30), widened to 0.8 (2026-08-31) so a beat
/// of repositioning between hits keeps the chain alive.
pub const CHAIN_SECONDS: f32 = 0.8;

/// The timing a tool's exports author, resolved by the caller: per-combo-
/// step ATTACK windows (positional like the curves — empty plays
/// [`ATTACK_SECONDS`] throughout), ONE work window for the mining loop
/// and its break impacts (mining always plays step 0, so it comes from the
/// first export's `window_mine`; the default is the dig-thunk cadence),
/// and per-step IMPACT phases — the instant each attack's motion lands
/// (positional too; empty = no step lands anything of its own, and the
/// clock never reports one).
#[derive(Copy, Clone, Debug)]
pub struct Pace<'a> {
    pub attack: &'a [f32],
    pub mine: f32,
    pub impact: &'a [f32],
}

impl Default for Pace<'_> {
    fn default() -> Self {
        Pace {
            attack: &[],
            mine: MINE_SECONDS,
            impact: &[],
        }
    }
}

impl Pace<'_> {
    fn attack_window(&self, combo: usize) -> f32 {
        if self.attack.is_empty() {
            ATTACK_SECONDS
        } else {
            self.attack[combo % self.attack.len()]
        }
    }

    fn impact_phase(&self, combo: usize) -> Option<f32> {
        if self.impact.is_empty() {
            None
        } else {
            Some(self.impact[combo % self.impact.len()])
        }
    }
}

/// The phase where an attack's arc becomes cancellable by the next attack:
/// the impact and its hold have fully played, only the recovery remains.
/// With the engine cooldown negated under the pack's pacing this boundary
/// IS the fastest re-attack — 0.72 of the step's window, so an authored
/// slower step is a slower weapon in exact proportion: every chained swing
/// keeps its whole impact and cancels only the tail.
const CANCEL_AT: f32 = HOLD_AT + HOLD_SPAN;

/// The swing a tool works with — its mining loop, its breaks and its attacks
/// all play this one curve.
pub fn swing_act(style: Style) -> Act {
    match style {
        Style::Pickaxe => Act::Strike,
        Style::Axe => Act::Chop,
        Style::Sword => Act::Slash,
    }
}

// ---- curves ---------------------------------------------------------------

/// One key of one curve. `item` is the FIRST-PERSON item offset only —
/// rotation in degrees (X, Y, Z), translation in 1/16-block px, the display
/// block's convention, relative to the item's authored hold. In third person
/// the ARM carries the motion (`shoulder`/`elbow`, Compose-mode rig degrees),
/// and the item rides the fist at its authored carry: offsetting it there
/// too would do the arm's work twice.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Key {
    /// Key time, phase `0..1`.
    t: f32,
    item: ([f32; 3], [f32; 3]),
    shoulder: [f32; 3],
    elbow: [f32; 3],
}

const fn key(t: f32, rotation: [f32; 3], px: [f32; 3], shoulder: [f32; 3], elbow: [f32; 3]) -> Key {
    Key {
        t,
        item: (rotation, px),
        shoulder,
        elbow,
    }
}

const fn rest(t: f32) -> Key {
    key(t, [0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3])
}

/// Phase where the strike / chop HITS and the hold begins, and the share of
/// the curve the hold keeps — the "last few frames" the pack exists to make
/// land. The rattle rings inside it and nothing else.
const HOLD_AT: f32 = 0.52;
const HOLD_SPAN: f32 = 0.20;

/// The pickaxe strike. DRAW 0→0.36 (head tips back-UP over the shoulder —
/// negative pitch — drawn slightly toward the camera), PLUNGE 0.36→0.52
/// (head slams DOWN — positive pitch — while the whole hand falls away into
/// the screen, the hit), the hold, home. Asymmetric on purpose: the tool
/// arrives fast and is PARKED on the target instead of already heading back.
///
/// Sign conventions in the held seat: positive X pitch points the head
/// down, +Z faces the camera, +Y is up.
const STRIKE: &[Key] = &[
    rest(0.0),
    key(
        0.36,
        [-26.0, -20.0, -10.0],
        [3.5, 2.5, 3.0],
        [30.0, 0.0, -14.0],
        [0.0, 0.0, -28.0],
    ),
    key(
        HOLD_AT,
        [50.0, 6.0, 16.0],
        [-2.0, -5.0, -4.5],
        [-20.0, 6.0, -34.0],
        [0.0, 0.0, -10.0],
    ),
    key(
        0.72,
        [46.0, 5.0, 14.0],
        [-1.0, -4.5, -4.2],
        [-16.0, 4.0, -30.0],
        [0.0, 0.0, -14.0],
    ),
    rest(1.0),
];

/// The axe chop: LOAD 0→0.30 (head turned out to the right — negative yaw —
/// and raised), SWEEP 0.30→0.52 (positive yaw carries the head ACROSS the
/// body to the left while the hand travels left and falls forward), the hard
/// stop, home. The right-to-left swing lives in the yaw sweep plus the
/// leftward x travel.
const CHOP: &[Key] = &[
    rest(0.0),
    key(
        0.30,
        [6.0, -48.0, -10.0],
        [5.5, 4.0, 2.5],
        [34.0, 20.0, -8.0],
        [0.0, 0.0, -34.0],
    ),
    key(
        HOLD_AT,
        [-18.0, 48.0, 14.0],
        [-8.0, -4.5, -5.0],
        [-18.0, -40.0, 8.0],
        [0.0, 0.0, -10.0],
    ),
    key(
        0.68,
        [-16.0, 44.0, 12.0],
        [-8.5, -4.5, -5.5],
        [-16.0, -44.0, 6.0],
        [0.0, 0.0, -14.0],
    ),
    rest(1.0),
];

/// The sword slash: a shallower LOAD out to the right (0→0.22) and a flat
/// SWEEP across the body (0.22→HOLD_AT) — the chop's arc with the rise
/// taken out, since a blade cuts level where an axe head falls. The pack
/// ships harness curves for the sword; this is the stand-in.
const SLASH: &[Key] = &[
    rest(0.0),
    key(
        0.22,
        [6.0, -42.0, -28.0],
        [5.0, 2.5, 2.0],
        [78.0, 28.0, 24.0],
        [0.0, 0.0, -22.0],
    ),
    key(
        HOLD_AT,
        [-12.0, 72.0, -52.0],
        [-9.0, -2.0, -6.5],
        [62.0, -48.0, -8.0],
        [0.0, 30.0, 0.0],
    ),
    key(
        0.66,
        [-12.0, 70.0, -50.0],
        [-9.0, -2.0, -6.5],
        [60.0, -46.0, -8.0],
        [0.0, 28.0, 0.0],
    ),
    rest(1.0),
];

/// One curve.
fn curve_of(act: Act) -> &'static [Key] {
    match act {
        Act::Strike => STRIKE,
        Act::Chop => CHOP,
        Act::Slash => SLASH,
    }
}

/// The compiled tables sample under the SHARED law
/// ([`animation::bracket`]) — the same smoothstep every authored curve,
/// harness preview, and the engine's own hand playback use.
fn sample(keys: &[Key], phase: f32) -> (&Key, &Key, f32) {
    animation::bracket(keys, |k| k.t, phase)
}

// ---- the clock ------------------------------------------------------------

/// One posing answer from the clock: the act in flight, its phase, and
/// which step of the tool's attack combo the play uses.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Play {
    pub act: Act,
    pub phase: f32,
    /// The combo step this play draws its curve from: the attack chain's
    /// position for a play an Attack edge started, `0` for everything else
    /// — the mining loop and breaks always play the first curve
    /// (mining is the first animation on repeat, by design). The pose side
    /// wraps it over however many curves the tool ships.
    pub combo: usize,
}

/// One hand's swing clock. Both mirrors run the identical rules, so a
/// prediction cannot disagree with the authority by construction.
#[derive(Debug, Default, PartialEq)]
pub struct Clock {
    /// The tool this hand would swing — any change (including to `None`)
    /// resets the clock: a swing (and its chain) belongs to the claim that
    /// started it.
    pub style: Option<Style>,
    /// The act in flight, if any, and its phase `0..1` into the curve.
    pub act: Option<Act>,
    pub phase: f32,
    /// The in-flight play's window in seconds — how the play STARTED picks
    /// it: an attack runs [`ATTACK_SECONDS`], work runs [`MINE_SECONDS`].
    seconds: f32,
    /// The in-flight play was started by an Attack edge — the only plays
    /// whose arc the spent rule protects from attack mashing.
    attacking: bool,
    /// The in-flight play's combo step (see [`Play::combo`]).
    combo: usize,
    /// An attack pressed while the arc still barred it, held for the arc's
    /// recovery: ONE deep, so a hack-and-slash mash never has to land on
    /// the cancel boundary. Dies with the claim like everything else here.
    queued: bool,
    /// How many quick consecutive attacks deep the chain is. Only Attack
    /// edges advance or reset it; the mining loop in between neither
    /// extends nor breaks a chain — the WINDOW does.
    chain: usize,
    /// Seconds since the last Attack edge (`None` = none under this claim):
    /// the chain window's clock.
    since_attack: Option<f32>,
    /// Whether the LAST step carried an attack's phase across its authored
    /// impact — set by [`Clock::step`], read by [`Clock::impact`]. A latch
    /// rather than a field on [`Play`] because the crossing can coincide
    /// with the arc's final step, which answers no play at all.
    impact_crossed: bool,
}

impl Clock {
    /// One clock step. `style` is the claimed tool in the main hand (`None`
    /// when nothing is claimed), `edge` the one-shot this tick's swing facts
    /// fired, `mining` the held-button level, `dt` the caller's clock step,
    /// and `pace` the tool's authored windows ([`Pace`]). Answers the
    /// [`Play`] while posing, `None` when the hand is idle — the caller
    /// releases the poses exactly then, and the eased pose lane carries the
    /// hand home.
    pub fn step(
        &mut self,
        style: Option<Style>,
        edge: Option<SwingKind>,
        mining: bool,
        dt: f32,
        pace: Pace,
    ) -> Option<Play> {
        if style != self.style {
            *self = Self {
                style,
                ..Self::default()
            };
        }
        self.impact_crossed = false;
        let style = self.style?;
        self.since_attack = self.since_attack.map(|since| since + dt);

        // A mining press's own click echoes as an Attack edge on the client
        // (the server never swings at a block): with the level up it IS the
        // mining, so it starts the LOOP on the dig cadence below rather
        // than a heavier attack arc — the first swing of a mining hold is
        // the mining animation, which was this seam's first shipped bug.
        let edge = match edge {
            Some(SwingKind::Attack) if mining => None,
            // Use jabs (place / throw / interact) are the ENGINE's: this
            // pack claims only the Swing motion, so the default jab plays on
            // the claimed hand and this clock must not pose against it.
            Some(SwingKind::Place | SwingKind::Throw | SwingKind::Interact) => None,
            edge => edge,
        };
        // An attack's arc is protected THROUGH its impact and hold: a
        // mid-arc attack click never restarts or chains NOW, so a mash
        // never clips the impact out of its own animation — it is QUEUED
        // (one deep) for the recovery instead. The RECOVERY past the hold
        // is cancellable — the next chained attack starts there.
        // [`Clock::bars_attack`] is this same predicate.
        let edge = match edge {
            Some(SwingKind::Attack) if self.bars_attack() => {
                self.queued = true;
                None
            }
            edge => edge,
        };
        // …and a QUEUED press fires the moment the recovery opens (or the
        // arc rests), exactly as a perfectly timed click would have: the
        // chain window is measured from the last edge, so it chains.
        let edge = match edge {
            None if self.queued && !self.bars_attack() => {
                self.queued = false;
                Some(SwingKind::Attack)
            }
            edge => edge,
        };

        if let Some(kind) = edge {
            if kind == SwingKind::Attack {
                // Attacks CHAIN: a follow-up inside the window advances the
                // combo, so mashing alternates through the tool's curves; a
                // slower click restarts the chain at its opening swing.
                self.chain = match self.since_attack {
                    Some(since) if since <= CHAIN_SECONDS => self.chain + 1,
                    _ => 0,
                };
                self.since_attack = Some(0.0);
                self.combo = self.chain;
            } else {
                self.combo = 0;
            }
            // An edge interrupts whatever is in flight with the tool's own
            // swing: the phase resyncs to the event (a break lands, so the
            // impact plays now), and a click during mining folds back into
            // the loop when its arc wraps below. Breaks are WORK and play
            // the work window; an attack plays ITS STEP's authored window.
            self.act = Some(swing_act(style));
            self.phase = 0.0;
            self.attacking = kind == SwingKind::Attack;
            self.seconds = if self.attacking {
                pace.attack_window(self.combo)
            } else {
                pace.mine
            };
        } else if self.act.is_none() && mining {
            // The held mining level starts the loop on a fresh arc — the
            // level RISE only gets here; while mining continues the arc
            // below simply keeps advancing.
            self.act = Some(swing_act(style));
            self.phase = 0.0;
            self.seconds = pace.mine;
            self.attacking = false;
            self.combo = 0;
        }

        let act = self.act?;
        let from = self.phase;
        self.phase += dt / self.seconds.max(0.01);
        // An ATTACK's motion lands the instant its phase crosses the step's
        // authored impact — once, on the step that crosses it. Work (the
        // loop, breaks) lands nothing of its own: mining's hit is the
        // engine's break timer.
        if self.attacking {
            if let Some(at) = pace.impact_phase(self.combo) {
                self.impact_crossed = from < at && at <= self.phase;
            }
        }
        if self.phase >= 1.0 {
            // The tool's own swing WRAPS while the button holds — the mining
            // loop, on the work window, back on the first curve whatever
            // play it grew out of. A released button rests instead,
            // finishing the arc home rather than
            // rewinding it.
            if mining && act == swing_act(style) {
                self.phase = self.phase.fract();
                self.seconds = pace.mine;
                self.attacking = false;
                self.combo = 0;
            } else {
                self.act = None;
                self.phase = 0.0;
                self.attacking = false;
                return None;
            }
        }
        Some(Play {
            act,
            phase: self.phase,
            combo: self.combo,
        })
    }

    /// Whether the in-flight play bars the NEXT attack: an attack's arc is
    /// protected through its impact and hold, and only its recovery may be
    /// cancelled by the follow-up. ONE predicate, two enforcers — the clock
    /// queues mid-arc attack edges behind it (both mirrors), and the server
    /// half publishes it as an Attack denial for a paced tool the engine
    /// still hits for (a tool landing its own hits keeps the press flowing
    /// so the queue can hear it). With the engine's attack
    /// cooldown negated while the pack paces a tool, this predicate IS the
    /// attack pace: damage can land exactly as often as the animation
    /// reaches its recovery.
    pub fn bars_attack(&self) -> bool {
        self.attacking && self.phase < CANCEL_AT
    }

    /// Whether the step just taken carried an attack across its authored
    /// impact phase ([`Pace::impact`]) — the instant the swing LANDS, and
    /// the moment whatever the tool is meant to strike gets struck. True
    /// for exactly one step per attack arc, never for work.
    pub fn impact(&self) -> bool {
        self.impact_crossed
    }
}

// ---- the pose law --------------------------------------------------------

/// The whole swing law as ONE pure function: `(act, phase)` → the item pose
/// in first person and the body offsets in third person. Both sides call
/// this verbatim, which is what makes a prediction that disagrees with the
/// authority impossible rather than unlikely. The caller gates on
/// [`Clock::step`] and poses nothing on a `None`. `data` is the tool's own
/// item curve when it ships one — it replaces the family's first-person
/// motion (what the harness authors is what plays, rattle included or
/// absent). `body` likewise replaces the family's THIRD-PERSON choreography
/// with authored bones. The two are orthogonal: a tool may ship either,
/// both, or neither, and the compiled tables stand in per channel.
pub fn pose(
    act: Act,
    phase: f32,
    data: Option<&PoseCurve>,
    body: Option<&BodyCurve>,
) -> (HeldPose, Vec<BonePoseData>) {
    let family = curve_of(act);
    let phase = phase.clamp(0.0, 1.0);
    let (lo, hi, u) = sample(family, phase);
    let (shoulder, elbow) = (
        mix3(lo.shoulder, hi.shoulder, u),
        mix3(lo.elbow, hi.elbow, u),
    );
    let item = match data {
        // The held-pose seam has one authored pivot and no per-key one, so
        // the sample's `origin` is dropped here (the harness folds it into
        // translation on export; the engine's own playback honors it).
        Some(curve) => {
            let s = curve.sample(phase);
            (s.rotation, s.translation)
        }
        None => {
            let mut item = (mix3(lo.item.0, hi.item.0, u), mix3(lo.item.1, hi.item.1, u));
            // The impact rattle rings only inside the hold: a tiny damped
            // camera-ward knock on the item's depth channel. A harness curve
            // does not get it — what the pack authors is what plays, to the
            // pixel; the compiled tables ARE the pack's authored feel.
            let q = (phase - HOLD_AT) / HOLD_SPAN;
            if q > 0.0 && q < 1.0 {
                item.1[2] += (std::f32::consts::TAU * 2.2 * q).sin() * 1.1 * (-6.0 * q).exp();
            }
            item
        }
    };

    let pose = HeldPose {
        first_person: HeldPoseData {
            rotation: item.0,
            translation: item.1,
        },
        third_person: HeldPoseData::IDENTITY,
    };
    // An authored body curve IS the third person; the compiled arm columns
    // stand in without one. Either way the swing rides Compose bones by
    // convention: the arm still wears the walk cycle, and a swinging body
    // that froze its stride would read as a stutter, not a stance. (A stance
    // is what the shield's guard does — and what a `replace` row in a body
    // export deliberately asks for.)
    let bones = match body {
        Some(curve) => curve.bones(phase),
        None => vec![
            BonePoseData {
                bone: bone::MAIN_SHOULDER.to_string(),
                rotation: shoulder,
                translation: [0.0; 3],
                mode: BonePoseMode::Compose,
            },
            BonePoseData {
                bone: bone::MAIN_ELBOW.to_string(),
                rotation: elbow,
                translation: [0.0; 3],
                mode: BonePoseMode::Compose,
            },
        ],
    };
    (pose, bones)
}

/// The swing claim this body's main hand carries: the pack owns the hand
/// while a tool is in it and nothing else has taken the hands. While the
/// shield's guard is up the claim releases — the guard poses those hands,
/// and its denial keeps them still anyway.
pub fn claim(style: Option<Style>, raised: bool) -> bool {
    style.is_some() && !raised
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt() -> f32 {
        1.0 / 60.0
    }

    /// Every curve starts and ends at the authored hold with a rest arm, is
    /// finite everywhere, and keeps the third-person item pose as the
    /// authored carry (the arm carries the motion there; an item offset in
    /// both views would do the arm's work twice).
    #[test]
    fn curves_leave_and_return_to_the_authored_hold() {
        for act in [Act::Strike, Act::Chop, Act::Slash] {
            for phase in [0.0, 0.05, 0.3, 0.5, HOLD_AT, 0.6, 0.8, 1.0] {
                let (pose, bones) = pose(act, phase, None, None);
                let all = pose
                    .first_person
                    .rotation
                    .into_iter()
                    .chain(pose.first_person.translation)
                    .chain(bones.iter().flat_map(|b| b.rotation))
                    .collect::<Vec<f32>>();
                assert!(all.iter().all(|c| c.is_finite()), "{act:?} at {phase}");
                assert!(
                    pose.third_person.is_identity(),
                    "third person rides the arm, not an item offset"
                );
                if phase >= 1.0 {
                    assert!(pose.first_person.is_identity(), "{act:?} ends at rest");
                } else if phase > 0.0 {
                    assert_ne!(pose, HeldPose::IDENTITY, "{act:?} moves at {phase}");
                }
            }
            for (pose, bones) in [pose(act, 0.0, None, None), pose(act, 1.0, None, None)] {
                assert!(
                    bones.iter().all(|b| b.rotation == [0.0; 3]),
                    "the arm rests at both ends"
                );
                assert!(pose.first_person.is_identity(), "and so does the item");
            }
        }
    }

    /// Work shares the dig cadence — a break edge and the mining loop
    /// advance identically at [`MINE_SECONDS`] — while an attack plays the
    /// same curve over its heavier [`ATTACK_SECONDS`] window. The split is
    /// deliberate (the weight is the point); the guards that make it safe
    /// are pinned below: the mining press's echoed edge and the arc's
    /// recovery cancel. Without them this exact split was the seam's first
    /// shipped bug.
    #[test]
    fn work_shares_the_dig_cadence_and_attacks_play_heavier() {
        let step = 0.05;
        for style in Style::ALL {
            let mut loop_hand = Clock::default();
            let looped = loop_hand
                .step(Some(style), None, true, step, Pace::default())
                .unwrap();
            let mut break_hand = Clock::default();
            let broke = break_hand
                .step(
                    Some(style),
                    Some(SwingKind::Break),
                    true,
                    step,
                    Pace::default(),
                )
                .unwrap();
            assert_eq!(
                broke, looped,
                "{style:?}: a break lands on the loop's clock"
            );

            let mut attack_hand = Clock::default();
            let attacked = attack_hand
                .step(
                    Some(style),
                    Some(SwingKind::Attack),
                    false,
                    step,
                    Pace::default(),
                )
                .unwrap();
            assert_eq!(attacked.act, looped.act, "one curve family");
            assert!(
                (attacked.phase - step / ATTACK_SECONDS).abs() < 1e-6
                    && attacked.phase < looped.phase,
                "{style:?}: the attack is the heavier, slower play"
            );
        }
    }

    /// An export's `window_attack` IS its step's attack pace (positional
    /// like the curves) and `window_mine` paces ALL the work — the mining
    /// loop, its wrap, and break impacts — while attacks never borrow the
    /// work window nor work an attack's.
    #[test]
    fn authored_windows_pace_attacks_per_step_and_mining_by_the_work_window() {
        let pace = Pace {
            attack: &[0.5, 0.25],
            mine: 0.42,
            impact: &[],
        };
        let attack = |hand: &mut Clock| {
            hand.step(Some(Style::Axe), Some(SwingKind::Attack), false, dt(), pace)
                .expect("an attack always plays")
        };
        let idle = |hand: &mut Clock, steps: usize| {
            for _ in 0..steps {
                hand.step(Some(Style::Axe), None, false, dt(), pace);
            }
        };

        let mut hand = Clock::default();
        let opening = attack(&mut hand);
        assert!(
            (opening.phase - dt() / 0.5).abs() < 1e-6,
            "step 0 plays its own authored attack window: {}",
            opening.phase
        );
        // Chain into step 1: the follow-up plays THAT step's faster window.
        idle(&mut hand, 30);
        let chained = attack(&mut hand);
        assert_eq!(chained.combo, 1);
        assert!(
            (chained.phase - dt() / 0.25).abs() < 1e-6,
            "step 1 plays its own authored attack window: {}",
            chained.phase
        );

        // Work runs the authored work window: the loop and a break edge
        // advance on it, never on a step's attack window.
        let mut work = Clock::default();
        let looped = work.step(Some(Style::Axe), None, true, dt(), pace).unwrap();
        assert!(
            (looped.phase - dt() / 0.42).abs() < 1e-6,
            "{}",
            looped.phase
        );
        let broke = Clock::default()
            .step(Some(Style::Axe), Some(SwingKind::Break), true, dt(), pace)
            .unwrap();
        assert_eq!(broke, looped, "a break lands on the work window");
    }

    /// Quick consecutive ATTACKS chain: each follow-up inside
    /// [`CHAIN_SECONDS`] plays the next combo step, a pause restarts the
    /// chain at its opening swing, and the mining loop (with its break
    /// edges) never leaves the first — combat alternates, mining repeats.
    #[test]
    fn quick_attacks_chain_the_combo_and_mining_never_does() {
        let mut hand = Clock::default();
        let attack = |hand: &mut Clock| {
            hand.step(
                Some(Style::Axe),
                Some(SwingKind::Attack),
                false,
                dt(),
                Pace::default(),
            )
            .expect("an attack always plays")
        };
        let idle = |hand: &mut Clock, steps: usize| {
            for _ in 0..steps {
                hand.step(Some(Style::Axe), None, false, dt(), Pace::default());
            }
        };

        // The first attack ever is the chain's opening swing.
        assert_eq!(attack(&mut hand).combo, 0);
        // Re-clicked as fast as the cooldown allows (~0.3 s): it chains, and
        // keeps counting — the pose side wraps it over the curves shipped.
        idle(&mut hand, 18);
        assert_eq!(attack(&mut hand).combo, 1);
        idle(&mut hand, 18);
        assert_eq!(attack(&mut hand).combo, 2);

        // A pause past the window restarts the chain.
        idle(&mut hand, 80);
        assert_eq!(attack(&mut hand).combo, 0);

        // Mining stays the FIRST animation on repeat: the loop and the break
        // edges it lands play combo 0, however fresh the last attack.
        assert_eq!(
            hand.step(Some(Style::Axe), None, true, dt(), Pace::default())
                .unwrap()
                .combo,
            0
        );
        assert_eq!(
            hand.step(
                Some(Style::Axe),
                Some(SwingKind::Break),
                true,
                dt(),
                Pace::default()
            )
            .unwrap()
            .combo,
            0
        );
    }

    /// An attack's arc is protected THROUGH its impact and hold: a mid-arc
    /// attack click neither restarts nor chains right away — it is QUEUED,
    /// one deep, and fires the moment the recovery opens, so a mash chains
    /// without having to land on the cancel boundary. A click in the
    /// RECOVERY chains at once. A claim change drops the queue.
    #[test]
    fn a_mid_arc_attack_queues_until_the_recovery_opens() {
        let mut hand = Clock::default();
        let attack = |hand: &mut Clock| {
            hand.step(
                Some(Style::Axe),
                Some(SwingKind::Attack),
                false,
                dt(),
                Pace::default(),
            )
        };
        let idle =
            |hand: &mut Clock| hand.step(Some(Style::Axe), None, false, dt(), Pace::default());
        let first = attack(&mut hand).expect("the opening swing plays");
        assert_eq!(first.combo, 0);
        assert!(hand.bars_attack(), "the fresh arc bars the next attack");

        // Mashed mid-arc: the play advances instead of restarting, no
        // chained step begins yet…
        let mashed = attack(&mut hand).expect("the arc keeps playing");
        assert_eq!(mashed.act, first.act);
        assert_eq!(mashed.combo, 0, "no chained step mid-swing");
        assert!(mashed.phase > first.phase, "the arc was not restarted");
        assert!(hand.queued, "…but the press is held");
        attack(&mut hand);
        assert!(
            hand.queued,
            "a second mid-arc press is not a second queue entry"
        );

        // …and the instant the hold has fully played, the queued press
        // fires as the chained step — no further click needed.
        while hand.bars_attack() {
            idle(&mut hand).unwrap();
        }
        let chained = idle(&mut hand).expect("the queued attack starts");
        assert_eq!(chained.combo, 1, "the queued press chains");
        assert!(chained.phase < 0.1, "a fresh arc");
        assert!(!hand.queued, "the queue is spent");
        assert!(hand.bars_attack(), "the chained arc bars in turn");

        // Nothing queued: the recovery plays out to rest on its own.
        while hand.bars_attack() {
            idle(&mut hand).unwrap();
        }
        assert!(hand.act.is_some(), "still mid-arc — only the tail remains");
        let mut at_rest = None;
        for _ in 0..200 {
            at_rest = idle(&mut hand);
            if at_rest.is_none() {
                break;
            }
        }
        assert!(at_rest.is_none(), "the tail rests without a queued press");

        // A queued press dies with the claim: switching off the weapon
        // mid-arc leaves nothing to fire.
        let mut swapped = Clock::default();
        attack(&mut swapped).unwrap();
        attack(&mut swapped);
        assert!(swapped.queued);
        assert_eq!(swapped.step(None, None, false, dt(), Pace::default()), None);
        assert!(!swapped.queued, "the swap drops the queue");
        assert_eq!(
            swapped.step(Some(Style::Axe), None, false, dt(), Pace::default()),
            None,
            "the tool comes back to an idle hand, not a stale swing"
        );

        // A mining press's echoed attack edge is the LOOP, not a heavy first
        // swing: with the level up it plays at the dig cadence from the
        // first frame — the seam's first shipped bug, pinned.
        let mut mining = Clock::default();
        assert_eq!(
            mining.step(
                Some(Style::Axe),
                Some(SwingKind::Attack),
                true,
                dt(),
                Pace::default()
            ),
            Some(Play {
                act: Act::Chop,
                phase: dt() / MINE_SECONDS,
                combo: 0,
            }),
        );
    }

    /// The clock reports an attack's authored impact on exactly ONE step —
    /// the one whose phase crosses it — and never for work, however long
    /// the loop runs: a swing that landed twice would double every hit, one
    /// that never landed would be a tool that cannot hurt, and a mining
    /// loop that landed would strike whatever stood near a wall being dug.
    #[test]
    fn an_attack_lands_its_impact_once_and_work_never_lands() {
        let pace = Pace {
            attack: &[],
            mine: MINE_SECONDS,
            impact: &[0.5, 0.25],
        };
        let mut hand = Clock::default();
        hand.step(Some(Style::Axe), Some(SwingKind::Attack), false, dt(), pace);
        let mut landed = usize::from(hand.impact());
        let mut phase_at_impact = None;
        while hand.act.is_some() {
            hand.step(Some(Style::Axe), None, false, dt(), pace);
            if hand.impact() {
                landed += 1;
                phase_at_impact = Some(hand.phase);
            }
        }
        assert_eq!(landed, 1, "one impact per arc");
        let at = phase_at_impact.expect("the crossing step");
        assert!(
            (0.5..0.5 + dt() / ATTACK_SECONDS + 1e-4).contains(&at),
            "at {at}"
        );

        // The chained step lands at ITS OWN authored phase.
        hand.step(Some(Style::Axe), Some(SwingKind::Attack), false, dt(), pace);
        let mut at = None;
        while hand.act.is_some() {
            hand.step(Some(Style::Axe), None, false, dt(), pace);
            if hand.impact() {
                at = Some(hand.phase);
            }
        }
        let at = at.expect("the chained step lands too");
        assert!(
            (0.25..0.5).contains(&at),
            "step 1 lands at its own phase: {at}"
        );

        // Work never lands, across several wraps of the loop.
        let mut work = Clock::default();
        for _ in 0..80 {
            work.step(Some(Style::Axe), None, true, dt(), pace);
            assert!(!work.impact(), "the mining loop lands nothing");
        }
        work.step(Some(Style::Axe), Some(SwingKind::Break), true, dt(), pace);
        for _ in 0..40 {
            work.step(Some(Style::Axe), None, true, dt(), pace);
            assert!(!work.impact(), "a break lands nothing of its own");
        }

        // A tool whose exports mark no impact never reports one.
        let mut quiet = Clock::default();
        quiet.step(
            Some(Style::Axe),
            Some(SwingKind::Attack),
            false,
            dt(),
            Pace::default(),
        );
        while quiet.act.is_some() {
            quiet.step(Some(Style::Axe), None, false, dt(), Pace::default());
            assert!(!quiet.impact());
        }
    }

    /// The mining level starts the loop at phase 0 on the shared cadence; a
    /// held button wraps on that ONE clock (never restarting from 0 while
    /// the level holds), and a released level finishes the arc home instead
    /// of rewinding it.
    #[test]
    fn mining_runs_the_loop_on_the_dig_cadence_and_releases_forward() {
        let frame = 0.02;
        let mut hand = Clock::default();
        // The rising level starts the loop.
        assert_eq!(
            hand.step(Some(Style::Pickaxe), None, true, frame, Pace::default()),
            Some(Play {
                act: Act::Strike,
                phase: frame / MINE_SECONDS,
                combo: 0,
            }),
        );
        let advanced = hand
            .step(Some(Style::Pickaxe), None, true, frame, Pace::default())
            .expect("a held level keeps the loop")
            .phase;
        assert!(
            (advanced - 2.0 * frame / MINE_SECONDS).abs() < 1e-4,
            "the loop advances at the swing cadence"
        );

        // A held button WRAPS (the phase cycles), never restarting from 0:
        // several turns of the clock inside this run.
        let mut wraps = 0;
        let mut last = 1.0;
        for _ in 0..60 {
            let Some(Play { act, phase, .. }) =
                hand.step(Some(Style::Pickaxe), None, true, frame, Pace::default())
            else {
                panic!("a held level never idles");
            };
            assert_eq!(act, Act::Strike);
            if phase < last {
                wraps += 1;
            }
            last = phase;
        }
        assert!(
            wraps >= 2,
            "a held level wraps on the swing cadence: {last}"
        );

        // The released level finishes the arc home: the swing plays out and
        // then the hand rests — still claimed, so the vanilla punch stays
        // silent while the tool is up.
        let mut done = Clock::default();
        done.step(Some(Style::Axe), None, true, 0.02, Pace::default());
        let mut released = None;
        for _ in 0..40 {
            released = done.step(Some(Style::Axe), None, false, 0.02, Pace::default());
            if released.is_none() {
                break;
            }
        }
        assert!(released.is_none(), "the released loop finishes and rests");
    }

    /// Edges map to curves per tool: the axe attacks and breaks with its
    /// chop, the pickaxe with its strike; a use click is not this clock's.
    /// And a tool swap resets the clock rather than carrying the last item's
    /// curve into new art.
    #[test]
    fn edges_map_to_their_tools_curves() {
        let mut axe = Clock::default();
        let chopped = axe.step(
            Some(Style::Axe),
            Some(SwingKind::Attack),
            false,
            dt(),
            Pace::default(),
        );
        assert_eq!(
            chopped,
            Some(Play {
                act: Act::Chop,
                phase: dt() / ATTACK_SECONDS,
                combo: 0,
            }),
            "the axe attacks with the chop"
        );

        let mut pick = Clock::default();
        assert_eq!(
            pick.step(
                Some(Style::Pickaxe),
                Some(SwingKind::Break),
                false,
                dt(),
                Pace::default()
            ),
            Some(Play {
                act: Act::Strike,
                phase: dt() / MINE_SECONDS,
                combo: 0,
            }),
            "the pickaxe breaks with the strike, on the dig cadence"
        );

        // A use click of any kind is the ENGINE's jab, not this clock's:
        // the edge is ignored, the hand stays idle, and the vanilla jab
        // (a motion this pack leaves unclaimed) plays over the authored
        // hold.
        for kind in [SwingKind::Place, SwingKind::Throw, SwingKind::Interact] {
            let mut any = Clock::default();
            assert_eq!(
                any.step(
                    Some(Style::Pickaxe),
                    Some(kind),
                    false,
                    dt(),
                    Pace::default()
                ),
                None,
                "a {kind:?} click starts no pack curve"
            );
        }

        // Swapping tools mid-swing restarts clean under the new claim.
        axe.step(Some(Style::Pickaxe), None, false, dt(), Pace::default());
        assert_eq!(axe.style, Some(Style::Pickaxe));
        assert!(axe.act.is_none(), "the pickaxe's clock starts fresh");
    }

    /// The claim follows the tool and yields to the guard: held tool, hands
    /// unclaimed elsewhere — a raised guard poses and stills those hands, and
    /// the claim must not argue with it.
    #[test]
    fn the_claim_follows_the_tool_and_yields_to_the_guard() {
        assert!(claim(Some(Style::Axe), false));
        assert!(!claim(None, false), "a bare hand stays vanilla");
        assert!(!claim(Some(Style::Pickaxe), true), "a raised guard owns it");
        assert!(!claim(None, true));
    }
}

#[cfg(test)]
mod harness_tests {
    use super::*;

    /// A data curve replaces the family's ITEM motion but NOT the arm —
    /// UNLESS the tool also ships a body export: without one the
    /// third-person body still composes the pack's compiled arm
    /// choreography. Synthetic curves, deliberately: the shipped exports are
    /// authored art, re-authored at will, and a test pinning their numbers
    /// would break on every re-save (the parse mechanics live with the
    /// parser, in mod-api).
    #[test]
    fn a_data_curve_drives_the_item_and_the_arm_stays_the_packs() {
        let curve = PoseCurve::from_harness(
            "{\"format\": \"petramond-swing-animation\", \"version\": 1, \"keys\": [\
             {\"t\": 0, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}, \
             {\"t\": 0.5, \"rotation\": [12.5, -51.5, -68.0], \"translation\": [5.5, 0, 0]}, \
             {\"t\": 1, \"rotation\": [0, 0, 0], \"translation\": [0, 0, 0]}]}",
        )
        .expect("a valid synthetic curve");
        let (pose, bones) = pose(Act::Chop, 0.5, Some(&curve), None);
        assert_eq!(
            pose.first_person.rotation,
            [12.5, -51.5, -68.0],
            "the data curve IS the first-person motion"
        );
        // The arm: the compiled chop's own motion, not zeros — the third
        // person must move even when the item curve is data.
        assert!(
            bones
                .iter()
                .find(|b| b.bone == bone::MAIN_SHOULDER)
                .is_some_and(|b| b.rotation != [0.0; 3]),
            "the arm law keeps composing under a data curve"
        );
    }

    /// An authored BODY curve replaces the compiled arm wholesale: inside
    /// [`pose`] the curve's own rows are the whole third person, exactly as
    /// the curve samples them.
    #[test]
    fn an_authored_body_curve_is_the_whole_third_person() {
        let body = BodyCurve::from_harness(
            "{\"format\": \"petramond-player-animation\", \"version\": 1, \
             \"bones\": [{\"name\": \"left_shoulder\", \"mode\": \"compose\"}], \
             \"keys\": [{\"t\": 0, \"pose\": {}}, {\"t\": 1, \"pose\": \
             {\"left_shoulder\": {\"rotation\": [40, 0, 0], \"translation\": [0, 0, 0]}}}]}",
        )
        .expect("a valid synthetic body curve");
        let (_, bones) = pose(Act::Strike, 0.7, None, Some(&body));
        assert_eq!(bones, body.bones(0.7));
        assert_eq!(bones.len(), 1, "only the curve's declared bones pose");
    }
}
