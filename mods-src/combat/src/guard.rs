//! The whole shield law, and every tuned number behind it.
//!
//! [`guard_of`] is a pure function of the actor snapshot, whether the use
//! press is the guard's, and one scalar — how far through the post-hit
//! recoil the body is — and answers everything: whether the hit is
//! absorbed, how fast the body walks, which clip each shielding hand plays
//! and how far through it. The server tick, the client frame and
//! the damage handler all call it, which makes a prediction that disagrees
//! with the authority impossible rather than merely unlikely.
//!
//! The engine knows nothing about a shield. Nothing outside this module may
//! mirror these values.

use crate::body::BodyClocks;
use crate::claims::{Body, Claims, Cover, Rule};
use mod_sdk::*;

/// The shield's registry name (`items.json` row).
pub const SHIELD_ITEM: &str = "combat:shield";

/// The one-shot played when the guard absorbs a hit (`sounds.json` row).
pub const BLOCK_SOUND: &str = "combat:shield_block";

/// Land-speed multiplier while the shield is up.
const GUARD_SPEED_SCALE: f32 = 0.5;

/// Ticks the shield stays INACTIVE after absorbing a hit — knocked aside, and
/// it has to be brought back. A second attacker inside that window gets
/// through, so a pack is still a real threat to somebody hiding behind one.
pub const IMPACT_TICKS: u32 = 10;

/// The guard's cover: 60° each way off the look direction. A hit from the
/// SIDE is not one the shield is in front of, and a hit from behind never
/// was.
const GUARD_COVER: Cover = Cover { arc_cos: 0.5 };

/// The clips a shielding hand plays (engine rig clips): the carry while the
/// shield is down — first person only, the body's own hold already IS the
/// carry — the guard while it is up, and the recoil, scrubbed through the
/// window, while it reels. The recoil clips start and end ON the guard, so
/// the cuts into and out of the window never show. Each hand plays in its
/// own claim slot; the off hand plays the main-hand clips mirrored.
///
/// The carry and the guard are HELD frames: a scrub parked at zero, which
/// never changes from tick to tick, so a settled guard costs no publish.
const CARRY_1P: &str = "petramond:fp_shield_carry";
const GUARD_1P: &str = "petramond:fp_guard";
const RECOIL_1P: &str = "petramond:fp_guard_impact";
const GUARD_3P: &str = "petramond:body_guard";
const RECOIL_3P: &str = "petramond:body_guard_impact";
const SLOTS: [&str; 2] = ["main_claim", "off_claim"];

/// Every rig clip the guard plays, `(rig, clip)`.
#[cfg(test)]
pub(crate) const CLIPS: [(&str, &str); 5] = [
    (rig::PLAYER_FIRST_PERSON, CARRY_1P),
    (rig::PLAYER_FIRST_PERSON, GUARD_1P),
    (rig::PLAYER_FIRST_PERSON, RECOIL_1P),
    (rig::PLAYER_BODY, GUARD_3P),
    (rig::PLAYER_BODY, RECOIL_3P),
];

/// One body's recoil clock: ticks since its shield took a hit, running
/// only through the window. Both sides run one — whole ticks on the
/// server, frame seconds over the tick length on the client — and only the
/// meaning has to agree, which is why it answers a FRACTION.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Recoil {
    elapsed: Option<f32>,
}

impl Recoil {
    /// The shield just took a hit.
    pub fn start(&mut self) {
        self.elapsed = Some(0.0);
    }

    /// Advance by `dt_ticks`; the clock releases itself when the window is
    /// out.
    pub fn step(&mut self, dt_ticks: f32) {
        if let Some(elapsed) = self.elapsed {
            let elapsed = elapsed + dt_ticks.max(0.0);
            self.elapsed = (elapsed < IMPACT_TICKS as f32).then_some(elapsed);
        }
    }

    /// How far through the window the body is, `0..1`; `None` settled.
    pub fn progress(&self) -> Option<f32> {
        self.elapsed.map(|elapsed| elapsed / IMPACT_TICKS as f32)
    }
}

