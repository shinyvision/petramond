//! The tool swing law.
//!
//! While a body's MAIN hand swings one of this pack's tools, this module
//! animates the hand: it holds the engine's player-rig CLIPS for the play
//! in flight in each rig's claim slot (`set_player_animator_plays`) — the
//! viewmodel's arms in first person, the body in third — scrubbed at the
//! pack's own clock, so the frame every mirror draws is the frame the clock
//! is at and a hit landing at the clip's impact lands where it is seen. The
//! engine's own swing for that hand is silenced by the Swing-motion claim
//! the pack publishes beside it — two swings on one hand fight each other,
//! so the claim is what makes the pack's clock the whole motion.
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
//! Every number the law runs on — which clips, the windows, where the arc
//! opens to the next attack — is the family's row ([`crate::families`]);
//! the state machine here is the law.

use crate::families::Family;
pub use crate::families::Style;
use mod_sdk::*;

/// How long after an attack a follow-up attack still CHAINS — plays the
/// next step of the tool's combo instead of restarting at the first.
/// Playtested at 0.6 (2026-08-30), widened to 0.8 (2026-08-31) so a beat
/// of repositioning between hits keeps the chain alive.
pub const CHAIN_SECONDS: f32 = 0.8;

/// The slot both engine rigs declare for a mod's main-hand play.
pub const CLAIM_SLOT: &str = "main_claim";

// ---- the clock ------------------------------------------------------------

/// One posing answer from the clock: the play's phase and which step of the
/// tool's attack combo it uses.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Play {
    pub phase: f32,
    /// The combo step this play draws its clips from: the attack chain's
    /// position for a play an Attack edge started, `0` for everything else
    /// — the mining loop and breaks always play the work loop (mining is
    /// the first animation on repeat, by design). The pose side wraps it
    /// over however many steps the family ships.
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
    /// Whether a play is in flight, and its phase `0..1` into the clip.
    pub playing: bool,
    pub phase: f32,
    /// The in-flight play's window in seconds — how the play STARTED picks
    /// it: an attack runs its step's window, work runs the family's.
    seconds: f32,
    /// The in-flight play was started by an Attack edge — the only plays
    /// whose arc the spent rule protects from attack mashing.
    attacking: bool,
    /// The phase the in-flight attack opens its recovery to the next
    /// attack ([`Family::cancel_at`] of its step).
    cancel_at: f32,
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
    /// Whether the LAST step carried an attack's phase across its impact —
    /// set by [`Clock::step`], read by [`Clock::impact`]. A latch rather
    /// than a field on [`Play`] because the crossing can coincide with the
    /// arc's final step, which answers no play at all.
    impact_crossed: bool,
}

impl Clock {
    /// One clock step. `claim` is the claimed tool in the main hand and its
    /// family (`None` when nothing is claimed), `edge` the one-shot this
    /// tick's swing facts fired, `mining` the held-button level, `dt` the
    /// caller's clock step. Answers the [`Play`] while posing, `None` when
    /// the hand is idle — the caller releases the poses exactly then, and
    /// the eased pose lane carries the hand home.
    pub fn step(
        &mut self,
        claim: Option<(Style, &Family)>,
        edge: Option<SwingKind>,
        mining: bool,
        dt: f32,
    ) -> Option<Play> {
        let style = claim.map(|(style, _)| style);
        if style != self.style {
            *self = Self {
                style,
                ..Self::default()
            };
        }
        self.impact_crossed = false;
        let (_, family) = claim?;
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
                // combo, so mashing alternates through the tool's steps; a
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
            // the work window; an attack plays ITS STEP's window.
            self.playing = true;
            self.phase = 0.0;
            self.attacking = kind == SwingKind::Attack;
            self.seconds = if self.attacking {
                family.attack_window(self.combo)
            } else {
                family.pace.mine
            };
            self.cancel_at = family.cancel_at(self.combo);
        } else if !self.playing && mining {
            // The held mining level starts the loop on a fresh arc — the
            // level RISE only gets here; while mining continues the arc
            // below simply keeps advancing.
            self.playing = true;
            self.phase = 0.0;
            self.seconds = family.pace.mine;
            self.attacking = false;
            self.combo = 0;
        }

