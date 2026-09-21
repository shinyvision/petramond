use super::*;
use crate::mob::{def, Mob};
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;

fn floor_at_zero(p: IVec3) -> bool {
    p.y < 0
}

fn owl_def() -> &'static MobDef {
    def(Mob::Owl)
}

#[test]
fn ambient_clocks_are_individual_and_action_starts_at_zero() {
    use crate::mob::model_meta::NamedAnimMeta;
    let named = [
        NamedAnimMeta {
            name: clips::AMBIENT.into(),
            length: 7.0,
            looping: true,
        },
        NamedAnimMeta {
            name: "swipe".into(),
            length: 0.8,
            looping: false,
        },
    ];
    let mut a = Instance::new(Mob::Owl, WorldPos::ZERO, 0.0, 1);
    let mut b = Instance::new(Mob::Owl, WorldPos::ZERO, 0.0, 2);
    a.id = 1;
    b.id = 2;
    let decision = BehaviorOutput {
        animation: Some("swipe".into()),
        ..Default::default()
    };
    a.apply_expression(0.05, owl_def(), &named, &decision);
    b.apply_expression(0.05, owl_def(), &named, &decision);
    assert_eq!(a.anim_state("swipe").unwrap().phase, 0.0);
    let before = a.anim_state(clips::AMBIENT).unwrap().phase;
    assert_ne!(before, b.anim_state(clips::AMBIENT).unwrap().phase);
    a.moving = true;
    a.apply_expression(0.05, owl_def(), &named, &BehaviorOutput::default());
    assert!((a.anim_state(clips::AMBIENT).unwrap().phase - before - 0.05).abs() < 1e-5);
    assert!((a.anim_state("swipe").unwrap().phase - 0.05).abs() < 1e-5);
}

#[test]
fn expression_advances_walk_and_eases_the_head() {
    use super::super::brain::HeadLook;
    let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
    // A walking tick (integrate sets `moving`), then expression advances the walk.
    owl.integrate(
        1.0 / 60.0,
        owl_def(),
        Vec3::new(1.0, 0.0, 0.0),
        false,
        &floor_at_zero,
    );
    owl.apply_expression(1.0 / 60.0, owl_def(), &[], &BehaviorOutput::default());
    let a1 = owl.anim_time;
    owl.integrate(
        1.0 / 60.0,
        owl_def(),
        Vec3::new(1.0, 0.0, 0.0),
        false,
        &floor_at_zero,
    );
    owl.apply_expression(1.0 / 60.0, owl_def(), &[], &BehaviorOutput::default());
    assert!(
        owl.anim_time > a1,
        "walk cycle keeps advancing: {a1} -> {}",
        owl.anim_time
    );

    // Head eases toward a look target, then recentres when there's none.
    let look = BehaviorOutput {
        head_look: Some(HeadLook {
            yaw: 1.0,
            pitch: 0.3,
        }),
        ..Default::default()
    };
    for _ in 0..120 {
        owl.integrate(1.0 / 60.0, owl_def(), Vec3::ZERO, false, &floor_at_zero);
        owl.apply_expression(1.0 / 60.0, owl_def(), &[], &look);
    }
    assert!(
        (owl.head_yaw - 1.0).abs() < 0.05,
        "head reaches yaw target: {}",
        owl.head_yaw
    );
    assert!(
        (owl.head_pitch - 0.3).abs() < 0.05,
        "head reaches pitch target: {}",
        owl.head_pitch
    );
    for _ in 0..120 {
        owl.apply_expression(1.0 / 60.0, owl_def(), &[], &BehaviorOutput::default());
    }
    assert!(
        owl.head_yaw.abs() < 0.05,
        "head recentres in yaw: {}",
        owl.head_yaw
    );
    assert!(
        owl.head_pitch.abs() < 0.05,
        "head recentres in pitch: {}",
        owl.head_pitch
    );
}

