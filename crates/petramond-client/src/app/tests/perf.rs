use super::*;
use std::time::Instant;

const BASE_REGION_PREFIX: &str = "minimap:r:";
const MIP_REGION_PREFIX: &str = "minimap:m:";
const RAW_VERSION: u8 = 0;

fn synthetic_region_value(rx: i32, rz: i32, blocks_per_cell: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 16 * (1 + 256 * 5));
    out.push(RAW_VERSION);
    for sub in 0..16i32 {
        out.push(1);
        let (tx, tz) = (rx * 4 + sub % 4, rz * 4 + sub / 4);
        for i in 0..256i32 {
            let wx = (tx * 16 + i % 16) * blocks_per_cell;
            let wz = (tz * 16 + i / 16) * blocks_per_cell;
            let height = ((wx * 3 + wz * 5).rem_euclid(60)) as i16;
            out.extend(height.to_le_bytes());
            out.extend([(wx.rem_euclid(251)) as u8, (wz.rem_euclid(241)) as u8, 200]);
        }
    }
    out
}

fn profile(label: &str, frames: &[f64]) {
    let mut sorted = frames.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
    let over_2ms = frames.iter().filter(|ms| **ms > 2.0).count();
    println!(
        "{label}: {} frames, p50 {:.2}ms, p95 {:.2}ms, max {:.2}ms, >2ms: {}",
        frames.len(),
        p(0.50),
        p(0.95),
        p(1.0),
        over_2ms,
    );
}

#[test]
#[ignore = "manual audit harness (wall-clock paced): run alone with --ignored --nocapture"]
fn resampled_sessions_should_rewrite_no_unchanged_tiles() {
    ensure_test_data_dir();
    let storage_dir = petramond::modding::client::client_storage_dir_for_test(
        &petramond::modding::client::local_session_key(""),
        "minimap",
    );
    let snapshot = |label: &str| -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut out = std::collections::BTreeMap::new();
        if let Ok(dir) = std::fs::read_dir(&storage_dir) {
            for entry in dir.flatten() {
                out.insert(
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read(entry.path()).unwrap_or_default(),
                );
            }
        }
        println!("{label}: {} stored keys", out.len());
        out
    };

    let home = WorldPos::new(100.5, 90.0, 100.5);
    let session = || {
        let mut app = app_with_render_dist(4);
        app.app.game_mut().place_player_for_test(home);
        app.set_server_player_pos(home);
        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut frames = 0u32;
        while frames < 900 {
            app.frame_and_pump_recorded((1280, 720), &mut kinds);
            std::thread::sleep(std::time::Duration::from_millis(1));
            frames += 1;
            if std::time::Instant::now() >= deadline {
                break;
            }
        }
        let replica = app.app.game().replica_for_test();
        let loaded = (100i32.div_euclid(16) - 8..=100i32.div_euclid(16) + 8)
            .flat_map(|cz| {
                (100i32.div_euclid(16) - 8..=100i32.div_euclid(16) + 8).map(move |cx| (cx, cz))
            })
            .filter(|&(cx, cz)| {
                replica
                    .client_surface_column_revision(petramond_world::chunk::ChunkPos::new(cx, cz))
                    .is_some()
            })
            .count();
        println!("session ran {frames} frames; loaded columns near home: {loaded}");
    };

    session();
    let first = snapshot("after session 1");
    assert!(!first.is_empty(), "session 1 persisted explored tiles");
    session();
    let second = snapshot("after session 2");

    let overlap = first.keys().filter(|key| second.contains_key(*key)).count();
    println!("overlapping keys: {overlap} of {}", first.len());
    assert!(overlap >= 9, "the sessions must resample shared ground");

    let rewritten = first
        .iter()
        .filter(|(key, old)| second.get(*key).is_some_and(|new| new != *old))
        .count();
    println!("rewritten values: {rewritten} / {}", first.len());
    assert_eq!(
        rewritten, 0,
        "a resample of unchanged terrain must rewrite nothing"
    );
}

fn present_full_tiles(app: &TestApp) -> usize {
    app.app
        .game()
        .client_mod_canvas_view("minimap:full_map")
        .map(|view| {
            view.elements
                .iter()
                .filter(|element| match &element.element {
                    mod_api::ClientCanvasElement::Image { image_key, .. } => {
                        image_key.starts_with("minimap:full_tile_")
                    }
                    _ => false,
                })
                .count()
        })
        .unwrap_or(0)
}