/// What the shield is doing for one actor.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Guard {
    /// This body's use press belongs to the shield. Still true while it is
    /// reeling: the player never let go, it just is not stopping anything.
    pub raised: bool,
    /// Which hands hold the shield — only a shielding hand is ever posed, so
    /// the other keeps its ordinary hold.
    pub main_holds: bool,
    pub off_holds: bool,
    /// How far through the post-hit window this body is (`None` = settled).
    ///
    /// ONE value gates the block AND drives the animation: an inactive shield
    /// has to LOOK inactive, and a separate presentation timer would eventually
    /// disagree with the rule about which it was.
    impact: Option<f32>,
}

impl Guard {
    /// Does this guard stop a hit? Only a raised shield that is not still
    /// reeling from the last one.
    pub fn absorbs(&self) -> bool {
        self.raised && self.impact.is_none()
    }

    /// What a raised shield stops the body doing. Both hands are committed to
    /// the guard — the off hand is on the grip whichever hand carries it — so
    /// there is nothing left to punch or mine with.
    ///
    /// Without it the shield is a free answer to every mob in the game: hold
    /// the button, walk in, and work from behind an invulnerable wall. The
    /// interact needs no barring here — holding the GESTURE is what keeps the
    /// button from doing anything else, and it lets go the moment the guard
    /// does.
    ///
    /// A reeling shield still denies both, for the same reason it still slows
    /// you: it is up, it is just not stopping anything.
    fn denied(&self) -> Vec<BodyAction> {
        if self.raised {
            vec![BodyAction::Attack, BodyAction::Mine]
        } else {
            Vec::new()
        }
    }

    /// The land-speed multiplier to claim (`1.0` releases the claim). A
    /// reeling shield is still up and still heavy.
    pub fn speed_scale(&self) -> f32 {
        if self.raised {
            GUARD_SPEED_SCALE
        } else {
            1.0
        }
    }

    /// What one hand (`0` main, `1` off) plays: nothing unless it holds the
    /// shield.
    fn plays(&self, hand: usize, holds: bool) -> Vec<AnimatorPlay> {
        if !holds {
            return Vec::new();
        }
        const HELD: AnimatorClock = AnimatorClock::Scrub(0.0);
        let (first_person, third_person, clock) = match (self.raised, self.impact) {
            (false, _) => (CARRY_1P, None, HELD),
            (true, None) => (GUARD_1P, Some(GUARD_3P), HELD),
            (true, Some(progress)) => (RECOIL_1P, Some(RECOIL_3P), AnimatorClock::Scrub(progress)),
        };
        let mut out = Vec::new();
        let mut play = |rig: &str, clip: &str| {
            out.push(AnimatorPlay {
                rig: rig.to_string(),
                slot: SLOTS[hand].to_string(),
                clip: clip.to_string(),
                clock,
                mirror: hand == 1,
                priority: 0,
            });
        };
        play(rig::PLAYER_FIRST_PERSON, first_person);
        if let Some(body) = third_person {
            play(rig::PLAYER_BODY, body);
        }
        out
    }

    /// Everything the guard claims about the body this tick, for the
    /// publisher to merge: the lowered carry still animates a shielding hand,
    /// everything else releases with the guard.
    pub fn claims(&self) -> Claims {
        Claims {
            holds_press: self.raised,
            speed: self.speed_scale(),
            denied: self.denied(),
            plays: [self.plays(0, self.main_holds), self.plays(1, self.off_holds)].concat(),
            cover: self.absorbs().then_some(GUARD_COVER),
            ..Default::default()
        }
    }
}

/// The entire shield law, as a pure function of the actor snapshot, the
/// press and the recoil clock — free of `self` and of the host, so the
/// tests below pin it directly and all three call sites share it verbatim.
///
/// `press` is whether the use press is the guard's — the composition's
/// answer, never the raw button: the press only becomes a guard once
/// nothing else took it, so opening a door with a shield in hand opens the
/// door and leaves the shield down, and a bow drawing over an off-hand
/// shield keeps it lowered. `impact` is `Some(progress)` — `0..1` through
/// [`IMPACT_TICKS`] — while the shield is still reeling.
pub fn guard_of(shield: ItemId, state: &PlayerSnapshot, press: bool, impact: Option<f32>) -> Guard {
    // Deciding a spectator HERE rather than by skipping the publish is what
    // RELEASES the claim: skipping would leave half speed and a raised shield
    // latched on a body no rule is evaluating any more.
    let holds = |slot: Option<ItemId>| !state.spectator && slot == Some(shield);
    let main_holds = holds(state.held);
    let off_holds = holds(state.off_held);
    let raised = press && (main_holds || off_holds);
    Guard {
        raised,
        main_holds,
        off_holds,
        impact: raised.then_some(impact).flatten(),
    }
}

