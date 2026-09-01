//! The whole shield law, and every tuned number behind it.
//!
//! [`guard_of`] is a pure function of the actor snapshot plus one scalar — how
//! far through the post-hit recoil the body is — and answers everything:
//! whether the hit is absorbed, how fast the body walks, where the shield sits
//! in each view, how the arm holding it is bent. The server tick, the client
//! frame and the damage handler all call it, which makes a prediction that
//! disagrees with the authority impossible rather than merely unlikely.
//!
//! The engine knows nothing about a shield. Nothing outside this module may
//! mirror these values.

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
pub const IMPACT_TICKS: u64 = 10;

/// The same window in seconds, for the CLIENT's clock: a client instance has
/// no tick to read, and an animation wants elapsed time anyway. (20 TPS.)
pub const IMPACT_SECONDS: f32 = IMPACT_TICKS as f32 / 20.0;

/// Fraction of the window the recoil takes to reach full deflection. Fast in,
/// slow out: the impact is a shove, the rest is the arm recovering.
const IMPACT_ATTACK: f32 = 0.25;

/// Cosine of the widest angle off the look direction the shield still covers:
/// 60° each way. A hit from the SIDE is not one the shield is in front of, and
/// a hit from behind never was.
const GUARD_ARC_COS: f32 = 0.5;

/// FIRST PERSON, shield DOWN. The authored `firstperson_righthand` hold IS the
/// guard, so IDLE is the override: drop it below the sight line and push it
/// away from the camera, which shrinks it as much as it lowers it.
///
/// Tuned with `render_held_pose_preview` (`HELD_POSE_ITEM=combat:shield`,
/// `HELD_POSE_STATES`). The −5px of depth is load-bearing; a rotation here (the
/// obvious first guess) tips the face out of frame entirely.
const LOWERED_1P: HeldPoseData = HeldPoseData {
    rotation: [0.0, 0.0, 0.0],
    translation: [1.5, -3.0, -5.0],
};

/// FIRST PERSON at the instant of impact, composed onto the authored guard
/// (hence a settled value of `IDENTITY`). The blow drives the shield back
/// toward the face: +Z is TOWARD the camera.
const IMPACT_1P: HeldPoseData = HeldPoseData {
    rotation: [-6.0, 0.0, 0.0],
    translation: [-0.5, 0.8, 1.2],
};

/// THIRD PERSON, the shielding ARM: the stance itself, as group rotations on
/// the rig's `left_shoulder` / `left_elbow` (the MAIN hand's arm — the rig
/// cross-names them, which is why `bone::MAIN_*` exists).
///
/// `Replace`, not `Compose`: `walk` and `sneak` both drive these exact two
/// bones, so a composed offset would ride on top of the swing and the arm
/// would flap while blocking.
const GUARD_SHOULDER: [f32; 3] = [59.073_37, 19.056_81, -20.208_48];
const GUARD_ELBOW: [f32; 3] = [0.0, 0.0, -37.5];

/// The same arm at the instant of impact: the shoulder GIVES and rolls in
/// across the chest while the elbow folds, so the limb absorbs the blow.
/// Everything in the fist comes with it, which is why [`IMPACT_3P`] only adds
/// the last bit.
///
/// Raising the shoulder HARDER is the obvious guess and is wrong: more X lifts
/// the shield over the face, which reads as bracing rather than as being hit.
const IMPACT_SHOULDER: [f32; 3] = [43.0, 26.0, 5.0];
const IMPACT_ELBOW: [f32; 3] = [0.0, 0.0, -54.0];

/// THIRD PERSON, the shield in the posed fist.
///
/// Measured from the FIST and composed onto the item's AUTHORED third-person
/// hold. It is relative to wherever the arm stance above puts the fist, so
/// changing either the stance or the authored hold invalidates it — re-tune
/// both in one render (`render_held_pose_preview` takes both), never one.
const GUARD_3P: HeldPoseData = HeldPoseData {
    rotation: [-49.408, 34.326, 9.381],
    translation: [3.033, 5.559, 9.000],
};

/// [`GUARD_3P`] at the instant of impact: the same tilt, pressed in toward the
/// body. Translation ONLY — the arm already carries the rotation, and an item
/// that turns as well reads as the shield coming loose in the fist.
const IMPACT_3P: HeldPoseData = HeldPoseData {
    rotation: GUARD_3P.rotation,
    translation: [1.8, 4.6, 8.0],
};

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn lerp_pose(a: HeldPoseData, b: HeldPoseData, t: f32) -> HeldPoseData {
    HeldPoseData {
        rotation: lerp3(a.rotation, b.rotation, t),
        translation: lerp3(a.translation, b.translation, t),
    }
}