fn expected_full_tiles(pan: [f64; 2], zoom: i8) -> usize {
    let (cell_blocks, cell_px) = match zoom {
        i8::MIN..=-2 => (2, 1),
        -1 => (1, 1),
        0 => (1, 2),
        1 => (1, 4),
        _ => (1, 8),
    };
    let bpp = cell_blocks as f64 / cell_px as f64;
    let tile_blocks = (160 / cell_px * cell_blocks) as f64;
    let axis = |center: f64| {
        let half_blocks = 800.0 * bpp * 0.5;
        let min = ((center - half_blocks) / tile_blocks).floor() as i64;
        let max = ((center + half_blocks) / tile_blocks).ceil() as i64 - 1;
        (max - min + 1) as usize
    };
    axis(pan[0]) * axis(pan[1])
}

#[test]
#[ignore = "manual perf harness: run alone with --ignored --nocapture"]
fn world_map_drag_fill_latency() {
    let _ = env_logger::builder().is_test(true).try_init();
    let mut app = app();
    let eye = app.app.game().listener_position();
    let (pcx, pcz) = (
        (eye.x.floor() as i32).div_euclid(16),
        (eye.z.floor() as i32).div_euclid(16),
    );

    let (prx, prz) = ((pcx * 16).div_euclid(64), (pcz * 16).div_euclid(64));
    let (pmx, pmz) = ((pcx * 16).div_euclid(128), (pcz * 16).div_euclid(128));
    let mut entries = Vec::new();
    for rz in (prz - 16)..=(prz + 16) {
        for rx in (prx - 16)..=(prx + 110) {
            entries.push((
                format!("{BASE_REGION_PREFIX}{rx}:{rz}"),
                synthetic_region_value(rx, rz, 1),
            ));
        }
    }
    for mz in (pmz - 9)..=(pmz + 9) {
        for mx in (pmx - 9)..=(pmx + 56) {
            entries.push((
                format!("{MIP_REGION_PREFIX}{mx}:{mz}"),
                synthetic_region_value(mx, mz, 2),
            ));
        }
    }
    let seeded = entries.len();
    let seed_started = Instant::now();
    petramond::modding::client::seed_client_storage_for_test(
        &petramond::modding::client::local_session_key(""),
        "minimap",
        entries,
    );
    println!("seeded {seeded} regions in {:.0?}", seed_started.elapsed());

    let screen = (1600u32, 1000u32);
    let frame = |app: &mut TestApp| {
        app.app.update_frame(screen);
        std::thread::sleep(std::time::Duration::from_millis(4));
    };

    for _ in 0..10 {
        frame(&mut app);
    }
    use petramond_input::keycode::KeyCode;
    assert!(app.app.handle_raw_key(KeyCode::KeyM, true));
    let _ = app.app.handle_raw_key(KeyCode::KeyM, false);
    frame(&mut app);
    assert!(app.app.screen.client_canvas_open(), "map open");
    app.app.compose_client_overlays(screen);

    let zoom0 = 0i8;
    let mut pan = [(eye.x / 0.5).round() * 0.5, (eye.z / 0.5).round() * 0.5];
    let fill = |app: &mut TestApp, pan: [f64; 2], zoom: i8, label: &str| {
        let expected = expected_full_tiles(pan, zoom);
        let started = Instant::now();
        let mut frames = 0u32;
        while present_full_tiles(app) < expected {
            frame(app);
            frames += 1;
            assert!(
                frames < 4000,
                "{label}: still {}/{} tiles after {frames} frames",
                present_full_tiles(app),
                expected
            );
        }
        println!(
            "{label}: {expected} tiles full after {frames} frames ({:.0?})",
            started.elapsed()
        );
    };
    fill(&mut app, pan, zoom0, "open @ zoom 0");

    app.app.set_cursor_position(800.0, 500.0);
    for _ in 0..2 {
        app.app.add_scroll_delta(1.0);
        frame(&mut app);
    }
    let zoom = -2i8;
    pan = [(pan[0] / 2.0).round() * 2.0, (pan[1] / 2.0).round() * 2.0];
    fill(&mut app, pan, zoom, "zoom 0 → −2");

    let bpp = 2.0f64;
    let mut deficit_frames = 0u32;
    let mut deficit_sum = 0u64;
    let mut deficit_max = 0usize;
    let mut drag_frames = 0u32;
    let drag_started = Instant::now();
    for _stroke in 0..4 {
        let start = [1150.0f32, 500.0f32];
        let pan0 = pan;
        app.app.set_cursor_position(start[0], start[1]);
        app.app
            .set_pointer_button(petramond_world::gui_state::PointerButton::Primary, true);
        for step in 1..=24 {
            let x = start[0] - step as f32 * 30.0;
            app.app.set_cursor_position(x, start[1]);
            pan = [pan0[0] - f64::from((x - start[0]).round()) * bpp, pan0[1]];
            frame(&mut app);
            drag_frames += 1;
            let expected = expected_full_tiles(pan, zoom);
            let present = present_full_tiles(&app);
            if present < expected {
                deficit_frames += 1;
                deficit_sum += (expected - present) as u64;
                deficit_max = deficit_max.max(expected - present);
            }
        }
        app.app
            .set_pointer_button(petramond_world::gui_state::PointerButton::Primary, false);
        frame(&mut app);
        drag_frames += 1;
    }
    println!(
        "drag @ −2: {drag_frames} frames ({:.0?}), {deficit_frames} frames with blank tiles, \
         mean blank {:.1}, worst blank {deficit_max}",
        drag_started.elapsed(),
        deficit_sum as f64 / drag_frames as f64,
    );
    fill(&mut app, pan, zoom, "settle after drag");
}

