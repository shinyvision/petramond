use crate::body::BodyClocks;
use mod_sdk::*;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Cover {
    pub arc_cos: f32,
}

impl Cover {
    pub fn covers(self, state: &PlayerSnapshot, origin: Option<[f64; 3]>) -> bool {
        let Some(origin) = origin else {
            return true;
        };
        let (dx, dz) = (
            (origin[0] - state.pos[0]) as f32,
            (origin[2] - state.pos[2]) as f32,
        );
        let distance = (dx * dx + dz * dz).sqrt();
        if distance < 1e-4 {
            return true;
        }
        (state.yaw.sin() * dx + state.yaw.cos() * dz) / distance >= self.arc_cos
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Claims {
    pub holds_press: bool,
    pub speed: f32,
    pub cooldown: f32,
    pub denied: Vec<BodyAction>,
    pub params: Vec<AnimatorParam>,
    pub display: [Option<String>; 2],
    pub main: Option<HeldPose>,
    pub off: Option<HeldPose>,
    pub bones: Vec<BonePoseData>,
    pub plays: Vec<AnimatorPlay>,
    pub cover: Option<Cover>,
}

impl Default for Claims {
    fn default() -> Self {
        Claims {
            holds_press: false,
            speed: 1.0,
            cooldown: 1.0,
            denied: Vec::new(),
            params: Vec::new(),
            display: [None, None],
            main: None,
            off: None,
            bones: Vec::new(),
            plays: Vec::new(),
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
    pub fn over(mut self, later: Claims) -> Claims {
        self.holds_press |= later.holds_press;
        self.speed *= later.speed;
        self.cooldown *= later.cooldown;
        union(&mut self.denied, later.denied);
        for param in later.params {
            if !self
                .params
                .iter()
                .any(|p| (&p.rig, &p.param) == (&param.rig, &param.param))
            {
                self.params.push(param);
            }
        }
        let [main_display, off_display] = later.display;
        self.display[0] = self.display[0].take().or(main_display);
        self.display[1] = self.display[1].take().or(off_display);
        self.main = self.main.or(later.main);
        self.off = self.off.or(later.off);
        self.bones.extend(later.bones);
        for play in later.plays {
            if !self
                .plays
                .iter()
                .any(|p| (&p.rig, &p.slot) == (&play.rig, &play.slot))
            {
                self.plays.push(play);
            }
        }
        self.cover = self.cover.or(later.cover);
        self
    }

    pub fn covers(&self, state: &PlayerSnapshot, origin: Option<[f64; 3]>) -> bool {
        self.cover.is_some_and(|cover| cover.covers(state, origin))
    }
}

pub fn swing_claim(hand: usize) -> Vec<AnimatorParam> {
    let param = if hand == 0 {
        "main.swing_claim"
    } else {
        "off.swing_claim"
    };
    [rig::PLAYER_BODY, rig::PLAYER_FIRST_PERSON]
        .into_iter()
        .map(|rig| AnimatorParam {
            rig: rig.to_string(),
            param: param.to_string(),
            value: AnimatorValue::Number(1.0),
        })
        .collect()
}

pub struct Body<'a> {
    pub state: &'a PlayerSnapshot,
    pub clocks: &'a BodyClocks,
    pub press: bool,
}

pub trait Rule {
    fn takes_press(&self, state: &PlayerSnapshot) -> bool;

    fn step(
        &self,
        clocks: &mut BodyClocks,
        player: PlayerId,
        state: &PlayerSnapshot,
        press: bool,
        dt_ticks: f32,
        authority: bool,
    );

    fn claims(&self, body: &Body) -> Claims;
}

pub fn taker(rules: &[Box<dyn Rule>], state: &PlayerSnapshot) -> Option<usize> {
    rules.iter().position(|rule| rule.takes_press(state))
}

pub fn presses(clocks: &BodyClocks, state: &PlayerSnapshot, index: usize) -> bool {
    state.holds_use && clocks.press_owner == Some(index)
}

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
            entombed: false,
            conditions: Vec::new(),
        }
    }

    #[test]
    fn a_cover_is_the_front_arc_only() {
        let cover = Cover { arc_cos: 0.5 };
        let s = state(0.0);
        assert!(cover.covers(&s, Some([0.0, 0.0, 4.0])), "dead ahead");
        assert!(cover.covers(&s, Some([1.0, 0.0, 4.0])), "just off centre");
        assert!(!cover.covers(&s, Some([4.0, 0.0, 0.0])), "side");
        assert!(!cover.covers(&s, Some([-4.0, 0.0, 0.0])), "other side");
        assert!(!cover.covers(&s, Some([0.0, 0.0, -4.0])), "behind");

        let s = state(std::f32::consts::FRAC_PI_2);
        assert!(cover.covers(&s, Some([4.0, 0.0, 0.0])));
        assert!(!cover.covers(&s, Some([0.0, 0.0, 4.0])));

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
