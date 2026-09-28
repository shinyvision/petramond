//! The Connect to Server flow: the screen's session
//! state and the ONE background thread that resolves the address, opens the
//! TCP connection, and runs the join handshake. The worker reports back over
//! an mpsc channel the ConnectServer screen drains each frame
//! ([`ConnectSession::poll`]).
//!
//! Cancellation is cooperative (a flag checked between blocking steps) plus a
//! GENERATION guard: Cancel/Back bump the session's `gen` and drop the
//! receiver, so an outcome from an abandoned attempt can never adopt a game.
//!
//! Identity is not typed here. A server that checks Petramond accounts is
//! answered with a join ticket minted on this same worker thread (the handshake
//! asks for the credential only after the server says which kind it wants); a
//! server that checks none is answered with the machine's resolved player name.
//! A refusal that means the stored sign-in is dead routes the player to the
//! Account screen instead of repeating the error.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use super::shell_docs::ShellCommand;
use super::ui_runtime::AppUi;
use super::{App, AppScreen};
use crate::game::Game;
use petramond::account::{self, AccountError};
use petramond::net::handle::ServerHandle;
use petramond::net::handshake::{
    client_handshake, installed_mod_ids, HandshakeError, HandshakeJoin, ServerOffer,
};
use petramond::net::protocol::{JoinCredential, ModEntry};
use petramond_render::camera::Camera;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

const ADDR_MAX_CHARS: usize = 64;

pub(super) enum ConnectPhase {
    Editing,
    Connecting { label: &'static str },
    Failed { message: String },
}

enum ConnectOutcome {
    Progress(&'static str),
    Joined(HandshakeJoin, ServerHandle),
    Missing(Vec<ModEntry>),
    Failed(String),
    SignInNeeded(String),
}

pub(super) struct ConnectSession {
    pub(super) phase: ConnectPhase,
    gen: u64,
    rx: Option<Receiver<(u64, ConnectOutcome)>>,
    cancel: Arc<AtomicBool>,
    pub(super) missing: Vec<ModEntry>,
    pub(super) addr: String,
}

impl Default for ConnectSession {
    fn default() -> Self {
        Self {
            phase: ConnectPhase::Editing,
            gen: 0,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            missing: Vec::new(),
            addr: String::new(),
        }
    }
}

pub(super) enum ConnectEvent {
    Joined(Box<HandshakeJoin>, ServerHandle),
    Missing,
    SignInNeeded(String),
}

impl ConnectSession {
    pub(super) fn connecting(&self) -> bool {
        matches!(self.phase, ConnectPhase::Connecting { .. })
    }

    #[cfg(test)]
    pub(super) fn has_worker(&self) -> bool {
        self.rx.is_some()
    }

    pub(super) fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.gen += 1;
        self.rx = None;
        if self.connecting() {
            self.phase = ConnectPhase::Editing;
        }
    }

    pub(super) fn poll(&mut self) -> Option<ConnectEvent> {
        loop {
            let rx = self.rx.as_ref()?;
            let (gen, outcome) = match rx.try_recv() {
                Ok(msg) => msg,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.rx = None;
                    if self.connecting() {
                        self.phase = ConnectPhase::Failed {
                            message: "The connection attempt failed".to_owned(),
                        };
                    }
                    return None;
                }
            };
            if gen != self.gen {
                continue;
            }
            match outcome {
                ConnectOutcome::Progress(label) => {
                    if self.connecting() {
                        self.phase = ConnectPhase::Connecting { label };
                    }
                }
                ConnectOutcome::Joined(join, handle) => {
                    self.rx = None;
                    self.phase = ConnectPhase::Editing;
                    return Some(ConnectEvent::Joined(Box::new(join), handle));
                }
                ConnectOutcome::Missing(mods) => {
                    self.rx = None;
                    self.phase = ConnectPhase::Editing;
                    self.missing = mods;
                    return Some(ConnectEvent::Missing);
                }
                ConnectOutcome::Failed(message) => {
                    self.rx = None;
                    self.phase = ConnectPhase::Failed { message };
                }
                ConnectOutcome::SignInNeeded(message) => {
                    self.rx = None;
                    self.phase = ConnectPhase::Editing;
                    return Some(ConnectEvent::SignInNeeded(message));
                }
            }
        }
    }

