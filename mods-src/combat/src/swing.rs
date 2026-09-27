//! The tool swing law.
//!
//! While a body's MAIN hand swings one of this pack's tools, this module animates the
//! hand: it holds the engine's player-rig CLIPS for the play in flight, in each rig's
//! claim slot (`set_player_animator_plays`), viewmodel arms in first person, body in
//! third, scrubbed at the pack's own clock. Whatever frame the clock is at is the
//! frame every mirror draws, so a hit landing at the clip's impact lands where it's
//! seen. The engine's own swing for that hand gets silenced by the Swing-motion claim
//! published beside it: two swings on one hand fight each other, so the claim is what
//! makes the pack's clock the whole motion.
//!
//! One pure law, both mirrors: server tick system animates every body at 20 Hz
//! (observers replicate the answer), client frame hook runs the same function for
//! the local player at frame rate, a round trip earlier, same shape as the shield's
//! prediction. The engine's eased pose lane smooths both clocks toward one curve so
//! tick and frame rates never visibly disagree.
//!
//! Use jabs (place / throw / interact) aren't animated here on purpose: the pack
//! claims only the Swing motion, leaving the engine's default jab playing on a
//! claimed hand, so a tool interacts just like any item.
//!
//! Every number the law runs on, which clips, the windows, where the arc opens to
//! the next attack, lives in the family's row ([`crate::families`]). The state
//! machine here is the law.

use crate::families::Family;
pub use crate::families::Style;
use mod_sdk::*;

pub const CHAIN_SECONDS: f32 = 0.8;

pub const CLAIM_SLOT: &str = "main_claim";

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Play {
    pub phase: f32,
    pub combo: usize,
}

#[derive(Debug, Default, PartialEq)]
pub struct Clock {
    pub style: Option<Style>,
    pub playing: bool,
    pub phase: f32,
    seconds: f32,
    attacking: bool,
    combo: usize,
    queued: bool,
    chain: usize,
    since_attack: Option<f32>,
    /// Whether the LAST step carried an attack's phase across its impact —
    /// set by [`Clock::step`], read by [`Clock::impact`]. A latch rather
    /// than a field on [`Play`] because the crossing can coincide with the
    /// arc's final step, which answers no play at all.
    impact_crossed: bool,
}

impl Clock {
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

        let edge = match edge {
            Some(SwingKind::Attack) if mining => None,
            Some(SwingKind::Place | SwingKind::Throw | SwingKind::Interact) => None,
            edge => edge,
        };
        let edge = match edge {
            Some(SwingKind::Attack) if self.bars_attack() => {
                self.queued = true;
                None
            }
            edge => edge,
        };
        let edge = match edge {
            None if self.queued && !self.bars_attack() => {
                self.queued = false;
                Some(SwingKind::Attack)
            }
            edge => edge,
        };

