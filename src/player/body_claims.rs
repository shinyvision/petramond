//! Transient claims on a player's body: how fast it moves, what it may do,
//! how it holds what it holds, and how its bones are bent.
//!
//! Claims are keyed by CLAIMANT, so two of them can want the same knob at
//! once and the engine resolves rather than letting the last writer win. Every
//! claim is transient — never saved — and lives only as long as its claimant
//! keeps re-stating it, which makes a claimant that goes away self-healing
//! instead of a leak.
//!
//! The same type resolves both mirrors: the server folds its side's claims and
//! replicates the ANSWER, and a client folds its own to predict the local body.
//! One resolution rule, so a prediction cannot disagree with the authority by
//! construction.

use mod_api::{BodyAction, HeldPose};
use serde::{Deserialize, Serialize};

use petramond_world::inventory::Hand;

/// One resolved bone offset on a body, with the bone RESOLVED to a rig id
/// ([`crate::player::model::bone_id`]) and no name anywhere.
///
/// A name is authoring vocabulary; below the ABI this is what the claim, the
/// wire row and the render instance all carry, so an offset is a plain `Copy`
/// value that costs nothing to publish 20 times a second.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct BonePose {
    /// The rig bone this offsets, as [`bone_id`](crate::player::model::bone_id)
    /// resolved it.
    pub bone: u16,
    /// Rotation in DEGREES about the bone's pivot, applied X then Y then Z.
    pub rotation: [f32; 3],
    /// Translation in 1/16-BLOCK pixels, in the bone's frame.
    pub translation: [f32; 3],
    /// REPLACE the animation's own posing of this bone rather than composing
    /// onto it — a stance, which must not also swing with the stride.
    pub hold: bool,
}

impl BonePose {
    /// Every component finite. A NaN here would poison every transform below
    /// the bone, so a list containing one is refused whole.
    fn is_finite(&self) -> bool {
        self.rotation
            .iter()
            .chain(&self.translation)
            .all(|c| c.is_finite())
    }
}

/// The set of [`BodyAction`]s barred on one body — the resolved answer, as a
/// bitmask, so it costs one byte on the wire and nothing to carry on a `Copy`
/// row. The ABI speaks the enum and this speaks bits; the conversion happens
/// once, at the host call, exactly as bone names resolve to rig ids there.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DeniedActions(u8);

impl DeniedActions {
    /// Nothing barred — what a body with no claims on it answers.
    pub const NONE: Self = DeniedActions(0);

    fn bit(action: BodyAction) -> u8 {
        match action {
            BodyAction::Attack => 1 << 0,
            BodyAction::Mine => 1 << 1,
            BodyAction::Use => 1 << 2,
        }
    }

    /// The set naming exactly `actions`.
    pub fn of(actions: impl IntoIterator<Item = BodyAction>) -> Self {
        DeniedActions(actions.into_iter().fold(0, |m, a| m | Self::bit(a)))
    }

    /// Is `action` barred on this body?
    #[inline]
    pub fn denies(self, action: BodyAction) -> bool {
        self.0 & Self::bit(action) != 0
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[inline]
    fn union(self, other: Self) -> Self {
        DeniedActions(self.0 | other.0)
    }
}

/// The claimant the ENGINE itself publishes under.
///
/// The engine is not privileged here: its own rules — a status effect's speed,
/// a spectator with no body to act with, a menu that has the hands — take a
/// slot like anything else, so "how fast does this body move" and "may it
/// mine" are ONE question with ONE answer wherever they are asked. It is the
/// engine's own reserved namespace, which no pack may register as its id, so
/// nothing can squat it.
pub const ENGINE_CLAIMANT: &str = "petramond";

/// The land-speed scale a body runs at with nothing claimed.
pub const MOVE_SCALE_DEFAULT: f32 = 1.0;
/// Widest land-speed scale the resolved product may reach — generous enough
/// for any haste, low enough that a hostile or typo'd claim cannot fling a
/// body past what the terrain streamer keeps up with. Matches the
/// status-effect row bound.
pub const MOVE_SCALE_MAX: f32 = 5.0;

/// One claimant's claim on one body.
#[derive(Clone, Debug, PartialEq)]
struct Claim {
    /// Who is claiming — also the deterministic ordering key, so the fold
    /// never depends on who happened to call first.
    claimant: Box<str>,
    /// Land-speed multiplier ([`MOVE_SCALE_DEFAULT`] = released).
    speed_scale: f32,
    main: Option<HeldPose>,
    off: Option<HeldPose>,
    bones: Vec<BonePose>,
    denied: DeniedActions,
}

impl Claim {
    /// Nothing left to say: the claim is dropped rather than kept at its
    /// neutral value, so the vector holds exactly the live claimants.
    fn is_released(&self) -> bool {
        self.speed_scale == MOVE_SCALE_DEFAULT
            && self.main.is_none()
            && self.off.is_none()
            && self.bones.is_empty()
            && self.denied.is_empty()
    }
}

/// Every claim on one body, in claimant order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BodyClaims {
    by_claimant: Vec<Claim>,
}