/// How far into the impact pose a shield `progress` of the way through its
/// recoil window sits: `0` settled, `1` fully deflected.
///
/// One curve for both sides — a body whose own view recoils differently from
/// every observer's is the same bug as a pose only one side predicts.
fn deflection(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    if progress < IMPACT_ATTACK {
        progress / IMPACT_ATTACK
    } else {
        1.0 - (progress - IMPACT_ATTACK) / (1.0 - IMPACT_ATTACK)
    }
}

/// Is a hit from `origin` one the guard is in front of?
///
/// HORIZONTAL only: pitch is where the player is LOOKING, not where the shield
/// is, so gating on it would drop the guard every time somebody glanced down.
///
/// No origin means no direction to judge, so the guard holds — refusing on
/// missing spatial context would quietly break the shield for any future
/// damage source that omits it.
pub fn covers(state: &PlayerSnapshot, origin: Option<[f32; 3]>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    let (dx, dz) = (origin[0] - state.pos[0], origin[2] - state.pos[2]);
    let distance = (dx * dx + dz * dz).sqrt();
    // Standing exactly inside the attacker names no direction either.
    if distance < 1e-4 {
        return true;
    }
    // Player yaw convention: forward is (sin yaw, cos yaw).
    (state.yaw.sin() * dx + state.yaw.cos() * dz) / distance >= GUARD_ARC_COS
}

/// What the shield is doing for one actor.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Guard {
    /// This body's interact press belongs to the shield. Still true while it is
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
    pub fn denied(&self) -> Vec<BodyAction> {
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

    fn deflection(&self) -> f32 {
        self.impact.map_or(0.0, deflection)
    }

    /// The shielding arm in the guard stance — empty while the shield is down,
    /// so the arms hang and swing normally.
    ///
    /// The off hand gets the MIRROR (negate the y and z rotations): the rig is
    /// mirror-symmetric, so one authored arm is both arms.
    pub fn arms(&self) -> Vec<BonePoseData> {
        if !self.raised {
            return Vec::new();
        }
        let deflection = self.deflection();
        let shoulder = lerp3(GUARD_SHOULDER, IMPACT_SHOULDER, deflection);
        let elbow = lerp3(GUARD_ELBOW, IMPACT_ELBOW, deflection);
        let hold = |bone: &str, r: [f32; 3], mirror: bool| BonePoseData {
            bone: bone.to_string(),
            rotation: if mirror { [r[0], -r[1], -r[2]] } else { r },
            translation: [0.0; 3],
            mode: BonePoseMode::Replace,
        };
        let mut out = Vec::new();
        if self.main_holds {
            out.push(hold(bone::MAIN_SHOULDER, shoulder, false));
            out.push(hold(bone::MAIN_ELBOW, elbow, false));
        }
        if self.off_holds {
            out.push(hold(bone::OFF_SHOULDER, shoulder, true));
            out.push(hold(bone::OFF_ELBOW, elbow, true));
        }
        out
    }

    /// The pose for one hand: `None` unless that hand holds the shield.
    ///
    /// Down lowers in first person only; up raises in third person only —
    /// for this model each authored hold is already correct in one of the two
    /// states, so each view overrides only the other.
    pub fn pose(&self, holds: bool) -> Option<HeldPose> {
        let deflection = self.deflection();
        let pose = if self.raised {
            HeldPose {
                first_person: lerp_pose(HeldPoseData::IDENTITY, IMPACT_1P, deflection),
                third_person: lerp_pose(GUARD_3P, IMPACT_3P, deflection),
            }
        } else {
            HeldPose {
                first_person: LOWERED_1P,
                third_person: HeldPoseData::IDENTITY,
            }
        };
        holds.then_some(pose)
    }
}