        if let Some(kind) = edge {
            if kind == SwingKind::Attack {
                // Attacks chain. Clicking again inside the window moves to the next step, and
                // waiting longer resets to the opening swing.
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
        } else if !self.playing && mining {
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
        if self.attacking {
            if let Some(at) = family.impact_phase(self.combo) {
                self.impact_crossed = from < at && at <= self.phase;
            }
        }
        if self.phase >= 1.0 {
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

    pub fn bars_attack(&self) -> bool {
        self.attacking
    }

    pub fn attacking(&self) -> bool {
        self.attacking
    }

    pub fn impact(&self) -> bool {
        self.impact_crossed
    }
}

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

/// The whole swing law in one pure function: takes a play, gives back the clips its
/// hand runs on both rigs, in each rig's claim slot. The clock runs on the BODY clip's
/// timeline (that's where the hit lands); the first-person clip gets scrubbed through
/// [`remap`] so the frame the wielder sees land is the frame that actually lands. Both
/// sides call this verbatim, so every mirror draws the same frame the clock is at.
/// Caller gates on [`Clock::step`] and animates nothing on a `None`.
pub fn plays(family: &Family, play: Play, attacking: bool) -> Vec<AnimatorPlay> {
    let progress = play.phase.clamp(0.0, 1.0);
    let (motion, first_person_progress) = if attacking {
        let step = play.combo % family.attacks.len();
        let fp = match (
            family.impact_phase(step),
            family.fp_impacts.get(step).copied().flatten(),
        ) {
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

    fn family(attack: &[f32], mine: f32, impact: &[f32]) -> Family {
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

    fn plain() -> Family {
        family(&[0.4], 0.3, &[])
    }

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
            assert_eq!(
                (body.clip.as_str(), body.slot.as_str()),
                (motion.body.as_str(), CLAIM_SLOT)
            );
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
            assert!(
                close(remap(0.3, from, to), 0.3),
                "an impact on an end is no map"
            );
        }
        let mut last = -1.0;
        for i in 0..=100 {
            let p = remap(i as f32 / 100.0, 0.42, 0.47);
            assert!(p >= last, "{p} after {last}");
            last = p;
        }

        let mut family = family(&[0.4], 0.3, &[0.5, 0.25]);
        family.fp_impacts = vec![Some(0.6), None];
        let fp = |family: &Family, phase: f32, combo: usize, attacking: bool| match plays(
            family,
            Play { phase, combo },
            attacking,
        )[0]
        .clock
        {
            AnimatorClock::Scrub(p) => p,
            other => panic!("{other:?}"),
        };
        assert!(
            close(fp(&family, 0.5, 0, true), 0.6),
            "step 0 lands on its own frame"
        );
        assert!(
            close(fp(&family, 0.25, 1, true), 0.25),
            "no viewmodel marker, no map"
        );
        assert!(
            close(fp(&family, 0.5, 0, false), 0.5),
            "work is never remapped"
        );
        family.impacts.clear();
        assert!(
            close(fp(&family, 0.5, 0, true), 0.5),
            "nothing lands, nothing maps"
        );
    }

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

    #[test]
    fn authored_windows_pace_attacks_per_step_and_mining_by_the_work_window() {
        let family = family(&[0.5, 0.25], 0.42, &[]);
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
        idle(&mut hand, 30);
        let chained = attack(&mut hand);
        assert_eq!(chained.combo, 1);
        assert!(
            (chained.phase - dt() / 0.25).abs() < 1e-6,
            "step 1 plays its own attack window: {}",
            chained.phase
        );

        let mut work = Clock::default();
        let looped = work.step(Some((AXE, &family)), None, true, dt()).unwrap();
        assert!(
            (looped.phase - dt() / 0.42).abs() < 1e-6,
            "{}",
            looped.phase
        );
        let broke = Clock::default()
            .step(Some((AXE, &family)), Some(SwingKind::Break), true, dt())
            .unwrap();
        assert_eq!(broke, looped, "a break lands on the work window");
    }

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

        assert_eq!(attack(&mut hand).combo, 0);
        idle(&mut hand, 25);
        assert_eq!(attack(&mut hand).combo, 1);
        idle(&mut hand, 25);
        assert_eq!(attack(&mut hand).combo, 2);

        idle(&mut hand, 80);
        assert_eq!(attack(&mut hand).combo, 0);

        assert_eq!(
            hand.step(Some((AXE, &family)), None, true, dt())
                .unwrap()
                .combo,
            0
        );
        assert_eq!(
            hand.step(Some((AXE, &family)), Some(SwingKind::Break), true, dt())
                .unwrap()
                .combo,
            0
        );
    }

    /// Attack arc plays out whole, no restart or chain mid-arc.
    /// A click during the swing queues, one deep, and fires once the arc has followed
    /// through. Mashing still chains without hitting the beat. A claim change drops
    /// the queue.
    #[test]
    fn a_mid_arc_attack_queues_until_the_arc_follows_through() {
        let family = plain();
        let mut hand = Clock::default();
        let attack = |hand: &mut Clock| {
            hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt())
        };
        let idle = |hand: &mut Clock| hand.step(Some((AXE, &family)), None, false, dt());
        let first = attack(&mut hand).expect("the opening swing plays");
        assert_eq!(first.combo, 0);
        assert!(hand.bars_attack(), "the fresh arc bars the next attack");

        let mashed = attack(&mut hand).expect("the arc keeps playing");
        assert_eq!(mashed.combo, 0, "no chained step mid-swing");
        assert!(mashed.phase > first.phase, "the arc was not restarted");
        assert!(hand.queued, "…but the press is held");
        attack(&mut hand);
        assert!(
            hand.queued,
            "a second mid-arc press is not a second queue entry"
        );

        let mut steps = 0;
        while hand.bars_attack() {
            idle(&mut hand);
            steps += 1;
            assert!(steps < 1000, "the arc ends");
        }
        let chained = idle(&mut hand).expect("the queued attack starts");
        assert_eq!(chained.combo, 1, "the queued press chains");
        assert!(chained.phase < 0.1, "a fresh arc");
        assert!(!hand.queued, "the queue is spent");
        assert!(hand.bars_attack(), "the chained arc bars in turn");

        let mut at_rest = None;
        for _ in 0..200 {
            at_rest = idle(&mut hand);
            if at_rest.is_none() {
                break;
            }
        }
        assert!(at_rest.is_none(), "the arc rests without a queued press");
        assert!(!hand.bars_attack(), "a rested hand bars nothing");

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

        let mut mining = Clock::default();
        assert_eq!(
            mining.step(Some((AXE, &family)), Some(SwingKind::Attack), true, dt()),
            Some(Play {
                phase: dt() / 0.3,
                combo: 0,
            }),
        );
    }

    #[test]
    fn the_arc_bars_the_next_attack_until_it_has_followed_through() {
        let family = family(&[0.4], 0.3, &[0.5, 0.1]);
        let mut hand = Clock::default();
        hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt());
        let mut landed = false;
        let mut steps = 0;
        while hand.bars_attack() {
            hand.step(Some((AXE, &family)), None, false, dt());
            landed |= hand.impact();
            steps += 1;
            assert!(steps < 1000, "the arc ends");
        }
        assert!(landed, "the arc stayed barred through its impact");
        assert!(!hand.playing, "…and to the end of the play");
    }

    #[test]
    fn an_attack_lands_its_impact_once_and_work_never_lands() {
        let family = family(&[0.4], 0.3, &[0.5, 0.25]);
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

        hand.step(Some((AXE, &family)), Some(SwingKind::Attack), false, dt());
        let mut at = None;
        while hand.playing {
            hand.step(Some((AXE, &family)), None, false, dt());
            if hand.impact() {
                at = Some(hand.phase);
            }
        }
        let at = at.expect("the chained step lands too");
        assert!(
            (0.25..0.5).contains(&at),
            "step 1 lands at its own phase: {at}"
        );

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

        let quiet_family = plain();
        let mut quiet = Clock::default();
        quiet.step(
            Some((AXE, &quiet_family)),
            Some(SwingKind::Attack),
            false,
            dt(),
        );
        while quiet.playing {
            quiet.step(Some((AXE, &quiet_family)), None, false, dt());
            assert!(!quiet.impact());
        }
    }

    #[test]
    fn mining_runs_the_loop_on_the_dig_cadence_and_releases_forward() {
        let family = plain();
        let frame = 0.02;
        let mut hand = Clock::default();
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
        assert!(
            wraps >= 2,
            "a held level wraps on the swing cadence: {last}"
        );

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

        for kind in [SwingKind::Place, SwingKind::Throw, SwingKind::Interact] {
            let mut any = Clock::default();
            assert_eq!(
                any.step(Some((PICK, &family)), Some(kind), false, dt()),
                None,
                "a {kind:?} click starts no pack play"
            );
        }

        axe.step(Some((PICK, &family)), None, false, dt());
        assert_eq!(axe.style, Some(PICK));
        assert!(!axe.playing, "the pickaxe's clock starts fresh");
    }

    #[test]
    fn the_claim_follows_the_tool_and_yields_to_the_guard() {
        assert!(claim(Some(AXE), false));
        assert!(!claim(None, false), "a bare hand stays vanilla");
        assert!(!claim(Some(PICK), true), "a raised guard owns it");
        assert!(!claim(None, true));
    }
}
