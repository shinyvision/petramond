//! The headless dedicated-server host: the SAME server the in-game "Open to
//! LAN" runs — `game/session.rs::build_headless_session` builds it with no
//! local session, [`ServerHandle::spawn`] runs the identical self-clocked
//! loop on its own thread, and this module's main thread just opens the
//! listener and parks on its console (`stop`, `save`, `say`, `op`, `deop`,
//! and `time`).
//!
//! One server codebase, two hosts: everything gameplay-visible (tick ladder,
//! streaming, flow control, joins/leaves, saves) is shared with the listen
//! server; the only headless-specific behavior lives behind
//! `ServerGame::has_local_session` (no local pipe recipient, every session
//! ack-windowed, fixed ticks skipped while nobody is connected).

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::server::handle::ServerHandle;

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
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self { view_distance: 32 }
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

/// `petramond_server <world-name>` — configured by `settings.json` beside the
/// binary (`view_distance`); env overrides: `PETRAMOND_SEED` (new worlds
/// only), `PETRAMOND_RD` (streaming radius > settings.json), `PETRAMOND_PORT`
/// (default 7434, 0 = ephemeral).
pub fn run() {
    super::init_logging();
    let Some(world_name) = std::env::args().nth(1) else {
        eprintln!("usage: petramond_server <world-name>");
        eprintln!("  settings.json (beside the binary): view_distance <4..64>");
        eprintln!("  env: PETRAMOND_SEED=<u32>  PETRAMOND_RD=<4..64>  PETRAMOND_PORT=<port>");
        std::process::exit(2);
    };
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

    let server = crate::game::session::build_headless_session(&world_name, seed, rd);
    let mut handle = ServerHandle::spawn(server);
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