/// The shield as one of the pack's rules: a shield in EITHER hand takes a
/// free press and holds it as the guard until the button comes up.
pub struct ShieldRule {
    shield: ItemId,
}

impl ShieldRule {
    /// Resolve the shield row; `None` (a build without it) leaves the guard
    /// out of the list.
    pub fn resolve() -> Option<ShieldRule> {
        let shield = resolve_item(SHIELD_ITEM);
        if shield.is_none() {
            log("[combat] 'combat:shield' did not resolve — the guard stays inert");
        }
        Some(ShieldRule { shield: shield? })
    }
}

impl Rule for ShieldRule {
    fn takes_press(&self, state: &PlayerSnapshot) -> bool {
        let holds = |slot| !state.spectator && slot == Some(self.shield);
        holds(state.held) || holds(state.off_held)
    }

    fn step(&self, _: &mut BodyClocks, _: PlayerId, _: &PlayerSnapshot, _: bool, _: f32, _: bool) {}

    fn claims(&self, body: &Body) -> Claims {
        guard_of(
            self.shield,
            body.state,
            body.press,
            body.clocks.recoil.progress(),
        )
        .claims()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIELD: ItemId = ItemId(7);
    const OTHER: ItemId = ItemId(8);

    fn actor(held: Option<ItemId>, off_held: Option<ItemId>, holds_use: bool) -> PlayerSnapshot {
        PlayerSnapshot {
            id: Some(PlayerId(0)),
            pos: [0.0; 3],
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            health: 20,
            on_ground: true,
            spectator: false,
            sneak: false,
            use_held: holds_use,
            holds_use,
            held,
            off_held,
            held_count: 1,
            pose_anchor: None,
            swing: Default::default(),
            half_width: 0.3,
            height: 1.8,
            eye_height: 1.62,
            conditions: Vec::new(),
        }
    }

    /// The law under the press the composition hands it: the snapshot's
    /// own `holds_use` (nothing earlier in the list took it).
    fn guard(state: &PlayerSnapshot, impact: Option<f32>) -> Guard {
        guard_of(SHIELD, state, state.holds_use, impact)
    }

    fn guarding() -> PlayerSnapshot {
        actor(Some(SHIELD), None, true)
    }

    #[test]
    fn a_guard_needs_the_press_and_a_shield_in_either_hand() {
        assert!(guard(&guarding(), None).raised);
        assert!(guard(&actor(None, Some(SHIELD), true), None).raised);
        assert!(!guard(&actor(Some(SHIELD), None, false), None).raised);
        assert!(!guard(&actor(Some(OTHER), None, true), None).raised);
        assert!(!guard(&actor(None, None, true), None).raised);
        assert!(
            !guard_of(SHIELD, &guarding(), false, None).raised,
            "a press an earlier rule holds is not the guard's"
        );
    }

    /// A shield that just took a hit is still UP but stops nothing until it
    /// settles: the window has to be something you can watch it come out of,
    /// not a shield that blinks away.
    #[test]
    fn a_reeling_shield_stays_raised_and_stops_absorbing() {
        let settled = guard(&guarding(), None);
        assert!(settled.raised && settled.absorbs());
        assert!(settled.claims().cover.is_some());

        for progress in [0.0, 0.5, 0.99] {
            let hit = guard(&guarding(), Some(progress));
            assert!(hit.raised, "the button is still held");
            assert!(!hit.absorbs(), "the shield is out of the way at {progress}");
            assert!(hit.claims().cover.is_none());
            assert_eq!(hit.speed_scale(), settled.speed_scale(), "still heavy");
        }
    }

    /// Only a shielding hand plays, and it plays what the shield is DOING:
    /// the carry, the guard, or the recoil scrubbed at the window's progress.
    /// A settled guard still scrubbing its recoil parks the shield knocked
    /// aside; a reeling one showing the guard hides the only sign that the
    /// next hit gets through.
    #[test]
    fn the_shielding_hand_plays_what_the_shield_is_doing() {
        let main = |g: Guard| g.plays(0, g.main_holds);
        assert!(main(guard(&actor(Some(OTHER), Some(SHIELD), true), None)).is_empty());
        let off = guard(&actor(Some(OTHER), Some(SHIELD), true), None);
        let off_plays = off.plays(1, off.off_holds);
        assert!(
            !off_plays.is_empty() && off_plays.iter().all(|p| p.mirror && p.slot == "off_claim"),
            "the off hand plays its own slot, mirrored"
        );
        let both = guard(&actor(Some(SHIELD), Some(SHIELD), true), None);
        assert_eq!(both.claims().plays.len(), 4, "both hands, both rigs");

        let on = |plays: &[AnimatorPlay], rig: &str| {
            plays.iter().find(|p| p.rig == rig).map(|p| (p.clip.clone(), p.clock))
        };
        let down = main(guard(&actor(Some(SHIELD), None, false), None));
        let up = main(guard(&guarding(), None));
        let reeling = main(guard(&guarding(), Some(0.4)));
        assert!(
            on(&down, rig::PLAYER_FIRST_PERSON).is_some() && on(&down, rig::PLAYER_BODY).is_none(),
            "the body's hold is the carry"
        );
        assert_ne!(on(&down, rig::PLAYER_FIRST_PERSON), on(&up, rig::PLAYER_FIRST_PERSON), "raising changes the view");
        assert!(on(&up, rig::PLAYER_BODY).is_some(), "and the body");
        assert_ne!(on(&reeling, rig::PLAYER_BODY), on(&up, rig::PLAYER_BODY));
        assert_eq!(on(&reeling, rig::PLAYER_BODY).unwrap().1, AnimatorClock::Scrub(0.4), "the recoil scrubs at the window's progress");
        for held in [&down, &up] {
            assert!(
                held.iter().all(|p| p.clock == AnimatorClock::Scrub(0.0)),
                "a settled hand holds its frame: {held:?}"
            );
        }
    }

    /// A SPECTATOR is not guarding, whatever their hotbar and button say — and
    /// the RULE has to say so rather than the publish loop skipping them, or a
    /// player who goes spectator mid-guard keeps half speed and a raised shield
    /// until something else happens to clear them.
    #[test]
    fn a_spectator_is_not_guarding_and_so_releases_every_claim() {
        let mut watching = actor(Some(SHIELD), Some(SHIELD), true);
        watching.spectator = true;
        let g = guard(&watching, Some(0.3));
        assert!(!g.raised);
        assert!(!g.absorbs());
        assert_eq!(g.speed_scale(), 1.0, "the speed claim is released");
        assert!(g.claims().plays.is_empty());
    }

    /// A released guard claims the NEUTRAL speed and bars nothing, which is
    /// what hands the slots back rather than pinning the body at half speed
    /// with its hands tied.
    #[test]
    fn releasing_the_guard_releases_every_claim() {
        let up = guard(&guarding(), None);
        assert_eq!(up.speed_scale(), GUARD_SPEED_SCALE);
        assert_eq!(up.denied(), [BodyAction::Attack, BodyAction::Mine]);

        let down = guard(&actor(Some(SHIELD), None, false), None);
        assert_eq!(down.speed_scale(), 1.0);
        assert!(down.denied().is_empty());
    }

    /// A shield knocked aside still ties the hands: the recoil is where it
    /// stops PROTECTING you, and handing back the punch it was costing would
    /// make taking a hit the best moment to attack.
    #[test]
    fn a_reeling_shield_still_denies_the_hands() {
        let hit = guard(&guarding(), Some(0.5));
        assert!(!hit.absorbs());
        assert_eq!(hit.denied(), [BodyAction::Attack, BodyAction::Mine]);
    }
}
