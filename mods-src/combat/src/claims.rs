//! One rule's claims on a body, in the seams' vocabulary; the RULE
//! interface every use-press feature of the pack implements; and how the
//! rules' claims become ONE publish.
//!
//! The pack's rules (the guard, the bow, the next one) sit in ONE ordered
//! list — precedence is list order — and share one interface: whether a
//! fresh press is theirs, how their clocks advance, and what they claim
//! about the body this tick. The raise handler asks the list who takes a
//! press; the tick, the frame and the damage handlers fold [`compose`]
//! over it. Nothing here knows which rule a claim came from, and the fact
//! that an EARLIER rule already holds the press reaches a later one through
//! [`Body::press`], never through a doctored snapshot. Adding a rule is one
//! list entry.

use crate::body::BodyClocks;
use mod_sdk::*;

/// A frontal arc within which a raised guard absorbs a hit: the cosine of
/// the widest angle off the look direction it still covers.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Cover {
    pub arc_cos: f32,
}

impl Cover {
    /// Is a hit from `origin` one this cover is in front of?
    ///
    /// HORIZONTAL only: pitch is where the player is LOOKING, not where the
    /// shield is, so gating on it would drop the guard every time somebody
    /// glanced down.
    ///
    /// No origin means no direction to judge, so the guard holds — refusing
    /// on missing spatial context would quietly break the shield for any
    /// future damage source that omits it.
    pub fn covers(self, state: &PlayerSnapshot, origin: Option<[f32; 3]>) -> bool {
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
        (state.yaw.sin() * dx + state.yaw.cos() * dz) / distance >= self.arc_cos
    }
}

/// What one rule claims about a body this tick. Everything releases by
/// default, so a rule that is not in play contributes nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct Claims {
    /// The rule holds the use press: the hands are its, so the tools' swing
    /// claim stands down while it does.
    pub holds_press: bool,
    /// Land-speed multiplier (`1.0` releases it). Multiplies across rules.
    pub speed: f32,
    /// Attack-cooldown multiplier (`1.0` releases it). Multiplies across
    /// rules.
    pub cooldown: f32,
    /// What the body may not do while the rule is in play. Unions across
    /// rules — a denial has no conflict to resolve. Kept a set (no
    /// duplicates) in insertion order, which is deterministic where a hash
    /// set's order would not be.
    pub denied: Vec<BodyAction>,
    /// The hand motions this pack animates itself, per hand (`[main, off]`),
    /// so the engine's own stand down. Unions across rules.
    pub hands: [Vec<HandMotion>; 2],
    /// What each hand DISPLAYS in place of its stack (`[main, off]`, by
    /// registry name); `None` = the stack's own art.
    pub display: [Option<String>; 2],
    /// Each hand's held pose; `None` = the authored hold. A hand holds one
    /// item, so the EARLIER rule's pose wins a hand it poses.
    pub main: Option<HeldPose>,
    pub off: Option<HeldPose>,
    /// Rig bone offsets. Extend across rules — a rule owns the joints it
    /// names, and two rules naming one joint is a pack bug, not a merge.
    pub bones: Vec<BonePoseData>,
    /// The body absorbs hits arriving inside this frontal arc. A body has
    /// one guard, so the earlier rule's stands.
    pub cover: Option<Cover>,
}

impl Default for Claims {
    fn default() -> Self {
        Claims {
            holds_press: false,
            speed: 1.0,
            cooldown: 1.0,
            denied: Vec::new(),
            hands: [Vec::new(), Vec::new()],
            display: [None, None],
            main: None,
            off: None,
            bones: Vec::new(),
            cover: None,
        }
    }
}

fn union<T: PartialEq>(into: &mut Vec<T>, from: Vec<T>) {
    for item in from {
        if !into.contains(&item) {
            into.push(item);
        }
    }
}

impl Claims {
    /// Compose `later` UNDER these claims: a hand, display or cover this
    /// rule states stays its own, everything else composes (multipliers
    /// multiply, denials and motions union, bones extend, the press is held
    /// if either holds it).
    pub fn over(mut self, later: Claims) -> Claims {
        self.holds_press |= later.holds_press;
        self.speed *= later.speed;
        self.cooldown *= later.cooldown;
        union(&mut self.denied, later.denied);
        let [main_hands, off_hands] = later.hands;
        union(&mut self.hands[0], main_hands);
        union(&mut self.hands[1], off_hands);
        let [main_display, off_display] = later.display;
        self.display[0] = self.display[0].take().or(main_display);
        self.display[1] = self.display[1].take().or(off_display);
        self.main = self.main.or(later.main);
        self.off = self.off.or(later.off);
        self.bones.extend(later.bones);
        self.cover = self.cover.or(later.cover);
        self
    }