        if !self.playing {
            return None;
        }
        let from = self.phase;
        self.phase += dt / self.seconds.max(0.01);
        // An ATTACK's motion lands the instant its phase crosses the step's
        // impact — once, on the step that crosses it. Work (the loop,
        // breaks) lands nothing of its own: mining's hit is the engine's
        // break timer.
        if self.attacking {
            if let Some(at) = family.impact_phase(self.combo) {
                self.impact_crossed = from < at && at <= self.phase;
            }
        }
        if self.phase >= 1.0 {
            // The tool's own swing WRAPS while the button holds — the mining
            // loop, on the work window, back on the work loop whatever
            // play it grew out of. A released button rests instead,
            // finishing the arc home rather than rewinding it.
            if mining {
                self.phase = self.phase.fract();
                self.seconds = family.pace.mine;
                self.attacking = false;
                self.combo = 0;
            } else {
                self.playing = false;
                self.phase = 0.0;
                self.attacking = false;
                return None;
            }
        }
        Some(Play {
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
    /// so the queue can hear it). With the engine's attack cooldown negated
    /// while the pack paces a tool, this predicate IS the attack pace:
    /// damage can land exactly as often as the animation reaches its
    /// recovery.
    pub fn bars_attack(&self) -> bool {
        self.attacking && self.phase < self.cancel_at
    }

    /// Whether the play in flight was started by an Attack edge — else it
    /// is work (the mining loop, a break).
    pub fn attacking(&self) -> bool {
        self.attacking
    }

    /// Whether the step just taken carried an attack across its impact
    /// phase ([`Family::impact_phase`]) — the instant the swing LANDS, and
    /// the moment whatever the tool is meant to strike gets struck. True
    /// for exactly one step per attack arc, never for work.
    pub fn impact(&self) -> bool {
        self.impact_crossed
    }
}

// ---- the animation law ----------------------------------------------------

/// A phase on one clip's timeline moved onto another's, piecewise-linear
/// through the two clips' impact phases: `from` maps to `to`, both ends
/// stay put. Identity when either impact is not strictly inside the clip.
pub fn remap(phase: f32, from: f32, to: f32) -> f32 {
    let inside = |p: f32| p > 0.0 && p < 1.0;
    if !(inside(from) && inside(to)) {
        return phase;
    }
    if phase <= from {
        phase * to / from
    } else {
        to + (phase - from) * (1.0 - to) / (1.0 - from)
    }
}

/// The whole swing law as ONE pure function: a play → the clips its hand
/// runs on both rigs in each rig's claim slot. The clock runs on the BODY
/// clip's timeline (its impact is where the hit lands); the first-person
/// clip is scrubbed through [`remap`] so the frame the wielder sees land is
/// the one that lands. Both sides call it verbatim, so every mirror draws
/// the frame the clock is at. The caller gates on [`Clock::step`] and
/// animates nothing on a `None`.
pub fn plays(family: &Family, play: Play, attacking: bool) -> Vec<AnimatorPlay> {
    let progress = play.phase.clamp(0.0, 1.0);
    let (motion, first_person_progress) = if attacking {
        let step = play.combo % family.attacks.len();
        let fp = match (family.impact_phase(step), family.fp_impacts.get(step).copied().flatten()) {
            (Some(body), Some(fp)) => remap(progress, body, fp),
            _ => progress,
        };
        (&family.attacks[step], fp)
    } else {
        (&family.work, progress)
    };
    vec![
        AnimatorPlay::scrubbed(
            rig::PLAYER_FIRST_PERSON,
            CLAIM_SLOT,
            &motion.first_person,
            first_person_progress,
        ),
        AnimatorPlay::scrubbed(rig::PLAYER_BODY, CLAIM_SLOT, &motion.body, progress),
    ]
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
    use crate::families::{Motion, Pace};
    use crate::strike::Profile;

    const AXE: Style = Style(0);
    const PICK: Style = Style(1);

    fn dt() -> f32 {
        1.0 / 60.0
    }

    /// A synthetic family: two attack steps, one work loop.
    fn family(attack: &[f32], mine: f32, cancel_at: f32, impact: &[f32]) -> Family {
        let motion = |name: &str| Motion {
            first_person: format!("t:fp_{name}"),
            body: format!("t:body_{name}"),
        };
        Family {
            kind: "test".into(),
            attacks: vec![motion("a"), motion("b")],
            work: motion("work"),
            pace: Pace {
                attack: attack.to_vec(),
                mine,
                cancel_at,
            },
            profile: Profile {
                reach: 3.0,
                sweet: 2.0,
                arc_yaw: 0.5,
                arc_pitch: 0.3,
                peak: 1.5,
                floor: 0.5,
                cleave: true,
            },
            impacts: impact.to_vec(),
            fp_impacts: vec![None; 2],
        }
    }

    /// The plain family: 0.4 s attacks, 0.3 s work, the recovery at 0.72,
    /// no impacts.
    fn plain() -> Family {
        family(&[0.4], 0.3, 0.72, &[])
    }

    /// A play's clips follow the clock: an attack runs its combo step
    /// (wrapping over however many the family ships), work always runs the
    /// family's loop, each rig gets its own clip in the claim slot, and the
    /// scrubbed progress is the phase, kept inside the clip.
    #[test]
    fn plays_run_their_combo_steps_clips_and_work_runs_the_loop() {
        let family = plain();
        let play = |phase: f32, combo: usize| Play { phase, combo };
        let by_rig = |plays: &[AnimatorPlay], rig: &str| {
            plays
                .iter()
                .find(|p| p.rig == rig)
                .cloned()
                .expect("one play per rig")
        };
        for step in 0..family.attacks.len() * 2 {
            let motion = &family.attacks[step % family.attacks.len()];
            let got = plays(&family, play(0.4, step), true);
            assert_eq!(got.len(), 2);
            let first = by_rig(&got, rig::PLAYER_FIRST_PERSON);
            assert_eq!(
                (first.clip.as_str(), first.slot.as_str()),
                (motion.first_person.as_str(), CLAIM_SLOT),
                "step {step}"
            );
            assert_eq!(first.clock, AnimatorClock::Scrub(0.4));
            assert!(!first.mirror, "the main hand plays unmirrored");
            let body = by_rig(&got, rig::PLAYER_BODY);
            assert_eq!((body.clip.as_str(), body.slot.as_str()), (motion.body.as_str(), CLAIM_SLOT));
        }
        let work = plays(&family, play(1.3, 1), false);
        assert_eq!(
            by_rig(&work, rig::PLAYER_FIRST_PERSON).clip,
            family.work.first_person,
            "work ignores the step"
        );
        assert_eq!(
            by_rig(&work, rig::PLAYER_BODY).clock,
            AnimatorClock::Scrub(1.0),
            "progress stays inside the clip"
        );
    }

    /// The first-person clip is scrubbed so its OWN impact frame shows the
    /// instant the body's impact lands: the clock's phase (the body's
    /// timeline) maps through the two impacts, straight on either side, the
    /// ends pinned. A step whose viewmodel clip marks no impact, or a
    /// family that lands nothing, scrubs both rigs at one phase.
    #[test]
    fn the_viewmodel_is_scrubbed_through_the_impact_remap() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(close(remap(0.42, 0.42, 0.47), 0.47), "impact to impact");
        assert!(close(remap(0.0, 0.42, 0.47), 0.0));
        assert!(close(remap(1.0, 0.42, 0.47), 1.0));
        assert!(close(remap(0.21, 0.42, 0.47), 0.235), "linear before");
        assert!(close(remap(0.71, 0.42, 0.47), 0.735), "linear after");
        assert!(close(remap(0.5, 0.5, 0.486), 0.486), "and the other way");
        for (from, to) in [(0.0, 0.5), (1.0, 0.5), (0.5, 0.0), (0.5, 1.0)] {
            assert!(close(remap(0.3, from, to), 0.3), "an impact on an end is no map");
        }
        // Monotone: a scrub never runs backwards through the map.
        let mut last = -1.0;
        for i in 0..=100 {
            let p = remap(i as f32 / 100.0, 0.42, 0.47);
            assert!(p >= last, "{p} after {last}");
            last = p;
        }

        let mut family = family(&[0.4], 0.3, 0.72, &[0.5, 0.25]);
        family.fp_impacts = vec![Some(0.6), None];
        let fp = |family: &Family, phase: f32, combo: usize, attacking: bool| match plays(family, Play { phase, combo }, attacking)[0].clock {
            AnimatorClock::Scrub(p) => p,
            other => panic!("{other:?}"),
        };
        assert!(close(fp(&family, 0.5, 0, true), 0.6), "step 0 lands on its own frame");
        assert!(close(fp(&family, 0.25, 1, true), 0.25), "no viewmodel marker, no map");
        assert!(close(fp(&family, 0.5, 0, false), 0.5), "work is never remapped");
        family.impacts.clear();
        assert!(close(fp(&family, 0.5, 0, true), 0.5), "nothing lands, nothing maps");
    }

    /// Work shares the dig cadence — a break edge and the mining loop
    /// advance identically on the work window — while an attack plays its
    /// step's own heavier window. The guards that make the split safe are
    /// pinned below: the mining press's echoed edge and the arc's recovery
    /// cancel. Without them this exact split was the seam's first shipped
    /// bug.
    #[test]
    fn work_shares_the_dig_cadence_and_attacks_play_their_own_window() {
        let step = 0.05;
        let family = plain();
        let mut loop_hand = Clock::default();
        let looped = loop_hand
            .step(Some((AXE, &family)), None, true, step)
            .unwrap();
        let mut break_hand = Clock::default();
        let broke = break_hand
            .step(Some((AXE, &family)), Some(SwingKind::Break), true, step)
            .unwrap();
        assert_eq!(broke, looped, "a break lands on the loop's clock");

        let mut attack_hand = Clock::default();
        let attacked = attack_hand
            .step(Some((AXE, &family)), Some(SwingKind::Attack), false, step)
            .unwrap();
        assert!(
            (attacked.phase - step / 0.4).abs() < 1e-6 && attacked.phase < looped.phase,
            "the attack is the heavier, slower play"
        );
    }

    /// A step's `window_attack` IS its attack pace (positional, wrapping)
    /// and `window_mine` paces ALL the work — the mining loop, its wrap,
    /// and break impacts — while attacks never borrow the work window nor
    /// work an attack's.
    #[test]
    fn authored_windows_pace_attacks_per_step_and_mining_by_the_work_window() {
        let family = family(&[0.5, 0.25], 0.42, 0.72, &[]);
        let attack = |hand: &mut Clock| {
            hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt())
                .expect("an attack always plays")
        };
        let idle = |hand: &mut Clock, steps: usize| {
            for _ in 0..steps {
                hand.step(Some((AXE, &family)), None, false, dt());
            }
        };

        let mut hand = Clock::default();
        let opening = attack(&mut hand);
        assert!(
            (opening.phase - dt() / 0.5).abs() < 1e-6,
            "step 0 plays its own attack window: {}",
            opening.phase
        );
        // Chain into step 1: the follow-up plays THAT step's faster window.
        idle(&mut hand, 30);
        let chained = attack(&mut hand);
        assert_eq!(chained.combo, 1);
        assert!(
            (chained.phase - dt() / 0.25).abs() < 1e-6,
            "step 1 plays its own attack window: {}",
            chained.phase
        );

        // Work runs the work window: the loop and a break edge advance on
        // it, never on a step's attack window.
        let mut work = Clock::default();
        let looped = work.step(Some((AXE, &family)), None, true, dt()).unwrap();
        assert!((looped.phase - dt() / 0.42).abs() < 1e-6, "{}", looped.phase);
        let broke = Clock::default()
            .step(Some((AXE, &family)), Some(SwingKind::Break), true, dt())
            .unwrap();
        assert_eq!(broke, looped, "a break lands on the work window");
    }

