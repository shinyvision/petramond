use std::sync::OnceLock;

use mod_api::{BodyAction, HeldPose, PlayerAttribute};

use super::rigs::{self, RigId};
use serde::{Deserialize, Serialize};

use petramond_world::inventory::Hand;
use petramond_world::item::ItemType;

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct BonePose {
    pub bone: u16,
    pub rotation: [f32; 3],
    pub translation: [f32; 3],
    pub hold: bool,
}

impl BonePose {
    fn is_finite(&self) -> bool {
        self.rotation
            .iter()
            .chain(&self.translation)
            .all(|c| c.is_finite())
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DeniedActions(u8);

impl DeniedActions {
    pub const NONE: Self = DeniedActions(0);

    fn bit(action: BodyAction) -> u8 {
        match action {
            BodyAction::Attack => 1 << 0,
            BodyAction::Mine => 1 << 1,
            BodyAction::Use => 1 << 2,
        }
    }

    pub fn of(actions: impl IntoIterator<Item = BodyAction>) -> Self {
        DeniedActions(actions.into_iter().fold(0, |m, a| m | Self::bit(a)))
    }

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

pub const ENGINE_CLAIMANT: &str = "petramond";

pub const ATTRIBUTE_DEFAULT: f32 = 1.0;

pub const MOVE_SCALE_DEFAULT: f32 = ATTRIBUTE_DEFAULT;

pub const MOVE_SCALE_MAX: f32 = 5.0;

pub const FLY_SCALE_MAX: f32 = 4.0;

fn attribute_max(attribute: PlayerAttribute) -> f32 {
    match attribute {
        PlayerAttribute::MoveSpeed => MOVE_SCALE_MAX,
        PlayerAttribute::AttackCooldown => 10.0,
        PlayerAttribute::FlySpeed => FLY_SCALE_MAX,
    }
}

type AttributeScales = [f32; PlayerAttribute::ALL.len()];

const ATTRIBUTES_RELEASED: AttributeScales = [ATTRIBUTE_DEFAULT; PlayerAttribute::ALL.len()];

pub use mod_api::{AnimatorClock, AnimatorValue};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AnimatorParam {
    pub rig: RigId,
    pub param: u16,
    pub value: AnimatorValue,
}

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
    pub fn progress(&self) -> Option<f32> {
        match self.clock {
            AnimatorClock::Scrub(progress) => Some(progress),
            AnimatorClock::Run { .. } => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct AnimatorClaims {
    pub params: Vec<AnimatorParam>,
    pub plays: Vec<AnimatorPlay>,
}

impl AnimatorClaims {
    pub fn is_empty(&self) -> bool {
        self.params.is_empty() && self.plays.is_empty()
    }

    fn sort(&mut self) {
        self.params.sort_by_key(|p| (p.rig, p.param));
        keep_last(&mut self.params, |p| (p.rig, p.param));
        self.plays.sort_by_key(|p| (p.rig, p.slot));
        keep_last(&mut self.plays, |p| (p.rig, p.slot));
    }

    pub fn retain_observed(&mut self) {
        self.params.retain(|p| rigs::observed(p.rig));
        self.plays.retain(|p| rigs::observed(p.rig));
    }
}

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

#[derive(Clone, Debug, PartialEq)]
struct Claim {
    claimant: Box<str>,
    attributes: AttributeScales,
    main: Option<HeldPose>,
    off: Option<HeldPose>,
    display: [Option<ItemType>; 2],
    bones: Vec<BonePose>,
    denied: DeniedActions,
    animator: AnimatorClaims,
}

impl Claim {
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

#[derive(Clone, Debug, Default)]
pub struct BodyClaims {
    by_claimant: Vec<Claim>,
    resolved_animator: OnceLock<AnimatorClaims>,
}

impl PartialEq for BodyClaims {
    fn eq(&self, other: &Self) -> bool {
        self.by_claimant == other.by_claimant
    }
}

impl BodyClaims {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.by_claimant.is_empty()
    }

    pub fn clear(&mut self) {
        self.by_claimant.clear();
        self.resolved_animator = OnceLock::new();
    }

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

    pub fn set_denied_actions(&mut self, claimant: &str, denied: DeniedActions) {
        if denied.is_empty() && !self.holds(claimant) {
            return;
        }
        let at = self.slot(claimant);
        self.by_claimant[at].denied = denied;
        self.prune(at);
    }

    pub fn denied_actions(&self) -> DeniedActions {
        self.by_claimant
            .iter()
            .fold(DeniedActions::NONE, |set, c| set.union(c.denied))
    }

    pub fn bone_poses(&self) -> impl Iterator<Item = BonePose> + '_ {
        self.by_claimant
            .iter()
            .flat_map(|c| c.bones.iter().copied())
    }

    pub fn has_bone_poses(&self) -> bool {
        self.by_claimant.iter().any(|c| !c.bones.is_empty())
    }

    pub fn replicated_attribute(&self, attribute: PlayerAttribute) -> f32 {
        self.by_claimant
            .iter()
            .filter(|c| &*c.claimant != ENGINE_CLAIMANT)
            .map(|c| c.attributes[attribute.index()])
            .product::<f32>()
            .clamp(0.0, attribute_max(attribute))
    }

    pub fn replicated_denied_actions(&self) -> DeniedActions {
        self.by_claimant
            .iter()
            .filter(|c| &*c.claimant != ENGINE_CLAIMANT)
            .fold(DeniedActions::NONE, |set, c| set.union(c.denied))
    }

    pub fn attribute(&self, attribute: PlayerAttribute) -> f32 {
        self.by_claimant
            .iter()
            .map(|c| c.attributes[attribute.index()])
            .product::<f32>()
            .clamp(0.0, attribute_max(attribute))
    }

    pub fn held_pose(&self, hand: Hand) -> Option<HeldPose> {
        self.by_claimant.iter().rev().find_map(|c| match hand {
            Hand::Main => c.main,
            Hand::Off => c.off,
        })
    }

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

    #[test]
    fn claims_compose_per_claimant_and_release_independently() {
        let mut body = BodyClaims::default();
        assert_eq!(body.attribute(PlayerAttribute::MoveSpeed), 1.0);

        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 0.5));
        assert!(body.set_attribute("armour", PlayerAttribute::MoveSpeed, 0.8));
        assert_eq!(body.attribute(PlayerAttribute::MoveSpeed), 0.4);

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

    #[test]
    fn denials_union_across_claimants_and_release_only_their_own() {
        use mod_api::BodyAction::{Attack, Mine};
        let mut body = BodyClaims::default();
        assert!(body.denied_actions().is_empty());

        body.set_denied_actions("combat", DeniedActions::of([Attack, Mine]));
        body.set_denied_actions("binding", DeniedActions::of([Mine]));
        assert!(body.denied_actions().denies(Attack));
        assert!(body.denied_actions().denies(Mine));

        body.set_denied_actions("combat", DeniedActions::NONE);
        assert!(!body.denied_actions().denies(Attack));
        assert!(body.denied_actions().denies(Mine), "the other claim stands");

        body.set_denied_actions("binding", DeniedActions::NONE);
        assert!(body.denied_actions().is_empty());
        assert!(body.is_empty());
    }

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

    #[test]
    fn non_finite_claims_are_refused_and_clamps_bound_the_rest() {
        let mut body = BodyClaims::default();
        assert!(!body.set_attribute("combat", PlayerAttribute::MoveSpeed, f32::NAN));
        assert!(body.is_empty(), "a refused claim stores nothing");

        let mut nan = pose(0.0);
        nan.third_person.rotation[1] = f32::INFINITY;
        assert!(!body.set_held_pose("combat", Some(nan), None));
        assert_eq!(body.held_pose(Hand::Main), None);

        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 1e9));
        assert_eq!(body.attribute(PlayerAttribute::MoveSpeed), MOVE_SCALE_MAX);
        assert!(body.set_attribute("armour", PlayerAttribute::MoveSpeed, -3.0));
        assert_eq!(
            body.attribute(PlayerAttribute::MoveSpeed),
            0.0,
            "negative clamps to a rooted body"
        );
    }

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

    #[test]
    fn a_neutral_write_from_an_unclaimed_body_touches_nothing() {
        let mut body = BodyClaims::default();
        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 1.0));
        assert!(body.set_held_pose("combat", None, None));
        assert!(body.set_bone_poses("combat", Vec::new()));
        assert!(body.is_empty(), "no claim was ever built");

        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 0.5));
        assert!(body.set_attribute("combat", PlayerAttribute::MoveSpeed, 1.0));
        assert!(body.is_empty());
    }

    #[test]
    fn an_identity_pose_is_not_a_claim() {
        let mut body = BodyClaims::default();
        assert!(body.set_held_pose("combat", Some(HeldPose::IDENTITY), None));
        assert!(body.is_empty());
        assert_eq!(body.held_pose(Hand::Main), None);
    }
}