    pub(super) fn open_fresh(&mut self, ui: &mut AppUi) {
        let addr = petramond::save::client::load()
            .last_server
            .unwrap_or_default();
        *self = ConnectSession::default();
        seed_connect_fields(ui, &addr);
    }

    pub(super) fn reopen(&mut self, ui: &mut AppUi) {
        self.phase = ConnectPhase::Editing;
        let addr = self.addr.clone();
        seed_connect_fields(ui, &addr);
    }
}

pub(super) fn connect_event_command(event: ConnectEvent) -> ShellCommand {
    match event {
        ConnectEvent::Joined(join, handle) => ShellCommand::AdoptRemote(join, handle),
        ConnectEvent::Missing => ShellCommand::Goto(AppScreen::ModsMissing),
        ConnectEvent::SignInNeeded(message) => ShellCommand::OpenAccount(Some(message)),
    }
}

fn seed_connect_fields(ui: &mut AppUi, addr: &str) {
    ui.ensure_active(petramond_world::gui_state::GuiKind::ConnectServer);
    ui.state_mut()
        .set("server_addr", petramond_ui::UiValue::Str(addr.to_owned()));
    ui.focus_text_input("server_addr", addr, ADDR_MAX_CHARS);
}

impl App {
    pub(super) fn open_connect_server(&mut self) {
        self.shell.connect.open_fresh(&mut self.ui);
        self.refresh_account_view();
        self.set_screen(AppScreen::ConnectServer);
    }

    pub(super) fn reopen_connect_server(&mut self) {
        self.shell.connect.reopen(&mut self.ui);
        self.set_screen(AppScreen::ConnectServer);
    }

    pub(super) fn begin_connect(&mut self) {
        let connect = &mut self.shell.connect;
        if connect.connecting() {
            return;
        }
        let addr_text = self
            .ui
            .state_mut()
            .get_str("server_addr")
            .unwrap_or("")
            .trim()
            .to_owned();
        let (host, port) = match petramond::net::address::parse_server_address(&addr_text) {
            Ok(parts) => parts,
            Err(e) => {
                connect.phase = ConnectPhase::Failed {
                    message: e.to_string(),
                };
                return;
            }
        };
        connect.addr = addr_text.clone();
        let fallback_name =
            petramond::save::client::resolve_player_name(&petramond::save::client::load());
        remember_server(&addr_text);
        let view_distance = self.render_dist;

        connect.gen += 1;
        let gen = connect.gen;
        connect.cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&connect.cancel);
        let (tx, rx) = mpsc::channel();
        connect.rx = Some(rx);
        connect.phase = ConnectPhase::Connecting {
            label: "Connecting…",
        };
        let cache_claims = self
            .retained_section_cache
            .as_ref()
            .map(|cache| cache.claims())
            .unwrap_or_default();
        std::thread::Builder::new()
            .name("petramond-connect".to_owned())
            .spawn(move || {
                let outcome = run_connect(
                    &host,
                    port,
                    &fallback_name,
                    view_distance,
                    cache_claims,
                    &cancel,
                    |label| {
                        let _ = tx.send((gen, ConnectOutcome::Progress(label)));
                    },
                );
                let _ = tx.send((gen, outcome));
            })
            .expect("spawn connect thread");
    }

    #[cfg(test)]
    pub(super) fn poll_connect_worker(&mut self) {
        if let Some(event) = self.shell.connect.poll() {
            self.run_shell_command(connect_event_command(event));
        }
    }

    pub(super) fn start_remote_game(&mut self, join: HandshakeJoin, handle: ServerHandle) {
        let cam = Camera::new(
            petramond_math::world_pos::WorldPos::new(8.0, 90.0, 8.0),
            self.shell_camera.aspect.max(0.01),
        );
        let retained_cache = self.retained_section_cache.take();
        self.adopt_game(Game::new_remote(
            cam,
            join.join,
            handle,
            self.render_dist,
            &self.shell.connect.addr,
            &join.server_mods,
            retained_cache,
        ));
    }
}