#[test]
#[ignore = "manual perf harness: run alone with --ignored --nocapture"]
fn world_map_zoom_out_frame_profile() {
    let _ = env_logger::builder().is_test(true).try_init();
    let mut app = app();
    let eye = app.app.game().listener_position();
    let (pcx, pcz) = (
        (eye.x.floor() as i32).div_euclid(16),
        (eye.z.floor() as i32).div_euclid(16),
    );

    let (prx, prz) = ((pcx * 16).div_euclid(64), (pcz * 16).div_euclid(64));
    let (pmx, pmz) = ((pcx * 16).div_euclid(128), (pcz * 16).div_euclid(128));
    let mut entries = Vec::new();
    for rz in (prz - 16)..=(prz + 16) {
        for rx in (prx - 16)..=(prx + 16) {
            entries.push((
                format!("{BASE_REGION_PREFIX}{rx}:{rz}"),
                synthetic_region_value(rx, rz, 1),
            ));
        }
    }
    for mz in (pmz - 9)..=(pmz + 9) {
        for mx in (pmx - 9)..=(pmx + 9) {
            entries.push((
                format!("{MIP_REGION_PREFIX}{mx}:{mz}"),
                synthetic_region_value(mx, mz, 2),
            ));
        }
    }
    let seeded = entries.len();
    let seed_started = Instant::now();
    petramond::modding::client::seed_client_storage_for_test(
        &petramond::modding::client::local_session_key(""),
        "minimap",
        entries,
    );
    println!("seeded {seeded} tiles in {:.0?}", seed_started.elapsed());

    let screen = (1280u32, 720u32);
    let frame = |app: &mut TestApp| {
        let started = Instant::now();
        app.app.update_frame(screen);
        started.elapsed().as_secs_f64() * 1e3
    };

    for _ in 0..10 {
        frame(&mut app);
    }
    use petramond_input::keycode::KeyCode;
    assert!(app.app.handle_raw_key(KeyCode::KeyM, true));
    let _ = app.app.handle_raw_key(KeyCode::KeyM, false);
    let mut open_frames = Vec::new();
    for _ in 0..30 {
        open_frames.push(frame(&mut app));
    }
    assert!(app.app.screen.client_canvas_open(), "map open");
    profile("open @ zoom 0", &open_frames);

    app.app.compose_client_overlays(screen);
    app.app.set_cursor_position(640.0, 360.0);
    let mut zoom_frames = Vec::new();
    for _ in 0..3 {
        app.app.add_scroll_delta(1.0);
        zoom_frames.push(frame(&mut app));
    }
    for _ in 0..150 {
        zoom_frames.push(frame(&mut app));
    }
    profile("zoom to −2 + fill", &zoom_frames);

    let mut steady_frames = Vec::new();
    for _ in 0..60 {
        steady_frames.push(frame(&mut app));
    }
    profile("steady @ −2", &steady_frames);

    app.app
        .set_pointer_button(petramond_world::gui_state::PointerButton::Primary, true);
    let mut pan_frames = Vec::new();
    for step in 1..=120 {
        app.app
            .set_cursor_position(640.0 - step as f32 * 3.0, 360.0 - step as f32 * 2.0);
        pan_frames.push(frame(&mut app));
    }
    app.app
        .set_pointer_button(petramond_world::gui_state::PointerButton::Primary, false);
    profile("pan @ −2", &pan_frames);
}