    /// Quick consecutive ATTACKS chain: each follow-up inside
    /// [`CHAIN_SECONDS`] plays the next combo step, a pause restarts the
    /// chain at its opening swing, and the mining loop (with its break
    /// edges) never leaves the first — combat alternates, mining repeats.
    #[test]
    fn quick_attacks_chain_the_combo_and_mining_never_does() {
        let family = plain();
        let mut hand = Clock::default();
        let attack = |hand: &mut Clock| {
            hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt())
                .expect("an attack always plays")
        };
        let idle = |hand: &mut Clock, steps: usize| {
            for _ in 0..steps {
                hand.step(Some((AXE, &family)), None, false, dt());
            }
        };

        // The first attack ever is the chain's opening swing.
        assert_eq!(attack(&mut hand).combo, 0);
        // Re-clicked as the recovery opens (~0.3 s): it chains, and keeps
        // counting — the pose side wraps it over the steps shipped.
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
            hand.step(Some((AXE, &family)), None, true, dt()).unwrap().combo,
            0
        );
        assert_eq!(
            hand.step(Some((AXE, &family)), Some(SwingKind::Break), true, dt())
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
        let family = plain();
        let mut hand = Clock::default();
        let attack = |hand: &mut Clock| {
            hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt())
        };
        let idle = |hand: &mut Clock| hand.step(Some((AXE, &family)), None, false, dt());
        let first = attack(&mut hand).expect("the opening swing plays");
        assert_eq!(first.combo, 0);
        assert!(hand.bars_attack(), "the fresh arc bars the next attack");

        // Mashed mid-arc: the play advances instead of restarting, no
        // chained step begins yet…
        let mashed = attack(&mut hand).expect("the arc keeps playing");
        assert_eq!(mashed.combo, 0, "no chained step mid-swing");
        assert!(mashed.phase > first.phase, "the arc was not restarted");
        assert!(hand.queued, "…but the press is held");
        attack(&mut hand);
        assert!(hand.queued, "a second mid-arc press is not a second queue entry");

        // …and the instant the hold has fully played, the queued press
        // fires as the chained step — no further click needed.
        while hand.bars_attack() {
            idle(&mut hand).unwrap();
        }
        assert!(hand.phase >= 0.72, "the recovery opens at the row's cancel");
        let chained = idle(&mut hand).expect("the queued attack starts");
        assert_eq!(chained.combo, 1, "the queued press chains");
        assert!(chained.phase < 0.1, "a fresh arc");
        assert!(!hand.queued, "the queue is spent");
        assert!(hand.bars_attack(), "the chained arc bars in turn");

        // Nothing queued: the recovery plays out to rest on its own.
        while hand.bars_attack() {
            idle(&mut hand).unwrap();
        }
        assert!(hand.playing, "still mid-arc — only the tail remains");
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
        assert_eq!(swapped.step(None, None, false, dt()), None);
        assert!(!swapped.queued, "the swap drops the queue");
        assert_eq!(
            swapped.step(Some((AXE, &family)), None, false, dt()),
            None,
            "the tool comes back to an idle hand, not a stale swing"
        );

        // A mining press's echoed attack edge is the LOOP, not a heavy first
        // swing: with the level up it plays at the dig cadence from the
        // first frame — the seam's first shipped bug, pinned.
        let mut mining = Clock::default();
        assert_eq!(
            mining.step(Some((AXE, &family)), Some(SwingKind::Attack), true, dt()),
            Some(Play {
                phase: dt() / 0.3,
                combo: 0,
            }),
        );
    }

    /// The hold begins at the step's IMPACT whatever the row's cancel says:
    /// a `cancel_at` authored before the impact would let a mash cut the
    /// hit out of its own animation. Past the impact the row's cancel is
    /// the boundary.
    #[test]
    fn the_recovery_never_opens_before_the_impact() {
        let family = family(&[0.4], 0.3, 0.2, &[0.5, 0.1]);
        let mut hand = Clock::default();
        hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt());
        let mut landed = false;
        while hand.bars_attack() {
            hand.step(Some((AXE, &family)), None, false, dt());
            landed |= hand.impact();
        }
        assert!(landed, "the arc stayed barred through its impact");
        assert!(hand.phase >= 0.5 && hand.phase < 0.6, "{}", hand.phase);

        // Step 1's impact is before the row's cancel: the row stands.
        let mut hand = Clock::default();
        hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt());
        while hand.playing {
            hand.step(Some((AXE, &family)), None, false, dt());
        }
        hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt());
        while hand.bars_attack() {
            hand.step(Some((AXE, &family)), None, false, dt());
        }
        assert!(hand.phase >= 0.2 && hand.phase < 0.3, "{}", hand.phase);
    }

    /// The clock reports an attack's impact on exactly ONE step — the one
    /// whose phase crosses it — and never for work, however long the loop
    /// runs: a swing that landed twice would double every hit, one that
    /// never landed would be a tool that cannot hurt, and a mining loop
    /// that landed would strike whatever stood near a wall being dug.
    #[test]
    fn an_attack_lands_its_impact_once_and_work_never_lands() {
        let family = family(&[0.4], 0.3, 0.72, &[0.5, 0.25]);
        let mut hand = Clock::default();
        hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt());
        let mut landed = usize::from(hand.impact());
        let mut phase_at_impact = None;
        while hand.playing {
            hand.step(Some((AXE, &family)), None, false, dt());
            if hand.impact() {
                landed += 1;
                phase_at_impact = Some(hand.phase);
            }
        }
        assert_eq!(landed, 1, "one impact per arc");
        let at = phase_at_impact.expect("the crossing step");
        assert!((0.5..0.5 + dt() / 0.4 + 1e-4).contains(&at), "at {at}");

        // The chained step lands at ITS OWN phase.
        hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt());
        let mut at = None;
        while hand.playing {
            hand.step(Some((AXE, &family)), None, false, dt());
            if hand.impact() {
                at = Some(hand.phase);
            }
        }
        let at = at.expect("the chained step lands too");
        assert!((0.25..0.5).contains(&at), "step 1 lands at its own phase: {at}");

        // Work never lands, across several wraps of the loop.
        let mut work = Clock::default();
        for _ in 0..80 {
            work.step(Some((AXE, &family)), None, true, dt());
            assert!(!work.impact(), "the mining loop lands nothing");
        }
        work.step(Some((AXE, &family)), Some(SwingKind::Break), true, dt());
        for _ in 0..40 {
            work.step(Some((AXE, &family)), None, true, dt());
            assert!(!work.impact(), "a break lands nothing of its own");
        }

        // A family whose clips mark no impact never reports one.
        let quiet_family = plain();
        let mut quiet = Clock::default();
        quiet.step(Some((AXE, &quiet_family)), Some(SwingKind::Attack), false, dt());
        while quiet.playing {
            quiet.step(Some((AXE, &quiet_family)), None, false, dt());
            assert!(!quiet.impact());
        }
    }

    /// The mining level starts the loop at phase 0 on the work window; a
    /// held button wraps on that ONE clock (never restarting from 0 while
    /// the level holds), and a released level finishes the arc home instead
    /// of rewinding it.
    #[test]
    fn mining_runs_the_loop_on_the_dig_cadence_and_releases_forward() {
        let family = plain();
        let frame = 0.02;
        let mut hand = Clock::default();
        // The rising level starts the loop.
        assert_eq!(
            hand.step(Some((PICK, &family)), None, true, frame),
            Some(Play {
                phase: frame / 0.3,
                combo: 0,
            }),
        );
        let advanced = hand
            .step(Some((PICK, &family)), None, true, frame)
            .expect("a held level keeps the loop")
            .phase;
        assert!(
            (advanced - 2.0 * frame / 0.3).abs() < 1e-4,
            "the loop advances at the swing cadence"
        );

        // A held button WRAPS (the phase cycles), never restarting from 0:
        // several turns of the clock inside this run.
        let mut wraps = 0;
        let mut last = 1.0;
        for _ in 0..60 {
            let Some(Play { phase, .. }) = hand.step(Some((PICK, &family)), None, true, frame)
            else {
                panic!("a held level never idles");
            };
            if phase < last {
                wraps += 1;
            }
            last = phase;
        }
        assert!(wraps >= 2, "a held level wraps on the swing cadence: {last}");

        // The released level finishes the arc home: the swing plays out and
        // then the hand rests — still claimed, so the vanilla punch stays
        // silent while the tool is up.
        let mut done = Clock::default();
        done.step(Some((AXE, &family)), None, true, 0.02);
        let mut released = None;
        for _ in 0..40 {
            released = done.step(Some((AXE, &family)), None, false, 0.02);
            if released.is_none() {
                break;
            }
        }
        assert!(released.is_none(), "the released loop finishes and rests");
    }

    /// An attack edge plays on the attack window, a break on the dig cadence;
    /// a use click is not this clock's. And a tool swap resets the clock
    /// rather than carrying the last item's play into new art.
    #[test]
    fn edges_pace_their_plays_and_a_use_click_is_not_the_clocks() {
        let family = plain();
        let mut axe = Clock::default();
        let chopped = axe.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt());
        assert_eq!(
            chopped,
            Some(Play {
                phase: dt() / 0.4,
                combo: 0,
            }),
            "the axe attacks on the attack window"
        );

        let mut pick = Clock::default();
        assert_eq!(
            pick.step(Some((PICK, &family)), Some(SwingKind::Break), false, dt()),
            Some(Play {
                phase: dt() / 0.3,
                combo: 0,
            }),
            "the pickaxe breaks on the dig cadence"
        );

        // A use click of any kind is the ENGINE's jab, not this clock's:
        // the edge is ignored, the hand stays idle, and the vanilla jab
        // (a motion this pack leaves unclaimed) plays over the authored
        // hold.
        for kind in [SwingKind::Place, SwingKind::Throw, SwingKind::Interact] {
            let mut any = Clock::default();
            assert_eq!(
                any.step(Some((PICK, &family)), Some(kind), false, dt()),
                None,
                "a {kind:?} click starts no pack play"
            );
        }

        // Swapping tools mid-swing restarts clean under the new claim.
        axe.step(Some((PICK, &family)), None, false, dt());
        assert_eq!(axe.style, Some(PICK));
        assert!(!axe.playing, "the pickaxe's clock starts fresh");
    }

    /// The claim follows the tool and yields to the guard: held tool, hands
    /// unclaimed elsewhere — a raised guard poses and stills those hands, and
    /// the claim must not argue with it.
    #[test]
    fn the_claim_follows_the_tool_and_yields_to_the_guard() {
        assert!(claim(Some(AXE), false));
        assert!(!claim(None, false), "a bare hand stays vanilla");
        assert!(!claim(Some(PICK), true), "a raised guard owns it");
        assert!(!claim(None, true));
    }
}