#[test]
fn named_anim_set_is_sorted_capped_and_idempotent() {
    let names = |owl: &Instance| -> Vec<String> {
        owl.active_anims().iter().map(|l| l.name.clone()).collect()
    };
    let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
    assert!(owl.set_anim_active("row_right", true));
    assert!(owl.set_anim_active("row_left", true));
    assert!(
        owl.set_anim_active("row_left", true),
        "re-activate is a no-op"
    );
    assert_eq!(names(&owl), ["row_left", "row_right"]);
    assert!(owl.set_anim_active("c", true));
    assert!(owl.set_anim_active("d", true));
    assert!(
        !owl.set_anim_active("e", true),
        "a fifth activation refuses (cap {})",
        crate::mob::MAX_ACTIVE_MOB_ANIMS
    );
    assert!(
        owl.set_anim_active("missing", false),
        "deactivate absent = ok"
    );
    assert!(owl.set_anim_active("row_left", false));
    assert_eq!(names(&owl), ["c", "d", "row_right"]);
}

#[test]
fn named_anim_layers_self_clock_by_their_rates() {
    // Each layer's phase advances by ITS OWN rate — rate 0 freezes a
    // layer mid-stroke (an oar pauses in place, never snaps home),
    // negative reverses — independent of the walk/idle clock.
    let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
    assert!(owl.set_anim_active("a", true));
    assert!(owl.set_anim_active("b", true));
    assert!(
        !owl.set_anim_rate("missing", 2.0),
        "a rate on an inactive anim refuses"
    );
    let step = |owl: &mut Instance| {
        owl.apply_expression(0.5, def(Mob::Owl), &[], &Default::default());
    };
    step(&mut owl); // both at default rate 1
    let phase = |owl: &Instance, n: &str| {
        owl.active_anims()
            .iter()
            .find(|l| l.name == n)
            .unwrap()
            .phase
    };
    assert_eq!((phase(&owl, "a"), phase(&owl, "b")), (0.5, 0.5));
    assert!(owl.set_anim_rate("a", 0.0));
    assert!(owl.set_anim_rate("b", -1.0));
    step(&mut owl);
    assert_eq!(phase(&owl, "a"), 0.5, "rate 0 freezes the layer in place");
    assert_eq!(phase(&owl, "b"), 0.0, "negative rate plays in reverse");
}

#[test]
fn named_anim_seek_lands_exactly_holds_and_yields_to_rate() {
    // A seek approaches its target DIRECTLY at |rate|/s, lands EXACTLY on
    // it (no overshoot, then holds at rate 0) — the settle-to-pose
    // contract an oar's gentle return depends on. A rate call cancels it.
    let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
    assert!(owl.set_anim_active("a", true));
    assert!(
        !owl.set_anim_seek("missing", 0.0, 1.0),
        "a seek on an inactive anim refuses"
    );
    let step = |owl: &mut Instance| {
        owl.apply_expression(0.5, def(Mob::Owl), &[], &Default::default());
    };
    let phase = |owl: &Instance| owl.active_anims()[0].phase;
    step(&mut owl); // free-runs to 0.5 at the default rate 1
    assert!(owl.set_anim_seek("a", 1.7, 1.0));
    step(&mut owl);
    assert_eq!(phase(&owl), 1.0, "seeking toward the target at |rate|");
    step(&mut owl);
    step(&mut owl);
    assert_eq!(
        phase(&owl),
        1.7,
        "lands EXACTLY on the target, no overshoot"
    );
    step(&mut owl);
    assert_eq!(phase(&owl), 1.7, "then holds (rate 0)");
    assert!(owl.set_anim_seek("a", 0.7, -1.0), "rate sign is ignored");
    step(&mut owl);
    assert_eq!(phase(&owl), 1.2, "seeks BACKWARD toward a lower target");
    assert!(owl.set_anim_rate("a", 1.0));
    step(&mut owl);
    assert_eq!(phase(&owl), 1.7, "a rate call cancels the seek");
}

