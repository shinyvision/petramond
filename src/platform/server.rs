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

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use crate::server::handle::ServerHandle;

/// `petramond_server <world-name>` — env overrides: `PETRAMOND_SEED` (new
/// worlds only), `PETRAMOND_RD` (streaming radius), `PETRAMOND_PORT`
/// (default 7434, 0 = ephemeral).
pub fn run() {
    super::init_logging();
    let Some(world_name) = std::env::args().nth(1) else {
        eprintln!("usage: petramond_server <world-name>");
        eprintln!("  env: PETRAMOND_SEED=<u32>  PETRAMOND_RD=<4..64>  PETRAMOND_PORT=<port>");
        std::process::exit(2);
    };
    let seed: u32 = std::env::var("PETRAMOND_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x1234_5678);
    let rd: i32 = std::env::var("PETRAMOND_RD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(32)
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
