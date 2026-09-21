//! Transient claims on a player's body: the scales on its engine
//! quantities (speed, the attack cooldown), what it may do,
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

use std::sync::OnceLock;

use mod_api::{BodyAction, HeldPose, PlayerAttribute};

use super::rigs::{self, RigId};
use serde::{Deserialize, Serialize};

use petramond_world::inventory::Hand;
use petramond_world::item::ItemType;

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

/// The scale every attribute runs at with nothing claimed — a claim at this
/// value IS a release.
pub const ATTRIBUTE_DEFAULT: f32 = 1.0;

/// [`ATTRIBUTE_DEFAULT`] under its older, movement-specific name — the
/// callers that mean "the unscaled land speed" read better saying so.
pub const MOVE_SCALE_DEFAULT: f32 = ATTRIBUTE_DEFAULT;

/// Widest land-speed scale the resolved product may reach — generous enough
/// for any haste, low enough that a hostile or typo'd claim cannot fling a
/// body past what the terrain streamer keeps up with. Matches the
/// status-effect row bound.
pub const MOVE_SCALE_MAX: f32 = 5.0;

/// The widest resolved product each attribute may reach. Per quantity,
/// because the harm a runaway scale can do is per quantity.
fn attribute_max(attribute: PlayerAttribute) -> f32 {
    match attribute {
        PlayerAttribute::MoveSpeed => MOVE_SCALE_MAX,
        // Ten times the engine cooldown (~3 s between swings): slower than
        // that is no longer an attack rate, it is a bar on attacking — which
        // is the denial claim's job, not a number's.
        PlayerAttribute::AttackCooldown => 10.0,
    }
}

/// One scale per [`PlayerAttribute`], indexed by declaration order
/// ([`PlayerAttribute::index`]) and sized by the vocabulary itself.
type AttributeScales = [f32; PlayerAttribute::ALL.len()];

const ATTRIBUTES_RELEASED: AttributeScales = [ATTRIBUTE_DEFAULT; PlayerAttribute::ALL.len()];

pub use mod_api::{AnimatorClock, AnimatorValue};

/// One graph param a claimant sets on one rig, resolved below the ABI: the
/// rig by its registry id, the param as its graph declared it (by index)
/// and the value it takes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AnimatorParam {
    pub rig: RigId,
    pub param: u16,
    pub value: AnimatorValue,
}

/// One montage a claimant holds in one slot of one rig, resolved below the
/// ABI: the rig by its registry id, slot and clip by their graph and
/// library indices, on the claimant's clock.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct AnimatorPlay {
    pub rig: RigId,
    pub slot: u16,
    pub clip: u16,
    pub clock: AnimatorClock,
    pub mirror: bool,
    pub priority: i32,
}

impl AnimatorPlay {
    /// The scrub progress, when this play is on the caller's clock.
    pub fn progress(&self) -> Option<f32> {
        match self.clock {
            AnimatorClock::Scrub(progress) => Some(progress),
            AnimatorClock::Run { .. } => None,
        }
    }
}

/// Everything claimed on a body's rig animators: the resolved answer the
/// wire carries and a driver applies. Sorted by `(rig, id)` so two folds of
/// the same claims are byte-equal.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct AnimatorClaims {
    pub params: Vec<AnimatorParam>,
    pub plays: Vec<AnimatorPlay>,
}

impl AnimatorClaims {
    pub fn is_empty(&self) -> bool {
        self.params.is_empty() && self.plays.is_empty()
    }

    /// Order by key, and where one key appears more than once keep the
    /// LAST entry — a later duplicate in a caller's list, or a later
    /// claimant's in the fold, wins. Stable sorts keep that order.
    fn sort(&mut self) {
        self.params.sort_by_key(|p| (p.rig, p.param));
        keep_last(&mut self.params, |p| (p.rig, p.param));
        self.plays.sort_by_key(|p| (p.rig, p.slot));
        keep_last(&mut self.plays, |p| (p.rig, p.slot));
    }

    /// Keep only the claims on rigs other players OBSERVE — what a player
    /// row carries; the player's own state keeps every rig.
    pub fn retain_observed(&mut self) {
        self.params.retain(|p| rigs::observed(p.rig));
        self.plays.retain(|p| rigs::observed(p.rig));
    }
}