impl BodyClaims {
    /// Whether anything is claimed — the cheap gate the replication and render
    /// paths test before building anything.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.by_claimant.is_empty()
    }

    /// Drop every claim. Used by a MIRROR body before it adopts the answer the
    /// authority resolved (see
    /// [`Player::adopt_resolved_body`](super::Player::adopt_resolved_body));
    /// nothing else clears a body, because a live claim expires by its
    /// claimant simply not re-stating it.
    pub fn clear(&mut self) {
        self.by_claimant.clear();
    }

    /// Whether `claimant` already holds a claim. The neutral-write fast path:
    /// re-stating "nothing" every tick (the shield nobody is carrying) must
    /// not allocate a claim just to prune it again.
    fn holds(&self, claimant: &str) -> bool {
        self.by_claimant
            .binary_search_by(|c| (*c.claimant).cmp(claimant))
            .is_ok()
    }

    fn slot(&mut self, claimant: &str) -> usize {
        match self
            .by_claimant
            .binary_search_by(|c| (*c.claimant).cmp(claimant))
        {
            Ok(i) => i,
            Err(i) => {
                self.by_claimant.insert(
                    i,
                    Claim {
                        claimant: claimant.into(),
                        speed_scale: MOVE_SCALE_DEFAULT,
                        main: None,
                        off: None,
                        bones: Vec::new(),
                        denied: DeniedActions::NONE,
                    },
                );
                i
            }
        }
    }

    fn prune(&mut self, at: usize) {
        if self.by_claimant[at].is_released() {
            self.by_claimant.remove(at);
        }
    }

    /// Set `claimant`'s land-speed multiplier, replacing its previous value.
    /// Non-finite is refused (`false`); finite values clamp into
    /// `[0, MOVE_SCALE_MAX]` so no single claim can reverse or fling the body.
    pub fn set_speed_scale(&mut self, claimant: &str, scale: f32) -> bool {
        if !scale.is_finite() {
            return false;
        }
        let scale = scale.clamp(0.0, MOVE_SCALE_MAX);
        if scale == MOVE_SCALE_DEFAULT && !self.holds(claimant) {
            return true;
        }
        let at = self.slot(claimant);
        self.by_claimant[at].speed_scale = scale;
        self.prune(at);
        true
    }

    /// Set `claimant`'s held-item pose for both hands (`None` releases a
    /// hand). A non-finite component is refused (`false`) and nothing is
    /// written — a NaN pose is a caller bug, not a value to store.
    pub fn set_held_pose(
        &mut self,
        claimant: &str,
        main: Option<HeldPose>,
        off: Option<HeldPose>,
    ) -> bool {
        if [main, off]
            .iter()
            .flatten()
            .any(|p: &HeldPose| !p.is_finite())
        {
            return false;
        }
        // An identity pose says the same thing as no pose; normalizing here
        // keeps a caller that publishes `IDENTITY` every tick from pinning a
        // claim (and a wire field) alive for nothing.
        let keep = |p: Option<HeldPose>| p.filter(|p| !p.is_identity());
        let (main, off) = (keep(main), keep(off));
        if main.is_none() && off.is_none() && !self.holds(claimant) {
            return true;
        }
        let at = self.slot(claimant);
        self.by_claimant[at].main = main;
        self.by_claimant[at].off = off;
        self.prune(at);
        true
    }

    /// Set `claimant`'s rig-bone offsets (an empty list releases them).
    /// Refused (`false`) whole if any component is non-finite.
    ///
    /// Nothing caps how many bones a body wears: each offset is a rotation
    /// about its own joint, so they genuinely compose, and a limit here would
    /// be a number a rig with more joints than today's runs into silently.
    pub fn set_bone_poses(&mut self, claimant: &str, bones: Vec<BonePose>) -> bool {
        if !bones.iter().all(BonePose::is_finite) {
            return false;
        }
        if bones.is_empty() && !self.holds(claimant) {
            return true;
        }
        let at = self.slot(claimant);
        self.by_claimant[at].bones = bones;
        self.prune(at);
        true
    }

    /// Set the actions `claimant` bars on this body (an empty set releases
    /// them). Infallible, unlike the pose setters: a set of enum values has no
    /// malformed form to refuse.
    pub fn set_denied_actions(&mut self, claimant: &str, denied: DeniedActions) {
        if denied.is_empty() && !self.holds(claimant) {
            return;
        }
        let at = self.slot(claimant);
        self.by_claimant[at].denied = denied;
        self.prune(at);
    }

    /// The resolved barred-action set: the UNION of every claim.
    ///
    /// A union rather than the last-wins rule poses use, because unlike a hand
    /// (which holds one item, so two poses are a conflict) a restriction has
    /// no conflict to resolve — two claimants each barring something both mean
    /// it, and one able to un-bar another's would make "is this body allowed
    /// to mine" depend on claimant order.
    pub fn denied_actions(&self) -> DeniedActions {
        self.by_claimant
            .iter()
            .fold(DeniedActions::NONE, |set, c| set.union(c.denied))
    }

    /// Every claim's rig-bone offsets, in claimant order — the order the
    /// renderer applies them in.
    ///
    /// Unlike a held pose these genuinely COMPOSE: a bone offset is a rotation
    /// about that bone's pivot, and two of them layer exactly the way the
    /// engine's own arm layers do, so every claim is handed through.
    pub fn bone_poses(&self) -> impl Iterator<Item = BonePose> + '_ {
        self.by_claimant
            .iter()
            .flat_map(|c| c.bones.iter().copied())
    }

    /// Whether any bone is offset — the gate replication tests before building
    /// a list.
    pub fn has_bone_poses(&self) -> bool {
        self.by_claimant.iter().any(|c| !c.bones.is_empty())
    }

    /// The resolved answer over the claims a MIRROR cannot work out for itself
    /// — everything except [`ENGINE_CLAIMANT`]'s, which both sides derive from
    /// state they both hold (the effect list, the mode, the open menu).
    ///
    /// Replication sends THIS, not the full fold. Sending the whole answer
    /// would make the engine's own half arrive a batch late and stop being
    /// predicted, which is the opposite of what folding it in was for; sending
    /// neither half would leave the mirror guessing at rules only the server
    /// can see. Each side computes the half it can and is told the half it
    /// cannot.
    pub fn replicated_speed_scale(&self) -> f32 {
        self.by_claimant
            .iter()
            .filter(|c| &*c.claimant != ENGINE_CLAIMANT)
            .map(|c| c.speed_scale)
            .product::<f32>()
            .clamp(0.0, MOVE_SCALE_MAX)
    }

    /// [`replicated_speed_scale`](Self::replicated_speed_scale) for the barred
    /// set, and for the same reason.
    pub fn replicated_denied_actions(&self) -> DeniedActions {
        self.by_claimant
            .iter()
            .filter(|c| &*c.claimant != ENGINE_CLAIMANT)
            .fold(DeniedActions::NONE, |set, c| set.union(c.denied))
    }

    /// The resolved land-speed multiplier: the PRODUCT of every claim, clamped
    /// once at the end. A product is the composable answer — two slows both
    /// apply, neither stomps the other, and the order they were set in cannot
    /// change the result.
    pub fn speed_scale(&self) -> f32 {
        self.by_claimant
            .iter()
            .map(|c| c.speed_scale)
            .product::<f32>()
            .clamp(0.0, MOVE_SCALE_MAX)
    }

    /// The resolved pose for one hand: the LAST claim in claimant order.
    ///
    /// There is no meaningful product here — a hand holds one item, so two
    /// live poses on it are a conflict, not a composition. Keying still buys
    /// the thing that matters: one claimant releasing its pose uncovers
    /// another's instead of blanking the hand.
    pub fn held_pose(&self, hand: Hand) -> Option<HeldPose> {
        self.by_claimant.iter().rev().find_map(|c| match hand {
            Hand::Main => c.main,
            Hand::Off => c.off,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(y: f32) -> HeldPose {
        HeldPose {
            first_person: mod_api::HeldPoseData {
                rotation: [0.0; 3],
                translation: [0.0, y, 0.0],
            },
            third_person: mod_api::HeldPoseData::IDENTITY,
        }
    }

    /// Two claimants wanting the same body at once is the case a single slot
    /// gets wrong: the speed scales must MULTIPLY, and one releasing must not
    /// take the other's claim with it.
    #[test]
    fn claims_compose_per_claimant_and_release_independently() {
        let mut body = BodyClaims::default();
        assert_eq!(body.speed_scale(), 1.0);

        assert!(body.set_speed_scale("combat", 0.5));
        assert!(body.set_speed_scale("armour", 0.8));
        assert_eq!(body.speed_scale(), 0.4);

        // Re-stating replaces that claimant's own value, never accumulates.
        assert!(body.set_speed_scale("combat", 0.5));
        assert_eq!(body.speed_scale(), 0.4);

        assert!(body.set_speed_scale("combat", 1.0));
        assert_eq!(body.speed_scale(), 0.8, "the other claim stands");
        assert!(body.set_speed_scale("armour", 1.0));
        assert_eq!(body.speed_scale(), 1.0);
        assert!(body.is_empty(), "a fully released body keeps no claims");
    }

    /// Denials UNION and a release only takes back its own: nobody may un-bar
    /// what another barred, and "may this body mine" must not depend on which
    /// claimant the fold reached last. (Poses are last-wins for the opposite
    /// reason — a hand holds one item — so crossing these two rules is a live
    /// hazard.)
    #[test]
    fn denials_union_across_claimants_and_release_only_their_own() {
        use mod_api::BodyAction::{Attack, Mine};
        let mut body = BodyClaims::default();
        assert!(body.denied_actions().is_empty());

        body.set_denied_actions("combat", DeniedActions::of([Attack, Mine]));
        body.set_denied_actions("binding", DeniedActions::of([Mine]));
        assert!(body.denied_actions().denies(Attack));
        assert!(body.denied_actions().denies(Mine));

        // The shield lowers; the binding spell still holds the pick.
        body.set_denied_actions("combat", DeniedActions::NONE);
        assert!(!body.denied_actions().denies(Attack));
        assert!(body.denied_actions().denies(Mine), "the other claim stands");

        body.set_denied_actions("binding", DeniedActions::NONE);
        assert!(body.denied_actions().is_empty());
        assert!(body.is_empty());
    }

    /// The pose's release rule is the whole reason it is keyed: one claimant
    /// clearing its hand must uncover another's pose, not blank the hand.
    #[test]
    fn a_released_pose_uncovers_the_other_claim() {
        let mut body = BodyClaims::default();
        assert!(body.set_held_pose("aaa", Some(pose(1.0)), None));
        assert!(body.set_held_pose("zzz", Some(pose(2.0)), None));
        assert_eq!(body.held_pose(Hand::Main), Some(pose(2.0)), "last in order");

        assert!(body.set_held_pose("zzz", None, None));
        assert_eq!(body.held_pose(Hand::Main), Some(pose(1.0)));
        assert_eq!(body.held_pose(Hand::Off), None);
    }

    /// A NaN would poison every transform downstream of the hand, and a
    /// non-finite scale would do the same to the movement integrator: both are
    /// refused whole rather than clamped into something plausible.
    #[test]
    fn non_finite_claims_are_refused_and_clamps_bound_the_rest() {
        let mut body = BodyClaims::default();
        assert!(!body.set_speed_scale("combat", f32::NAN));
        assert!(body.is_empty(), "a refused claim stores nothing");

        let mut nan = pose(0.0);
        nan.third_person.rotation[1] = f32::INFINITY;
        assert!(!body.set_held_pose("combat", Some(nan), None));
        assert_eq!(body.held_pose(Hand::Main), None);

        // A single wild claim is clamped, and so is a product of many.
        assert!(body.set_speed_scale("combat", 1e9));
        assert_eq!(body.speed_scale(), MOVE_SCALE_MAX);
        assert!(body.set_speed_scale("armour", -3.0));
        assert_eq!(body.speed_scale(), 0.0, "negative clamps to a rooted body");
    }

    /// Bone offsets are the one claim that genuinely COMPOSES across
    /// claimants, so every one is handed through, however many there are.
    #[test]
    fn bone_offsets_from_every_claimant_apply() {
        let bend = |bone: u16, deg: f32| BonePose {
            bone,
            rotation: [deg, 0.0, 0.0],
            translation: [0.0; 3],
            hold: false,
        };
        let mut body = BodyClaims::default();
        assert!(body.set_bone_poses("aaa", vec![bend(3, -22.0)]));
        assert!(body.set_bone_poses("zzz", vec![bend(7, 5.0)]));
        let got: Vec<_> = body.bone_poses().map(|b| b.bone).collect();
        assert_eq!(got, [3, 7], "both bends apply, in claimant order");

        assert!(body.set_bone_poses("aaa", Vec::new()));
        assert_eq!(body.bone_poses().map(|b| b.bone).collect::<Vec<_>>(), [7]);

        // Nothing caps the count: a rig with more joints than today's must not
        // silently lose the offsets past some number picked out of the air.
        let many: Vec<_> = (0..32).map(|i| bend(i, 1.0)).collect();
        assert!(body.set_bone_poses("aaa", many));
        assert_eq!(body.bone_poses().count(), 33);

        assert!(!body.set_bone_poses("aaa", vec![bend(1, f32::NAN)]));
        assert_eq!(
            body.bone_poses().count(),
            33,
            "a refused list stores nothing"
        );
    }

    /// The neutral write is the COMMON one: a claimant re-states its claims
    /// for every player every tick and almost none of them are carrying the
    /// thing it cares about, so publishing "nothing" must not allocate a claim
    /// just to prune it again.
    #[test]
    fn a_neutral_write_from_an_unclaimed_body_touches_nothing() {
        let mut body = BodyClaims::default();
        assert!(body.set_speed_scale("combat", 1.0));
        assert!(body.set_held_pose("combat", None, None));
        assert!(body.set_bone_poses("combat", Vec::new()));
        assert!(body.is_empty(), "no claim was ever built");

        // ...and it still RELEASES a claim that does exist.
        assert!(body.set_speed_scale("combat", 0.5));
        assert!(body.set_speed_scale("combat", 1.0));
        assert!(body.is_empty());
    }

    /// An identity pose means "no pose"; publishing it every tick must not pin
    /// a claim (or a replicated wire field) alive.
    #[test]
    fn an_identity_pose_is_not_a_claim() {
        let mut body = BodyClaims::default();
        assert!(body.set_held_pose("combat", Some(HeldPose::IDENTITY), None));
        assert!(body.is_empty());
        assert_eq!(body.held_pose(Hand::Main), None);
    }
}
