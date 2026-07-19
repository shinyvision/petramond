//! World-join latency harness (manual, `#[ignore]`d): measures the wall
//! clock from `Game::new` (the shell's "play" click) to a playable spawn —
//! the player's 3×3 column neighbourhood meshed in the replica.
//!
//! Run against the REAL topology (server thread + loopback):
//!
//! ```text
//! PETRAMOND_DATA_DIR=<scratch> PETRAMOND_MODS=$PWD/mods \
//! PETRAMOND_JOIN_FPS=60 \
//! RUST_LOG=petramond::join::perf=debug,petramond::modding::perf=debug \
//! cargo test --profile playtest --lib game::tests::joinprofile::join_profile \
//!   -- --exact --ignored --nocapture
//! ```
//!
//! First invocation creates the world (fresh-world numbers); rerun for the
//! existing-world cold-process numbers. `PETRAMOND_JOIN_RD` overrides the
//! render distance (default 32), `PETRAMOND_JOIN_FPS` the client frame
//! cadence (default 240), `PETRAMOND_JOIN_WORLD` the save-directory name
//! (point it at a COPY of a real save for big-save numbers). Runs vary ±30%:
//! compare minima of several runs.

use std::time::{Duration, Instant};

use crate::camera::Camera;
use crate::chunk::ChunkPos;
use crate::game::{Game, GameInput};
use crate::mathh::Vec3;
use crate::net::protocol::ClientToServer;