/// The entire shield law, as a pure function of the actor snapshot and the
/// recoil clock — free of `self` and of the host, so the tests below pin it
/// directly and all three call sites share it verbatim.
///
/// `impact` is `Some(progress)` — `0..1` through [`IMPACT_TICKS`] — while the
/// shield is still reeling. It arrives as a FRACTION rather than a count
/// because each side measures it off a different clock (ticks on the server,
/// frame seconds on the client) and only the meaning has to agree.
/// `shield` is `None` when the registry carries no shield row — no hand can
/// hold one, so the guard resolves released; the caller's other rules (the
/// tools' swings) run regardless.
pub fn guard_of(shield: Option<ItemId>, state: &PlayerSnapshot, impact: Option<f32>) -> Guard {
    // Deciding a spectator HERE rather than by skipping the publish is what
    // RELEASES the claim: skipping would leave half speed and a raised shield
    // latched on a body no rule is evaluating any more.
    let holds = |slot: Option<ItemId>| {
        !state.spectator && shield.is_some_and(|shield| slot == Some(shield))
    };
    let main_holds = holds(state.held);
    let off_holds = holds(state.off_held);
    // `holds_use`, never the raw button: the press only becomes a guard once
    // nothing else took it, so opening a door with a shield in hand opens the
    // door and leaves the shield down.
    let raised = state.holds_use && (main_holds || off_holds);
    Guard {
        raised,
        main_holds,
        off_holds,
        impact: raised.then_some(impact).flatten(),
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
        }
    }

    fn guarding() -> PlayerSnapshot {
        actor(Some(SHIELD), None, true)
    }

    #[test]
    fn a_guard_needs_the_press_and_a_shield_in_either_hand() {
        assert!(guard_of(Some(SHIELD), &guarding(), None).raised);
        assert!(guard_of(Some(SHIELD), &actor(None, Some(SHIELD), true), None).raised);
        assert!(!guard_of(Some(SHIELD), &actor(Some(SHIELD), None, false), None).raised);
        assert!(!guard_of(Some(SHIELD), &actor(Some(OTHER), None, true), None).raised);
        assert!(!guard_of(Some(SHIELD), &actor(None, None, true), None).raised);
    }

    /// A shield that just took a hit is still UP but stops nothing until it
    /// settles: the window has to be something you can watch it come out of,
    /// not a shield that blinks away.
    #[test]
    fn a_reeling_shield_stays_raised_and_stops_absorbing() {
        let settled = guard_of(Some(SHIELD), &guarding(), None);
        assert!(settled.raised && settled.absorbs());

        for progress in [0.0, 0.5, 0.99] {
            let hit = guard_of(Some(SHIELD), &guarding(), Some(progress));
            assert!(hit.raised, "the button is still held");
            assert!(!hit.absorbs(), "the shield is out of the way at {progress}");
            assert_eq!(hit.speed_scale(), settled.speed_scale(), "still heavy");
        }
    }

    /// The recoil starts and ends AT the settled pose, or a one-off cue leaves
    /// the shield parked somewhere the settled rule would never put it.
    #[test]
    fn the_recoil_returns_to_the_settled_guard_at_both_ends() {
        let settled = guard_of(Some(SHIELD), &guarding(), None);
        for progress in [0.0, 1.0] {
            let hit = guard_of(Some(SHIELD), &guarding(), Some(progress));
            assert_eq!(hit.pose(true), settled.pose(true), "at {progress}");
            assert_eq!(hit.arms(), settled.arms(), "at {progress}");
        }
        let peak = guard_of(Some(SHIELD), &guarding(), Some(IMPACT_ATTACK));
        assert_ne!(peak.pose(true), settled.pose(true), "and moves in between");
        assert_ne!(peak.arms(), settled.arms());
    }

    /// A shield covers what the player is FACING. Getting the yaw convention
    /// backwards blocks exactly the hits it should let through, and no other
    /// test would notice.
    #[test]
    fn the_guard_covers_the_front_arc_only() {
        let mut state = guarding();
        // Yaw 0 faces +Z.
        assert!(covers(&state, Some([0.0, 0.0, 4.0])), "dead ahead");
        assert!(covers(&state, Some([1.0, 0.0, 4.0])), "just off centre");
        assert!(!covers(&state, Some([4.0, 0.0, 0.0])), "side");
        assert!(!covers(&state, Some([-4.0, 0.0, 0.0])), "other side");
        assert!(!covers(&state, Some([0.0, 0.0, -4.0])), "behind");

        // The arc turns with the player, not with the world.
        state.yaw = std::f32::consts::FRAC_PI_2;
        assert!(covers(&state, Some([4.0, 0.0, 0.0])));
        assert!(!covers(&state, Some([0.0, 0.0, 4.0])));

        // Height is not part of it: a hit from directly above is still frontal
        // if the attacker is in front, and an attacker sharing the column has
        // no direction at all.
        assert!(covers(&state, Some([4.0, 9.0, 0.0])));
        assert!(covers(&state, Some(state.pos)));
        assert!(covers(&state, None), "no origin, no direction to refuse on");
    }

    /// Pinned in the terms the asset is authored in: this model's first-person
    /// hold IS the guard, so raising leaves first person alone and lowering
    /// moves it; the third-person hold is the carry, so it is the reverse.
    /// Inverted, the shield flips around in the wielder's face and reads as a
    /// renderer bug rather than a mod one.
    #[test]
    fn each_view_overrides_only_the_state_its_authored_hold_is_wrong_for() {
        let raised = guard_of(Some(SHIELD), &guarding(), None);
        let up = raised.pose(raised.main_holds).expect("shielding hand");
        assert!(
            up.first_person.is_identity(),
            "a settled guard keeps the authored first-person hold"
        );
        assert!(!up.third_person.is_identity(), "raising moves the body");

        let idle = guard_of(Some(SHIELD), &actor(Some(SHIELD), None, false), None);
        let down = idle.pose(idle.main_holds).expect("shielding hand");
        assert!(!down.first_person.is_identity(), "idle lowers the screen");
        assert!(
            down.third_person.is_identity(),
            "idle keeps the authored carry"
        );
        assert!(
            down.first_person.translation[1] < 0.0,
            "lowering is downward, and only far enough to clear the sight line"
        );
    }

    /// Only a shielding arm is held, only while the shield is up, and each hand
    /// holds its OWN joints. Both facts fail silently: the rig authors the main
    /// hand's arm as the model's LEFT, and a stance that skips a joint leaves
    /// the elbow swinging with the stride.
    #[test]
    fn only_a_shielding_arm_is_held_and_each_hand_holds_its_own_joints() {
        let idle = guard_of(Some(SHIELD), &actor(Some(SHIELD), None, false), None);
        assert!(idle.arms().is_empty(), "an idle arm hangs normally");

        let names = |g: Guard| -> Vec<String> { g.arms().into_iter().map(|b| b.bone).collect() };
        let main = guard_of(Some(SHIELD), &actor(Some(SHIELD), Some(OTHER), true), None);
        assert_eq!(names(main), [bone::MAIN_SHOULDER, bone::MAIN_ELBOW]);

        let off = guard_of(Some(SHIELD), &actor(Some(OTHER), Some(SHIELD), true), None);
        assert_eq!(names(off), [bone::OFF_SHOULDER, bone::OFF_ELBOW]);

        let both = guard_of(Some(SHIELD), &actor(Some(SHIELD), Some(SHIELD), true), None);
        assert_eq!(both.arms().len(), 4, "a whole arm per shielding hand");
        let mirrored: Vec<[f32; 3]> = both.arms().iter().map(|b| b.rotation).collect();
        assert!(
            mirrored.contains(&GUARD_SHOULDER)
                && mirrored.contains(&[GUARD_SHOULDER[0], -GUARD_SHOULDER[1], -GUARD_SHOULDER[2]]),
            "one authored arm, mirrored for the other"
        );
    }

    #[test]
    fn only_the_shielding_hand_is_posed() {
        for holds_use in [false, true] {
            let g = guard_of(
                Some(SHIELD),
                &actor(Some(SHIELD), Some(OTHER), holds_use),
                None,
            );
            assert!(g.pose(g.main_holds).is_some());
            assert!(g.pose(g.off_holds).is_none());

            let g = guard_of(
                Some(SHIELD),
                &actor(Some(OTHER), Some(SHIELD), holds_use),
                None,
            );
            assert!(g.pose(g.main_holds).is_none());
            assert!(g.pose(g.off_holds).is_some());
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
        let guard = guard_of(Some(SHIELD), &watching, Some(0.3));
        assert!(!guard.raised);
        assert!(!guard.absorbs());
        assert_eq!(guard.speed_scale(), 1.0, "the speed claim is released");
        assert!(guard.arms().is_empty());
        assert!(guard.pose(guard.main_holds).is_none());
        assert!(guard.pose(guard.off_holds).is_none());
    }

    /// A released guard claims the NEUTRAL speed and bars nothing, which is
    /// what hands the slots back rather than pinning the body at half speed
    /// with its hands tied.
    #[test]
    fn releasing_the_guard_releases_every_claim() {
        let up = guard_of(Some(SHIELD), &guarding(), None);
        assert_eq!(up.speed_scale(), GUARD_SPEED_SCALE);
        assert_eq!(up.denied(), [BodyAction::Attack, BodyAction::Mine]);

        let down = guard_of(Some(SHIELD), &actor(Some(SHIELD), None, false), None);
        assert_eq!(down.speed_scale(), 1.0);
        assert!(down.denied().is_empty());
    }

    /// A shield knocked aside still ties the hands: the recoil is where it
    /// stops PROTECTING you, and handing back the punch it was costing would
    /// make taking a hit the best moment to attack.
    #[test]
    fn a_reeling_shield_still_denies_the_hands() {
        let hit = guard_of(Some(SHIELD), &guarding(), Some(0.5));
        assert!(!hit.absorbs());
        assert_eq!(hit.denied(), [BodyAction::Attack, BodyAction::Mine]);
    }
}