/// Of each run of equal keys in the sorted `items`, keep only the last.
fn keep_last<T, K: PartialEq>(items: &mut Vec<T>, key: impl Fn(&T) -> K) {
    let mut write = 0;
    for read in 0..items.len() {
        let last_of_run = read + 1 == items.len() || key(&items[read]) != key(&items[read + 1]);
        if last_of_run {
            items.swap(write, read);
            write += 1;
        }
    }
    items.truncate(write);
}

/// One claimant's claim on one body.
#[derive(Clone, Debug, PartialEq)]
struct Claim {
    /// Who is claiming — also the deterministic ordering key, so the fold
    /// never depends on who happened to call first.
    claimant: Box<str>,
    /// One multiplier per engine quantity ([`ATTRIBUTE_DEFAULT`] =
    /// released).
    attributes: AttributeScales,
    main: Option<HeldPose>,
    off: Option<HeldPose>,
    /// What each hand DISPLAYS in place of its stack's own art (`[main,
    /// off]`); `None` = the stack's own.
    display: [Option<ItemType>; 2],
    bones: Vec<BonePose>,
    denied: DeniedActions,
    /// This claimant's params and plays on the body's rig animators; empty
    /// = released.
    animator: AnimatorClaims,
}

impl Claim {
    /// Nothing left to say: the claim is dropped rather than kept at its
    /// neutral value, so the vector holds exactly the live claimants.
    fn is_released(&self) -> bool {
        self.attributes == ATTRIBUTES_RELEASED
            && self.main.is_none()
            && self.off.is_none()
            && self.display == [None; 2]
            && self.bones.is_empty()
            && self.denied.is_empty()
            && self.animator.is_empty()
    }
}

/// Every claim on one body, in claimant order.
#[derive(Clone, Debug, Default)]
pub struct BodyClaims {
    by_claimant: Vec<Claim>,
    /// The animator fold, resolved once per change: every player row reads
    /// it every tick, so it is cached and emptied by each animator write.
    resolved_animator: OnceLock<AnimatorClaims>,
}

impl PartialEq for BodyClaims {
    fn eq(&self, other: &Self) -> bool {
        self.by_claimant == other.by_claimant
    }
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
        self.resolved_animator = OnceLock::new();
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
                        attributes: ATTRIBUTES_RELEASED,
                        main: None,
                        off: None,
                        display: [None; 2],
                        bones: Vec::new(),
                        denied: DeniedActions::NONE,
                        animator: AnimatorClaims::default(),
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

