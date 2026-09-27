use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    pub view_distance: i32,
    pub simulation_distance: crate::mob::SimDistance,
    pub max_players: usize,
    pub online_mode: bool,
    pub presentation_packs: bool,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            view_distance: 32,
            simulation_distance: crate::mob::SimDistance::default(),
            max_players: 20,
            online_mode: true,
            presentation_packs: true,
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

fn install_world_content(world_name: &str) -> Result<(), petramond_world::content::ContentErrors> {
    crate::content::install_from_env(&[])?;
    let dir = crate::save::world_dir(world_name);
    let disabled = crate::save::settings::load(&dir).disabled_mods;
    let scoped = crate::content::for_world(&disabled)?;
    petramond_world::content::install(scoped);
    Ok(())
}

pub fn run() {
    super::init_logging();
    let Some(world_name) = std::env::args().nth(1) else {
        eprintln!("usage: petramond_server <world-name>");
        eprintln!(
            "  settings.json (beside the binary): view_distance <4..64>, \
             simulation_distance {{full_chunks, reduced_chunks, reduced_interval}}, \
             max_players <1..256>, online_mode <bool>, presentation_packs <bool>"
        );
        eprintln!("  env: PETRAMOND_SEED=<u32>  PETRAMOND_RD=<4..64>  PETRAMOND_PORT=<port>");
        eprintln!("       PETRAMOND_ONLINE_MODE=0  PETRAMOND_ACCOUNT_URL=<origin>");
        std::process::exit(2);
    };
    let _content_lock = crate::content::ContentLock::shared(&crate::content::Dirs::installed())
        .map_err(|e| log::warn!("content lock: {e}"))
        .ok();
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
    if !settings.online_mode && std::env::var("PETRAMOND_ONLINE_MODE").is_err() {
        server.set_account_policy(crate::account::AccountPolicy::Offline);
    }
    server.set_client_policy(crate::net::protocol::ClientPolicy {
        presentation_packs: settings.presentation_packs,
    });
    if server.account_policy().requires_account() {
        log::info!(
            "online mode: players are verified against {}",
            crate::account::service_url()
        );
    } else {
        log::warn!("online mode is OFF: any client may claim any player name");
    }
    let mut handle = crate::server::handle::spawn(server);
    let port = match handle.open_to_lan(port) {
        Ok(port) => port,
        Err(e) => {
            log::error!("could not bind port {port}: {e}");
            std::process::exit(1);
        }
    };
    log::info!("serving world '{world_name}' on port {port} — type 'stop' to save and exit");

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
