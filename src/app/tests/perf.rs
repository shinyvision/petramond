//! Manual perf harnesses (`#[ignore]`): run alone, with output, e.g.
//! `cargo test --profile playtest --lib app::tests::perf -- --ignored --nocapture`.
//! They print frame-time profiles instead of asserting thresholds — a timing
//! pin would flake under load ([[perf-timing-under-load]] rules apply: run on
//! a quiet machine and compare like with like).

use super::*;
use std::time::Instant;

/// Mirrors the minimap's region storage format in its RAW mode (see
/// `mods-src/minimap/src/codec.rs` — the decoder accepts RAW forever exactly
/// so harnesses can fabricate values); update alongside it.
const BASE_REGION_PREFIX: &str = "minimap:r:";
const MIP_REGION_PREFIX: &str = "minimap:m:";
const RAW_VERSION: u8 = 0;

/// One RAW region value: 16 sub-tiles of 256 cells, cell = (le i16 height,
/// rgb) derived from the world position by a deterministic pattern.
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
            out.extend([
                (wx.rem_euclid(251)) as u8,
                (wz.rem_euclid(241)) as u8,
                200,
            ]);
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

/// Surface-sample stability audit: sample the same world across two sessions
/// on shared client storage and report which persisted tiles get REWRITTEN
/// with different bytes on the resample — stable host-sampled colors rewrite
/// nothing. VERIFIED 2026-07-15: 49/49 overlapping tiles, zero rewrites —
/// the replica's tint halo arrives atomically with each column payload and
/// stream-finality gates the rest, so host colors are byte-stable across
/// sessions. Playtest-observed tile churn is therefore live-world evolution
/// (random ticks), not sampling instability. Prints per-cell analysis of any
/// diffs (edge clustering, height vs color) should this ever regress.
#[test]
#[ignore = "manual audit harness (wall-clock paced): run alone with --ignored --nocapture"]
fn resampled_sessions_should_rewrite_no_unchanged_tiles() {
    ensure_test_data_dir();
    let storage_dir = crate::modding::client::client_storage_dir_for_test(
        &crate::modding::client::local_session_key(""),
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

    // Both sessions sample from the same pinned spot, so their explored sets
    // overlap fully and a byte diff is meaningful.
    let home = crate::mathh::Vec3::new(100.5, 90.0, 100.5);
    let session = || {
        let mut app = app_with_render_dist(4);
        app.app
            .game
            .as_mut()
            .unwrap()
            .place_player_for_test(home);
        app.server.sessions[0].player.pos = home;
        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut frames = 0u32;
        // Settle terrain in, then keep sampling long enough for the mod's
        // flush interval to fire a few times.
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
            .flat_map(|cz| (100i32.div_euclid(16) - 8..=100i32.div_euclid(16) + 8).map(move |cx| (cx, cz)))
            .filter(|&(cx, cz)| {
                replica
                    .client_surface_column_revision(crate::chunk::ChunkPos::new(cx, cz))
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
    assert_eq!(rewritten, 0, "a resample of unchanged terrain must rewrite nothing");
}

/// Frame-time profile of the world map over a large synthetic explored world:
/// open at the default zoom, then ride the wheel to the outermost (−2) level
/// and let the progressive fill run. The "after" numbers of every world-map
/// optimization compare against this exact scenario.
#[test]
#[ignore = "manual perf harness: run alone with --ignored --nocapture"]
fn world_map_zoom_out_frame_profile() {
    // Manual harness: surface the engine's slow-dispatch diagnostics when
    // run with RUST_LOG=petramond::modding::perf=debug.
    let _ = env_logger::builder().is_test(true).try_init();
    let mut app = app();
    let eye = app.app.game().listener_position();
    let (pcx, pcz) = (
        (eye.x.floor() as i32).div_euclid(16),
        (eye.z.floor() as i32).div_euclid(16),
    );

    // Cover the −2 viewport generously with both stores: ±16 base regions
    // (= ±64 chunk tiles, ~22 MB raw) plus the mip regions over the same
    // ground — the "long-played world" case that stuttered.
    let (prx, prz) = (
        (pcx * 16).div_euclid(64),
        (pcz * 16).div_euclid(64),
    );
    let (pmx, pmz) = (
        (pcx * 16).div_euclid(128),
        (pcz * 16).div_euclid(128),
    );
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
    crate::modding::client::seed_client_storage_for_test(
        &crate::modding::client::local_session_key(""),
        "minimap",
        entries,
    );
    println!(
        "seeded {seeded} tiles in {:.0?}",
        seed_started.elapsed()
    );

    let screen = (1280u32, 720u32);
    let frame = |app: &mut TestApp| {
        let started = Instant::now();
        app.app.update_frame(screen);
        started.elapsed().as_secs_f64() * 1e3
    };

    // Warmup + open the map at the default zoom.
    for _ in 0..10 {
        frame(&mut app);
    }
    use winit::keyboard::KeyCode;
    assert!(app.app.handle_raw_key(KeyCode::KeyM, true));
    let _ = app.app.handle_raw_key(KeyCode::KeyM, false);
    let mut open_frames = Vec::new();
    for _ in 0..30 {
        open_frames.push(frame(&mut app));
    }
    assert!(app.app.screen.client_canvas_open(), "map open");
    profile("open @ zoom 0", &open_frames);

    // Wheel down to −2 with the cursor centered on the canvas.
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

    // Steady state: everything loaded and rastered.
    let mut steady_frames = Vec::new();
    for _ in 0..60 {
        steady_frames.push(frame(&mut app));
    }
    profile("steady @ −2", &steady_frames);

    // Pan a long diagonal drag: boundary crossings + band loads.
    app.app.set_pointer_button(crate::controls::PointerButton::Primary, true);
    let mut pan_frames = Vec::new();
    for step in 1..=120 {
        app.app
            .set_cursor_position(640.0 - step as f32 * 3.0, 360.0 - step as f32 * 2.0);
        pan_frames.push(frame(&mut app));
    }
    app.app
        .set_pointer_button(crate::controls::PointerButton::Primary, false);
    profile("pan @ −2", &pan_frames);
}