    /// Does this body's guard stop a hit arriving from `origin`? No cover,
    /// no block.
    pub fn covers(&self, state: &PlayerSnapshot, origin: Option<[f32; 3]>) -> bool {
        self.cover.is_some_and(|cover| cover.covers(state, origin))
    }
}

/// What a rule sees of one body when resolving its claims.
pub struct Body<'a> {
    pub state: &'a PlayerSnapshot,
    pub clocks: &'a BodyClocks,
    /// The use press is THIS rule's: the button is down, this rule took it
    /// when it was pressed, and no earlier rule in the list holds it.
    pub press: bool,
}

/// One use-press rule of the pack.
pub trait Rule {
    /// Whether a FRESH press by `state`'s body is this rule's. The raise
    /// handler asks the list in order and the first taker holds the
    /// gesture until the button comes up. The one place a rule may read
    /// the inventory — never per tick or frame.
    fn takes_press(&self, state: &PlayerSnapshot) -> bool;

    /// Advance this rule's clocks for `player`'s body by `dt_ticks`.
    /// `press` is [`Body::press`] for this rule; on the `authority` the
    /// rule also acts on any edge its clock reports (a loosed arrow) — a
    /// mirror only shows.
    fn step(
        &self,
        clocks: &mut BodyClocks,
        player: PlayerId,
        state: &PlayerSnapshot,
        press: bool,
        dt_ticks: f32,
        authority: bool,
    );

    /// This rule's claims on the body this tick.
    fn claims(&self, body: &Body) -> Claims;
}

/// Which rule takes a fresh press from `state`'s body: the first in the
/// list that does, as its index.
pub fn taker(rules: &[Box<dyn Rule>], state: &PlayerSnapshot) -> Option<usize> {
    rules.iter().position(|rule| rule.takes_press(state))
}

/// Whether the body's current press belongs to the rule at `index`: the
/// button is down and that rule took it.
pub fn presses(clocks: &BodyClocks, state: &PlayerSnapshot, index: usize) -> bool {
    state.holds_use && clocks.press_owner == Some(index)
}

