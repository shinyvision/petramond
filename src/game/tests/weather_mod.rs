//! Full-loop proof for the weather mod's replicated state: the server wasm's
//! tick publishes the `weather:*` shader params (the whole cloud field), they
//! land in the server world's environment AND the cross-mod world-KV mirror,
//! and the values stay inside the field's documented ranges. Precipitation
//! visuals and snow policy are seed/field-dependent and deliberately
//! unpinned; the derive/kill machinery has its own unit tests
//! (`game::ambient`). Pack registration needs the fixture in the registry,
//! so the assertions run in a child process (the established
//! `PETRAMOND_MODS` re-spawn pattern).

use super::super::tick::TickEvents;
use crate::camera::Camera;
use crate::mathh::Vec3;

#[test]
fn weather_mod_publishes_its_field_params_via_wasm() {
    let Some(root) = crate::modding::tests::stage_mods_fixture("weather-params", &["weather"])
    else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::weather_mod::weather_params_inner");
}

/// Runs ONLY in the child process spawned above.
#[test]
#[ignore = "spawned by weather_mod_publishes_its_field_params_via_wasm with a fixture pack env"]
fn weather_params_inner() {
    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    assert_eq!(game.mods_for_test().loaded(), 1, "the weather wasm loaded");
    super::common::flat_floor_loaded_air(&mut game.server.world, crate::block::Block::Stone);

    let mut ev = TickEvents::default();
    for _ in 0..3 {
        game.server.game_tick_step(&mut ev);
    }

    let env = game.server.world.environment().shader_params().clone();
    let wind = env
        .get("weather:wind")
        .expect("the weather tick publishes weather:wind");
    let sky = env
        .get("weather:sky")
        .expect("the weather tick publishes weather:sky");
    // [off_x, off_z, wind_x, wind_z]: offset wrapped into the field period,
    // wind within the core's speed envelope.
    assert!((0.0..weather_core_wrap()).contains(&wind[0]));
    assert!((0.0..weather_core_wrap()).contains(&wind[1]));
    let speed = (wind[2] * wind[2] + wind[3] * wind[3]).sqrt();
    assert!(
        (0.1..=8.0).contains(&speed),
        "wind speed {speed} outside the plausible envelope"
    );
    // [storm, rain_start, feature_size, seed]: the documented ranges.
    assert!((0.3..=0.8).contains(&sky[0]), "storm bias {}", sky[0]);
    assert!(sky[1] > 0.0 && sky[1] < 1.0, "rain threshold {}", sky[1]);
    assert!(sky[2] > 0.0, "feature size {}", sky[2]);
    assert!(sky[3] >= 0.0 && sky[3] == sky[3].trunc(), "seed {}", sky[3]);

    // The cross-mod KV mirror rides the same tick.
    assert!(game.server.world.mod_kv_get("weather:wind").is_some());
    assert!(game.server.world.mod_kv_get("weather:storm").is_some());

    // The offset advances with the wind: a later tick moves it.
    let before = wind[0];
    for _ in 0..40 {
        game.server.game_tick_step(&mut ev);
    }
    let after = game.server.world.environment().shader_params()["weather:wind"][0];
    assert!(
        (after - before).abs() > 1e-4,
        "the advection offset must move with the wind"
    );
}

fn weather_core_wrap() -> f32 {
    65536.0
}
