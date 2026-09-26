//! The headless dedicated-server host: the SAME server the in-game "Open to
//! LAN" runs — `game/session.rs::build_headless_session` builds it with no
//! local session, [`crate::server::handle::spawn`] runs the identical self-clocked
//! loop on its own thread, and this module's main thread just opens the
//! listener and parks on its console (`stop`, `save`, `say`, `op`, `deop`,
//! and `time`).
//!
//! One server codebase, two hosts: everything gameplay-visible (tick ladder,
//! streaming, flow control, joins/leaves, saves) is shared with the listen
//! server; the only headless-specific behavior lives behind
//! `SessionRegistry::has_local_session` (no local pipe recipient, every session
//! ack-windowed, fixed ticks skipped while nobody is connected).

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Headless-server settings: `settings.json` NEXT TO THE SERVER BINARY (not
/// in the data dir — one config per deployed binary). Materialized with
/// defaults on first run so the knobs are discoverable; unknown fields are
/// ignored so hand-edited files survive version drift. `PETRAMOND_*` env vars
/// override the file for one-off runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    /// The server's streaming radius CEILING in chunks (`4..=64`): what the
    /// world keeps loaded around players. A client requesting less (its own
    /// view-distance option) is streamed less; requesting more clamps here.
    pub view_distance: i32,
    /// How far from the nearest player mobs simulate, in chunks — fully,
    /// then at a reduced AI rate, then frozen (see `mob::SimDistance`).
    /// Independent of `view_distance`: loading more of the world no longer
    /// costs mob simulation.
    pub simulation_distance: crate::mob::SimDistance,
    /// Most players connected at once (`1..=256`); joins beyond it are
    /// refused with `ServerFull`.
    pub max_players: usize,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            view_distance: 32,
            simulation_distance: crate::mob::SimDistance::default(),
            max_players: 20,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("settings.json"))
}

fn load_settings() -> ServerSettings {
    let Some(path) = settings_path() else {
        return ServerSettings::default();
    };
    if !path.exists() {
        let defaults = ServerSettings::default();
        match serde_json::to_vec_pretty(&defaults) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(&path, bytes) {
                    log::warn!("could not materialize {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("could not encode default server settings: {e}"),
        }
        return defaults;
    }
    match std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice::<ServerSettings>(&b).map_err(|e| e.to_string()))
    {
        Ok(s) => s,
        Err(e) => {
            log::warn!(
                "server settings {} are unreadable ({e}); using defaults",
                path.display()
            );
            ServerSettings::default()
        }
    }
}

/// Load the process's content before anything touches it: every installed
/// pack, then — this process serves exactly one world and draws nothing — the
/// registry scoped to that world's enabled mods, so a pack the world switched
/// off registers no ids at all. Either failure is the full load report.
fn install_world_content(
    world_name: &str,
) -> Result<(), petramond_world::content::ContentErrors> {
    crate::content::install_from_env(&[])?;
    let dir = crate::save::world_dir(world_name);
    let disabled = crate::save::settings::load(&dir).disabled_mods;
    let scoped = crate::content::for_world(&disabled)?;
    petramond_world::content::install(scoped);
    Ok(())
}

/// `petramond_server <world-name>` — configured by `settings.json` beside the
/// binary (`view_distance`, `simulation_distance`); env overrides: `PETRAMOND_SEED` (new worlds
/// only), `PETRAMOND_RD` (streaming radius > settings.json), `PETRAMOND_PORT`
/// (default 7434, 0 = ephemeral).
pub fn run() {
    super::init_logging();
    let Some(world_name) = std::env::args().nth(1) else {
        eprintln!("usage: petramond_server <world-name>");
        eprintln!(
            "  settings.json (beside the binary): view_distance <4..64>, \
             simulation_distance {{full_chunks, reduced_chunks, reduced_interval}}, \
             max_players <1..256>"
        );
        eprintln!("  env: PETRAMOND_SEED=<u32>  PETRAMOND_RD=<4..64>  PETRAMOND_PORT=<port>");
        std::process::exit(2);
    };
    if let Err(e) = install_world_content(&world_name) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    let settings = load_settings();
    let seed: u32 = std::env::var("PETRAMOND_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x1234_5678);
    let rd: i32 = std::env::var("PETRAMOND_RD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(settings.view_distance)
        .clamp(4, 64);
    let port: u16 = std::env::var("PETRAMOND_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(crate::net::DEFAULT_PORT);

    let mut server = crate::server::session_build::build_headless_session(&world_name, seed, rd);
    server.set_sim_distance(settings.simulation_distance);
    server.set_max_players(settings.max_players);
    let mut handle = crate::server::handle::spawn(server);
    let port = match handle.open_to_lan(port) {
        Ok(port) => port,
        Err(e) => {
            log::error!("could not bind port {port}: {e}");
            std::process::exit(1);
        }
    };
    log::info!("serving world '{world_name}' on port {port} — type 'stop' to save and exit");

    // Console commands arrive over a channel so the main loop can also keep
    // the handle's (unused) local outbox drained and watch for crashes.
    // Stdin EOF (running under a supervisor with no console) just stops the
    // reader; the server keeps running until a signal kills the process —
    // autosave bounds the loss, but `stop` is the clean path.
    let (line_tx, line_rx) = mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("petramond-console".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                match stdin.read_line(&mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        if line_tx.send(line.trim().to_string()).is_err() {
                            return;
                        }
                    }
                }
            }
        })
        .expect("spawn console thread");

    let mut discard = Vec::new();
    let mut console_open = true;
    loop {
        let command = if console_open {
            match line_rx.recv_timeout(Duration::from_millis(250)) {
                Ok(cmd) => Some(cmd),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => {
                    console_open = false;
                    None
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(250));
            None
        };
        match command.as_deref() {
            Some("stop") => break,
            Some("save") => {
                handle.save_all();
                log::info!("save requested");
            }
            Some("") | None => {}
            Some(other) => handle.command(other.to_owned()),
        }
        // No local session ever produces gameplay messages, but join/leave
        // broadcasts still land on the local pipe — keep it from banking.
        handle.drain(&mut discard);
        discard.clear();
        if handle.is_crashed() {
            log::error!("server thread crashed; exiting (the world was NOT saved — autosave bounds the loss)");
            std::process::exit(1);
        }
    }
    log::info!("stopping: saving world and disconnecting players");
    handle.shutdown_and_join();
    log::info!("server stopped");
}