    /// Set `claimant`'s multiplier on one engine quantity, replacing its
    /// previous value. Non-finite is refused (`false`); finite values clamp
    /// into `[0, attribute_max]` so no single claim can reverse a quantity
    /// or fling it past what the engine tolerates.
    pub fn set_attribute(
        &mut self,
        claimant: &str,
        attribute: PlayerAttribute,
        scale: f32,
    ) -> bool {
        if !scale.is_finite() {
            return false;
        }
        let scale = scale.clamp(0.0, attribute_max(attribute));
        if scale == ATTRIBUTE_DEFAULT && !self.holds(claimant) {
            return true;
        }
        let at = self.slot(claimant);
        self.by_claimant[at].attributes[attribute.index()] = scale;
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

    /// Set what `claimant` has each hand display in place of the held
    /// stack's own art (`None` releases a hand). Infallible: an item id is
    /// resolved at the ABI, so there is nothing malformed to refuse.
    pub fn set_held_display(
        &mut self,
        claimant: &str,
        main: Option<ItemType>,
        off: Option<ItemType>,
    ) {
        if main.is_none() && off.is_none() && !self.holds(claimant) {
            return;
        }
        let at = self.slot(claimant);
        self.by_claimant[at].display = [main, off];
        self.prune(at);
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

    /// Set the graph params `claimant` holds on the body's rigs (an empty
    /// list releases them). Refused (`false`) whole if a number is
    /// non-finite. One entry per `(rig, param)`: a later duplicate in the
    /// list replaces the earlier.
    pub fn set_animator_params(&mut self, claimant: &str, params: Vec<AnimatorParam>) -> bool {
        if params
            .iter()
            .any(|p| matches!(p.value, AnimatorValue::Number(v) if !v.is_finite()))
        {
            return false;
        }
        if params.is_empty() && !self.holds(claimant) {
            return true;
        }
        let at = self.slot(claimant);
        let mine = &mut self.by_claimant[at].animator;
        mine.params = params;
        mine.sort();
        self.prune(at);
        self.resolved_animator = OnceLock::new();
        true
    }

    /// Set the montages `claimant` holds in the body's rig slots (an empty
    /// list releases them). Refused (`false`) whole if a clock's progress
    /// or rate is non-finite; a scrub progress clamps into `0..=1`. One
    /// entry per `(rig, slot)`: a later duplicate in the list replaces the
    /// earlier.
    pub fn set_animator_plays(&mut self, claimant: &str, plays: Vec<AnimatorPlay>) -> bool {
        let finite = |clock: &AnimatorClock| match clock {
            AnimatorClock::Scrub(progress) => progress.is_finite(),
            AnimatorClock::Run { rate, .. } => rate.is_finite(),
        };
        if plays.iter().any(|p| !finite(&p.clock)) {
            return false;
        }
        if plays.is_empty() && !self.holds(claimant) {
            return true;
        }
        let at = self.slot(claimant);
        let mine = &mut self.by_claimant[at].animator;
        mine.plays = plays;
        for p in &mut mine.plays {
            if let AnimatorClock::Scrub(progress) = &mut p.clock {
                *progress = progress.clamp(0.0, 1.0);
            }
        }
        mine.sort();
        self.prune(at);
        self.resolved_animator = OnceLock::new();
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

    /// The resolved answer for one attribute over the claims a MIRROR cannot
    /// work out for itself — everything except [`ENGINE_CLAIMANT`]'s, which
    /// both sides derive from state they both hold (the effect list, the
    /// mode, the open menu).
    ///
    /// Replication sends THIS, not the full fold. Sending the whole answer
    /// would make the engine's own half arrive a batch late and stop being
    /// predicted, which is the opposite of what folding it in was for; sending
    /// neither half would leave the mirror guessing at rules only the server
    /// can see. Each side computes the half it can and is told the half it
    /// cannot.
    pub fn replicated_attribute(&self, attribute: PlayerAttribute) -> f32 {
        self.by_claimant
            .iter()
            .filter(|c| &*c.claimant != ENGINE_CLAIMANT)
            .map(|c| c.attributes[attribute.index()])
            .product::<f32>()
            .clamp(0.0, attribute_max(attribute))
    }

    /// [`replicated_attribute`](Self::replicated_attribute) for the barred
    /// set, and for the same reason.
    pub fn replicated_denied_actions(&self) -> DeniedActions {
        self.by_claimant
            .iter()
            .filter(|c| &*c.claimant != ENGINE_CLAIMANT)
            .fold(DeniedActions::NONE, |set, c| set.union(c.denied))
    }

    /// The resolved multiplier on one engine quantity: the PRODUCT of every
    /// claim, clamped once at the end. A product is the composable answer —
    /// two slows both apply, neither stomps the other, and the order they
    /// were set in cannot change the result.
    pub fn attribute(&self, attribute: PlayerAttribute) -> f32 {
        self.by_claimant
            .iter()
            .map(|c| c.attributes[attribute.index()])
            .product::<f32>()
            .clamp(0.0, attribute_max(attribute))
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

    /// The resolved animator claims: per `(rig, param)` and per `(rig,
    /// slot)` the LAST claimant in order wins, by the pose rule — a param
    /// holds one value and a slot plays one thing, so two live claims on
    /// one are a conflict, not a composition; keying still lets one
    /// claimant's release uncover another's.
    pub fn animator(&self) -> &AnimatorClaims {
        self.resolved_animator.get_or_init(|| {
            let mut out = AnimatorClaims::default();
            for c in &self.by_claimant {
                out.params.extend(c.animator.params.iter().cloned());
                out.plays.extend(c.animator.plays.iter().copied());
            }
            out.sort();
            out
        })
    }

    /// The resolved DISPLAY for one hand — the item whose art draws in place
    /// of the held stack's — by the pose rule: the LAST claim in claimant
    /// order, since a hand shows one thing.
    pub fn held_display(&self, hand: Hand) -> Option<ItemType> {
        let hand = match hand {
            Hand::Main => 0,
            Hand::Off => 1,
        };
        self.by_claimant.iter().rev().find_map(|c| c.display[hand])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Animator claims resolve per `(rig, slot)` and `(rig, param)` with the
    /// pose rule: the later claimant wins, a release uncovers the earlier
    /// claim, a non-finite value is refused whole, and a progress clamps.
    #[test]
    fn animator_claims_resolve_per_slot_and_param_last_wins_and_release_uncovers() {
        let play = |slot: u16, clip: u16, progress: f32| AnimatorPlay {
            rig: RigId(1),
            slot,
            clip,
            clock: AnimatorClock::Scrub(progress),
            mirror: false,
            priority: 0,
        };
        let param = |name: u16, value: f32| AnimatorParam {
            rig: RigId(0),
            param: name,
            value: AnimatorValue::Number(value),
        };
        let mut claims = BodyClaims::default();
        assert!(claims.set_animator_plays("alpha", vec![play(0, 1, 0.25), play(1, 5, 0.0)]));
        assert!(claims.set_animator_plays("beta", vec![play(0, 2, 1.5)]));
        assert!(claims.set_animator_params("alpha", vec![param(3, 1.0)]));
        let resolved = claims.animator().clone();
        assert_eq!(
            resolved
                .plays
                .iter()
                .map(|p| (p.slot, p.clip, p.progress()))
                .collect::<Vec<_>>(),
            [(0, 2, Some(1.0)), (1, 5, Some(0.0))],
            "the later claimant wins slot 0, clamped; slot 1 stands"
        );
        assert_eq!(resolved.params, [param(3, 1.0)]);

        assert!(!claims.set_animator_plays("beta", vec![play(0, 3, f32::NAN)]));
        assert!(!claims.set_animator_params("beta", vec![param(3, f32::INFINITY)]));
        assert_eq!(
            claims.animator(),
            &resolved,
            "a refused write changes nothing"
        );

        assert!(claims.set_animator_plays("beta", Vec::new()));
        assert_eq!(
            claims.animator().plays[0].clip,
            1,
            "a release uncovers the earlier claim"
        );
        assert!(claims.set_animator_plays("alpha", Vec::new()));
        assert!(claims.set_animator_params("alpha", Vec::new()));
        assert!(claims.is_empty());
    }

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
        assert_eq!(body.attribute(PlayerAttribute::MoveSpeed), 1.0);

        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 0.5));
        assert!(body.set_attribute("armour", PlayerAttribute::MoveSpeed, 0.8));
        assert_eq!(body.attribute(PlayerAttribute::MoveSpeed), 0.4);

        // Re-stating replaces that claimant's own value, never accumulates.
        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 0.5));
        assert_eq!(body.attribute(PlayerAttribute::MoveSpeed), 0.4);

        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 1.0));
        assert_eq!(
            body.attribute(PlayerAttribute::MoveSpeed),
            0.8,
            "the other claim stands"
        );
        assert!(body.set_attribute("armour", PlayerAttribute::MoveSpeed, 1.0));
        assert_eq!(body.attribute(PlayerAttribute::MoveSpeed), 1.0);
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
        assert!(!body.set_attribute("combat", PlayerAttribute::MoveSpeed, f32::NAN));
        assert!(body.is_empty(), "a refused claim stores nothing");

        let mut nan = pose(0.0);
        nan.third_person.rotation[1] = f32::INFINITY;
        assert!(!body.set_held_pose("combat", Some(nan), None));
        assert_eq!(body.held_pose(Hand::Main), None);

        // A single wild claim is clamped, and so is a product of many.
        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 1e9));
        assert_eq!(body.attribute(PlayerAttribute::MoveSpeed), MOVE_SCALE_MAX);
        assert!(body.set_attribute("armour", PlayerAttribute::MoveSpeed, -3.0));
        assert_eq!(
            body.attribute(PlayerAttribute::MoveSpeed),
            0.0,
            "negative clamps to a rooted body"
        );
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
        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 1.0));
        assert!(body.set_held_pose("combat", None, None));
        assert!(body.set_bone_poses("combat", Vec::new()));
        assert!(body.is_empty(), "no claim was ever built");

        // ...and it still RELEASES a claim that does exist.
        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 0.5));
        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 1.0));
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