/// The rules' claims on one body, folded in list order. A rule holding the
/// press denies it to every later rule — the composition carries that fact,
/// so no rule is handed a snapshot that lies about the button.
pub fn compose(rules: &[Box<dyn Rule>], state: &PlayerSnapshot, clocks: &BodyClocks) -> Claims {
    let mut free = true;
    let mut merged = Claims::default();
    for (index, rule) in rules.iter().enumerate() {
        let press = free && presses(clocks, state, index);
        let claims = rule.claims(&Body {
            state,
            clocks,
            press,
        });
        free &= !claims.holds_press;
        merged = merged.over(claims);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(px: f32) -> HeldPose {
        HeldPose {
            first_person: HeldPoseData {
                rotation: [0.0; 3],
                translation: [px, 0.0, 0.0],
            },
            third_person: HeldPoseData::IDENTITY,
        }
    }

    /// The merge is the pack's one precedence rule: a posed hand is the
    /// earlier rule's, the rest composes. Getting this backwards would have
    /// a shield's carry pose overwrite a drawn bow, or a bow's 0.6× speed
    /// discard the guard's 0.5×; a denial stated twice must not be written
    /// twice.
    #[test]
    fn earlier_poses_win_hands_and_everything_else_composes() {
        let bow = Claims {
            holds_press: true,
            speed: 0.6,
            denied: vec![BodyAction::Attack],
            display: [Some("m:pull".into()), None],
            main: Some(pose(1.0)),
            ..Default::default()
        };
        let guard = Claims {
            speed: 0.5,
            denied: vec![BodyAction::Attack, BodyAction::Mine],
            main: Some(pose(2.0)),
            off: Some(pose(3.0)),
            cover: Some(Cover { arc_cos: 0.5 }),
            ..Default::default()
        };
        let merged = bow.over(guard);
        assert!(merged.holds_press);
        assert!((merged.speed - 0.3).abs() < 1e-6, "speeds multiply");
        assert_eq!(merged.denied, [BodyAction::Attack, BodyAction::Mine]);
        assert_eq!(merged.display[0].as_deref(), Some("m:pull"));
        assert_eq!(merged.main, Some(pose(1.0)), "the bow keeps the main hand");
        assert_eq!(merged.off, Some(pose(3.0)), "the guard's off hand shows");
        assert_eq!(merged.cover, Some(Cover { arc_cos: 0.5 }));
        assert_eq!(Claims::default().over(Claims::default()), Claims::default());
    }

    fn state(yaw: f32) -> PlayerSnapshot {
        PlayerSnapshot {
            id: Some(PlayerId(0)),
            pos: [0.0; 3],
            vel: [0.0; 3],
            yaw,
            pitch: 0.0,
            health: 20,
            on_ground: true,
            spectator: false,
            sneak: false,
            use_held: true,
            holds_use: true,
            held: None,
            off_held: None,
            held_count: 1,
            pose_anchor: None,
            swing: Default::default(),
            half_width: 0.3,
            height: 1.8,
            eye_height: 1.62,
        }
    }

    /// A cover stops what the player is FACING. Getting the yaw convention
    /// backwards blocks exactly the hits it should let through, and no
    /// other test would notice.
    #[test]
    fn a_cover_is_the_front_arc_only() {
        let cover = Cover { arc_cos: 0.5 };
        let s = state(0.0);
        // Yaw 0 faces +Z.
        assert!(cover.covers(&s, Some([0.0, 0.0, 4.0])), "dead ahead");
        assert!(cover.covers(&s, Some([1.0, 0.0, 4.0])), "just off centre");
        assert!(!cover.covers(&s, Some([4.0, 0.0, 0.0])), "side");
        assert!(!cover.covers(&s, Some([-4.0, 0.0, 0.0])), "other side");
        assert!(!cover.covers(&s, Some([0.0, 0.0, -4.0])), "behind");

        // The arc turns with the player, not with the world.
        let s = state(std::f32::consts::FRAC_PI_2);
        assert!(cover.covers(&s, Some([4.0, 0.0, 0.0])));
        assert!(!cover.covers(&s, Some([0.0, 0.0, 4.0])));

        // Height is not part of it: a hit from directly above is still
        // frontal if the attacker is in front, and an attacker sharing the
        // column has no direction at all.
        assert!(cover.covers(&s, Some([4.0, 9.0, 0.0])));
        assert!(cover.covers(&s, Some(s.pos)));
        assert!(
            cover.covers(&s, None),
            "no origin, no direction to refuse on"
        );
        assert!(
            !Claims::default().covers(&s, Some([4.0, 0.0, 0.0])),
            "no cover, no block"
        );
    }

    /// A rule that answers only "the press is mine" with nothing else.
    struct Holder;
    impl Rule for Holder {
        fn takes_press(&self, _: &PlayerSnapshot) -> bool {
            true
        }
        fn step(
            &self,
            _: &mut BodyClocks,
            _: PlayerId,
            _: &PlayerSnapshot,
            _: bool,
            _: f32,
            _: bool,
        ) {
        }
        fn claims(&self, body: &Body) -> Claims {
            Claims {
                holds_press: body.press,
                speed: if body.press { 0.5 } else { 1.0 },
                ..Default::default()
            }
        }
    }

    /// The press reaches exactly ONE rule: the one that took it, and only
    /// while nothing earlier in the list holds it. A later rule seeing a
    /// press the earlier one owns is the off-hand shield popping up under a
    /// drawn bow.
    #[test]
    fn the_press_belongs_to_the_rule_that_took_it_and_earlier_rules_shadow_it() {
        let rules: Vec<Box<dyn Rule>> = vec![Box::new(Holder), Box::new(Holder)];
        let s = state(0.0);
        let mut clocks = BodyClocks::default();
        assert_eq!(taker(&rules, &s), Some(0), "the first taker wins");

        clocks.press_owner = Some(1);
        let merged = compose(&rules, &s, &clocks);
        assert!(merged.holds_press);
        assert_eq!(merged.speed, 0.5, "one rule claims, not both");

        clocks.press_owner = Some(0);
        assert_eq!(compose(&rules, &s, &clocks).speed, 0.5);

        let mut released = s.clone();
        released.holds_use = false;
        assert!(!compose(&rules, &released, &clocks).holds_press);
        assert_eq!(compose(&rules, &released, &clocks).speed, 1.0);
    }
}