#[test]
#[ignore]
fn join_profile() {
    let _ = env_logger::builder().is_test(false).try_init();
    assert!(
        std::env::var("PETRAMOND_DATA_DIR").is_ok(),
        "set PETRAMOND_DATA_DIR to a scratch dir (this test writes a real save)"
    );
    let rd: i32 = std::env::var("PETRAMOND_JOIN_RD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(32);
    let fps: f64 = std::env::var("PETRAMOND_JOIN_FPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(240.0);

    let world = std::env::var("PETRAMOND_JOIN_WORLD").unwrap_or_else(|_| "joinprofile".into());
    let t_click = Instant::now();
    let cam = Camera::new(Vec3::new(8.0, 90.0, 8.0), 16.0 / 9.0);
    let mut game = Game::new(cam, &world, 0x312, rd);
    let t_new = t_click.elapsed();

    // Pump production frames at 240 fps cadence until the spawn
    // neighbourhood is meshed (or we give up).
    let input = GameInput::default();
    let dt = (1.0 / fps) as f32;
    let mut frames = 0u64;
    let mut t_first_mesh = None;
    let t_playable = loop {
        game.tick(dt, &input);
        frames += 1;
        let feet = game.player.pos;
        let pc = ChunkPos::new(
            (feet.x.floor() as i32).div_euclid(16),
            (feet.z.floor() as i32).div_euclid(16),
        );
        let installed = game.replica.loaded_section_count();
        let handoff = game.terrain_render_handoff();
        if t_first_mesh.is_none() && handoff.has_column_mesh(pc) {
            t_first_mesh = Some(t_click.elapsed());
        }
        let own_meshed = handoff.has_column_mesh(pc);
        let neighbourhood_meshed = (-1..=1).all(|dz| {
            (-1..=1).all(|dx| handoff.has_column_mesh(ChunkPos::new(pc.cx + dx, pc.cz + dz)))
        });
        println!(
            "  {:7.1} ms frame {frames:3}: {installed:5} sections installed, own mesh {}, 3x3 {}",
            t_click.elapsed().as_secs_f64() * 1e3,
            own_meshed as u8,
            neighbourhood_meshed as u8,
        );
        if neighbourhood_meshed {
            break t_click.elapsed();
        }
        if t_click.elapsed() > Duration::from_secs(60) {
            panic!("spawn neighbourhood never meshed within 60 s");
        }
        std::thread::sleep(Duration::from_secs_f64(dt as f64));
    };

    println!("== join profile (rd {rd}, {fps:.0} fps) ==");
    print_summary(rd, t_new, t_first_mesh, t_playable, frames);
}

/// The PACING-FREE floor: the same pipeline pumped synchronously (loopback,
/// no server thread, no frame sleeps) as fast as results land. The gap
/// between this and [`join_profile`] is pure scheduling/pacing latency.
#[test]
#[ignore]
fn join_profile_sync() {
    let _ = env_logger::builder().is_test(false).try_init();
    assert!(
        std::env::var("PETRAMOND_DATA_DIR").is_ok(),
        "set PETRAMOND_DATA_DIR to a scratch dir (this test writes a real save)"
    );
    let rd: i32 = std::env::var("PETRAMOND_JOIN_RD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(32);

    let t_click = Instant::now();
    let (mut server, bootstrap) = crate::game::session::build_session("joinprofile", 0x312, rd);
    let (handle, pipe) = crate::server::handle::ServerHandle::loopback();
    let mut game = Game::assemble(
        Camera::new(Vec3::new(8.0, 90.0, 8.0), 16.0 / 9.0),
        handle,
        bootstrap,
    );
    let t_new = t_click.elapsed();

    let input = GameInput::default();
    let mut frames = 0u64;
    let mut t_first_mesh = None;
    // Stage marks, all relative to the click.
    let mut t_first_server_install = None;
    let mut t_server_spawn_light_final = None;
    let mut t_first_client_install = None;
    let mut last = Instant::now();
    let t_playable = loop {
        // Real elapsed dt: a fixed dt at spin speed would run fixed ticks at
        // many times real time and burn the main thread we are measuring.
        let now = Instant::now();
        let dt = (now - last).as_secs_f32();
        last = now;
        game.tick_send(dt, &input);
        let mut inbox: Vec<ClientToServer> = Vec::new();
        while let Ok(msg) = pipe.inbox.try_recv() {
            inbox.push(msg);
        }
        let out = server.pump(dt, &mut inbox);
        for msg in out.msgs {
            let _ = pipe.outbox.send(msg);
        }
        game.tick_receive(dt);
        frames += 1;
        let feet = game.player.pos;
        let pc = ChunkPos::new(
            (feet.x.floor() as i32).div_euclid(16),
            (feet.z.floor() as i32).div_euclid(16),
        );
        if t_first_server_install.is_none() && server.world.loaded_section_count() > 0 {
            t_first_server_install = Some(t_click.elapsed());
        }
        if t_server_spawn_light_final.is_none() {
            let feet_cy = (feet.y.floor() as i32).div_euclid(16);
            let all_final = (-1..=1).all(|dz| {
                (-1..=1).all(|dx| {
                    (-1..=1).all(|dy| {
                        let sp =
                            crate::chunk::SectionPos::new(pc.cx + dx, feet_cy + dy, pc.cz + dz);
                        let loaded = server
                            .world
                            .section_at_world_for_test(sp.cx * 16, sp.cy * 16, sp.cz * 16)
                            .is_some();
                        !loaded || server.world.section_light_final(sp)
                    })
                })
            });
            let any_loaded = server.world.loaded_section_count() > 0;
            if any_loaded && all_final {
                t_server_spawn_light_final = Some(t_click.elapsed());
            }
        }
        if t_first_client_install.is_none() && game.replica.loaded_section_count() > 0 {
            t_first_client_install = Some(t_click.elapsed());
        }
        let handoff = game.terrain_render_handoff();
        if t_first_mesh.is_none() && handoff.has_column_mesh(pc) {
            t_first_mesh = Some(t_click.elapsed());
        }
        let neighbourhood_meshed = (-1..=1).all(|dz| {
            (-1..=1).all(|dx| handoff.has_column_mesh(ChunkPos::new(pc.cx + dx, pc.cz + dz)))
        });
        if neighbourhood_meshed {
            break t_click.elapsed();
        }
        if t_click.elapsed() > Duration::from_secs(60) {
            panic!("spawn neighbourhood never meshed within 60 s");
        }
        std::thread::yield_now();
    };
    println!("== join profile SYNC floor (rd {rd}) ==");
    let ms = |t: Option<Duration>| t.map_or(-1.0, |t| t.as_secs_f64() * 1e3);
    println!(
        "first server section install:    {:.1} ms",
        ms(t_first_server_install)
    );
    println!(
        "server spawn 3x3x3 light-final:  {:.1} ms",
        ms(t_server_spawn_light_final)
    );
    println!(
        "first client section install:    {:.1} ms",
        ms(t_first_client_install)
    );
    print_summary(rd, t_new, t_first_mesh, t_playable, frames);
}

fn print_summary(
    _rd: i32,
    t_new: Duration,
    t_first_mesh: Option<Duration>,
    t_playable: Duration,
    frames: u64,
) {
    println!(
        "Game::new (click -> constructed): {:.1} ms",
        t_new.as_secs_f64() * 1e3
    );
    if let Some(t) = t_first_mesh {
        println!(
            "click -> own column meshed:      {:.1} ms",
            t.as_secs_f64() * 1e3
        );
    }
    println!(
        "click -> spawn playable (3x3):   {:.1} ms ({frames} frames)",
        t_playable.as_secs_f64() * 1e3
    );
}