#[test]
fn named_anim_controls_are_bounded_and_phase_stepping_stays_finite() {
    let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
    assert!(!owl.set_anim_active(&"a".repeat(mod_api::MAX_MOB_ANIM_NAME_BYTES + 1), true));
    assert!(owl.set_anim_active("a", true));
    assert!(!owl.set_anim_rate("a", mod_api::MAX_MOB_ANIM_RATE_MAGNITUDE * 2.0));
    assert!(!owl.set_anim_seek("a", mod_api::MAX_MOB_ANIM_PHASE_MAGNITUDE * 2.0, 1.0));

    let layer = &mut owl.active_anims[0];
    layer.phase = f32::INFINITY;
    layer.rate = 1.0;
    layer.seek = Some(2.0);
    step_anim_layer(layer, 0.05);
    assert!(layer.phase.is_finite());
    assert_eq!(layer.rate, 0.0);
    assert_eq!(layer.seek, None);

    layer.phase = mod_api::MAX_MOB_ANIM_PHASE_MAGNITUDE;
    layer.rate = mod_api::MAX_MOB_ANIM_RATE_MAGNITUDE;
    step_anim_layer(layer, 0.05);
    assert!(layer.phase.is_finite());
    assert!(layer.phase.abs() <= mod_api::MAX_MOB_ANIM_PHASE_MAGNITUDE);
    assert_eq!(layer.rate, 0.0, "a step past the phase envelope parks");
}

/// A mod-activated ONE-SHOT layer retires itself once it has played
/// through (fire-and-forget activation — the sheep's `eat` bite ends on
/// its own); looping clips, deliberate rate-0 holds, and names the model
/// doesn't carry all stay until deactivated.
#[test]
fn a_finished_one_shot_layer_retires_itself() {
    use crate::mob::model_meta::NamedAnimMeta;
    let named = [
        NamedAnimMeta {
            name: "bite".into(),
            length: 0.2,
            looping: false,
        },
        NamedAnimMeta {
            name: "hum".into(),
            length: 0.2,
            looping: true,
        },
    ];
    let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
    assert!(owl.set_anim_active("bite", true));
    assert!(owl.set_anim_active("hum", true));
    assert!(owl.set_anim_active("mystery", true));
    for _ in 0..30 {
        owl.apply_expression(1.0 / 60.0, owl_def(), &named, &BehaviorOutput::default());
    }
    let names: Vec<&str> = owl.active_anims().iter().map(|l| l.name.as_str()).collect();
    assert!(
        !names.contains(&"bite"),
        "played-through one-shot retired: {names:?}"
    );
    assert!(
        names.contains(&"hum"),
        "a looping layer plays until deactivated"
    );
    assert!(
        names.contains(&"mystery"),
        "an unknown name is the mod's business"
    );

    // A rate-0 hold is a deliberate pose — it never expires, and resuming
    // playback lets it finish and retire.
    assert!(owl.set_anim_active("bite", true));
    assert!(owl.set_anim_rate("bite", 0.0));
    for _ in 0..30 {
        owl.apply_expression(1.0 / 60.0, owl_def(), &named, &BehaviorOutput::default());
    }
    assert!(owl.anim_state("bite").is_some(), "a frozen one-shot holds");
    assert!(owl.set_anim_rate("bite", 1.0));
    for _ in 0..30 {
        owl.apply_expression(1.0 / 60.0, owl_def(), &named, &BehaviorOutput::default());
    }
    assert!(
        owl.anim_state("bite").is_none(),
        "resumed playback finishes and retires"
    );
}

#[test]
fn the_head_gathers_speed_settles_without_overshoot_and_lands_on_its_target() {
    let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
    let look = BehaviorOutput {
        head_look: Some(crate::mob::brain::HeadLook {
            yaw: 1.2,
            pitch: -0.8,
        }),
        ..Default::default()
    };
    let mut steps = Vec::new();
    for _ in 0..40 {
        let before = owl.head_yaw;
        owl.apply_expression(1.0 / 20.0, owl_def(), &[], &look);
        assert!(owl.head_yaw <= 1.2 + 1e-6, "never past the target");
        steps.push(owl.head_yaw - before);
    }
    assert!(steps[1] > steps[0], "it starts gently: {steps:?}");
    let peak = steps.iter().copied().fold(0.0, f32::max);
    assert!(steps[6] < peak, "and slows into the target: {steps:?}");
    // An action judged along the gaze needs the head to ARRIVE, not approach.
    assert_eq!(owl.head_yaw, 1.2);
    assert_eq!(owl.head_pitch, -0.8);
}