fn remember_server(addr: &str) {
    if cfg!(test) {
        return;
    }
    let mut settings = petramond::save::client::load();
    settings.last_server = Some(addr.to_owned());
    if let Err(e) = petramond::save::client::store(&settings) {
        log::warn!("could not persist the last server address: {e}");
    }
}

fn credential_for(
    offer: &ServerOffer,
    fallback_name: &str,
) -> Result<JoinCredential, HandshakeError> {
    if !offer.requires_account {
        return Ok(JoinCredential::Name(fallback_name.to_owned()));
    }
    account::session::join_ticket_for(offer.server_id)
        .map(JoinCredential::Ticket)
        .map_err(|e| HandshakeError::Credential {
            sign_in_required: matches!(e, AccountError::SignInRequired(_)),
            message: e.message().to_owned(),
        })
}

fn run_connect(
    host: &str,
    port: u16,
    name: &str,
    view_distance: i32,
    cache_claims: Vec<petramond::net::protocol::SectionCacheClaim>,
    cancel: &AtomicBool,
    progress: impl Fn(&'static str),
) -> ConnectOutcome {
    let cancelled = || cancel.load(Ordering::Relaxed);
    let addrs: Vec<std::net::SocketAddr> = match (host, port).to_socket_addrs() {
        Ok(addrs) => addrs.collect(),
        Err(_) => return ConnectOutcome::Failed(format!("Unknown host {host}")),
    };
    if addrs.is_empty() {
        return ConnectOutcome::Failed(format!("Unknown host {host}"));
    }

    let mut stream = None;
    let mut last_err: Option<std::io::Error> = None;
    for addr in &addrs {
        if cancelled() {
            return ConnectOutcome::Failed("Cancelled".to_owned());
        }
        match TcpStream::connect_timeout(addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_err = Some(e),
        }
    }
    let mut stream = match stream {
        Some(s) => s,
        None => {
            let e = last_err.expect("no stream implies a connect error");
            return ConnectOutcome::Failed(match e.kind() {
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                    "Connection timed out".to_owned()
                }
                _ => format!("Couldn't reach {host}: {e}"),
            });
        }
    };
    if cancelled() {
        return ConnectOutcome::Failed("Cancelled".to_owned());
    }

    progress("Joining world…");
    if let Err(e) = stream.set_read_timeout(Some(CONNECT_TIMEOUT)) {
        return ConnectOutcome::Failed(format!("Couldn't reach {host}: {e}"));
    }
    let identity = match crate::game::session::player_identity() {
        Ok(identity) => identity,
        Err(e) => {
            return ConnectOutcome::Failed(format!("Couldn't load your player identity: {e}"))
        }
    };
    let (join, channel) = match client_handshake(
        &mut stream,
        &identity,
        |offer| credential_for(offer, name),
        view_distance,
        &installed_mod_ids(),
        cache_claims,
    ) {
        Ok(join) => join,
        Err(HandshakeError::MissingMods(mods)) => return ConnectOutcome::Missing(mods),
        Err(HandshakeError::Credential {
            message,
            sign_in_required: true,
        }) => return ConnectOutcome::SignInNeeded(message),
        Err(e) => return ConnectOutcome::Failed(e.to_string()),
    };
    if cancelled() {
        return ConnectOutcome::Failed("Cancelled".to_owned());
    }
    let remap = petramond::net::remap::IdRemap::build(&join.join.tables);
    let conn = match petramond::net::connection::TcpClientConn::spawn(stream, remap, channel) {
        Ok(conn) => conn,
        Err(e) => return ConnectOutcome::Failed(format!("Connection error: {e}")),
    };
    ConnectOutcome::Joined(join, ServerHandle::from_remote(conn))
}
