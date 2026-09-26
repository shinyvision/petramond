//! The Connect to Server flow: the screen's session
//! state and the ONE background thread that resolves the address, opens the
//! TCP connection, and runs the join handshake. The worker reports back over
//! an mpsc channel the ConnectServer screen drains each frame
//! ([`ConnectSession::poll`]).
//!
//! Cancellation is cooperative (a flag checked between blocking steps) plus a
//! GENERATION guard: Cancel/Back bump the session's `gen` and drop the
//! receiver, so an outcome from an abandoned attempt can never adopt a game.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use super::shell_docs::ShellCommand;
use super::ui_runtime::AppUi;
use super::{App, AppScreen};
use crate::game::Game;
use petramond::net::handle::ServerHandle;
use petramond::net::handshake::{
    client_handshake, installed_mod_ids, HandshakeError, HandshakeJoin,
};
use petramond::net::protocol::ModEntry;
use petramond_render::camera::Camera;

/// Per-step network deadline: the TCP connect and each handshake read.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Matches the document's `max_chars` for the address input.
const ADDR_MAX_CHARS: usize = 64;

pub(super) enum ConnectPhase {
    /// Fields editable, no attempt running.
    Editing,
    /// The worker thread is running; `label` is the muted progress line.
    Connecting { label: &'static str },
    /// The last attempt failed; `message` fills the danger status label.
    Failed { message: String },
}

/// What the worker reports back, tagged with the attempt's generation.
enum ConnectOutcome {
    /// Progress label change ("Joining world…" once the socket is up).
    Progress(&'static str),
    /// Handshake succeeded; the connection threads are already running.
    Joined(HandshakeJoin, ServerHandle),
    /// The server runs mods this client lacks (the join was refused).
    Missing(Vec<ModEntry>),
    Failed(String),
}

pub(super) struct ConnectSession {
    pub(super) phase: ConnectPhase,
    /// Attempt generation: outcomes tagged with an older gen are stale.
    gen: u64,
    rx: Option<Receiver<(u64, ConnectOutcome)>>,
    /// Cooperative cancel for the worker thread (checked between steps).
    cancel: Arc<AtomicBool>,
    /// The mod list of the last refused join (the ModsMissing screen's rows).
    pub(super) missing: Vec<ModEntry>,
    /// The last ATTEMPTED address/name — re-seeded into the entry fields when
    /// the ModsMissing screen returns here.
    pub(super) addr: String,
    pub(super) name: String,
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
            name: String::new(),
        }
    }
}

/// What draining the connect worker decided, for the caller to act on.
pub(super) enum ConnectEvent {
    /// The handshake succeeded; the connection threads are already running.
    Joined(Box<HandshakeJoin>, ServerHandle),
    /// The server runs mods this client lacks: the refusal's list is in
    /// [`ConnectSession::missing`].
    Missing,
}

impl ConnectSession {
    pub(super) fn connecting(&self) -> bool {
        matches!(self.phase, ConnectPhase::Connecting { .. })
    }

    /// Whether a worker thread's channel is live (tests pin that a parse
    /// failure never spawns one).
    #[cfg(test)]
    pub(super) fn has_worker(&self) -> bool {
        self.rx.is_some()
    }

    /// Abandon the in-flight attempt (Cancel/Back/ESC): flag the worker and
    /// make anything it already reported stale.
    pub(super) fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.gen += 1;
        self.rx = None;
        if self.connecting() {
            self.phase = ConnectPhase::Editing;
        }
    }

    /// Drain the worker's outcomes — the ConnectServer screen's per-frame
    /// prep. Progress and failures land in `phase`; a join or a mod refusal
    /// comes back for the caller to act on.
    pub(super) fn poll(&mut self) -> Option<ConnectEvent> {
        loop {
            let rx = self.rx.as_ref()?;
            let (gen, outcome) = match rx.try_recv() {
                Ok(msg) => msg,
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    // The worker died without a report (a panic): fail loud
                    // rather than spin on "Connecting…" forever.
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
            }
        }
    }

    /// Reset for a fresh open of the Connect screen, its fields seeded from
    /// client.json (`last_server` + the resolved player name).
    pub(super) fn open_fresh(&mut self, ui: &mut AppUi) {
        let settings = petramond::save::client::load();
        let addr = settings.last_server.clone().unwrap_or_default();
        let name = petramond::save::client::resolve_player_name(&settings);
        *self = ConnectSession::default();
        seed_connect_fields(ui, &addr, name);
    }

    /// Back from the ModsMissing screen: the refused attempt's address and
    /// name intact.
    pub(super) fn reopen(&mut self, ui: &mut AppUi) {
        self.phase = ConnectPhase::Editing;
        let (addr, name) = (self.addr.clone(), self.name.clone());
        seed_connect_fields(ui, &addr, name);
    }
}

/// The shell command a connect event asks for.
pub(super) fn connect_event_command(event: ConnectEvent) -> ShellCommand {
    match event {
        ConnectEvent::Joined(join, handle) => ShellCommand::AdoptRemote(join, handle),
        // A refusal lists the mods this client lacks; its Back reopens the
        // connect screen with the attempt intact.
        ConnectEvent::Missing => ShellCommand::Goto(AppScreen::ModsMissing),
    }
}

/// Seed the connect document's entry fields and focus the address.
fn seed_connect_fields(ui: &mut AppUi, addr: &str, name: String) {
    // Activate the document FIRST: switching kinds resets bound state, which
    // would wipe the seeds below on the screen's first frame.
    ui.ensure_active(petramond_world::gui_state::GuiKind::ConnectServer);
    let state = ui.state_mut();
    state.set("server_addr", petramond_ui::UiValue::Str(addr.to_owned()));
    state.set("player_name", petramond_ui::UiValue::Str(name));
    // Ready to type immediately, editing from the prefill.
    ui.focus_text_input("server_addr", addr, ADDR_MAX_CHARS);
}

impl App {
    /// Open the Connect to Server screen from the title: fields prefilled
    /// from client.json (`last_server` + the resolved player name).
    pub(super) fn open_connect_server(&mut self) {
        self.shell.connect.open_fresh(&mut self.ui);
        self.set_screen(AppScreen::ConnectServer);
    }

    /// Back from the ModsMissing screen: same screen, the refused attempt's
    /// address and name intact.
    pub(super) fn reopen_connect_server(&mut self) {
        self.shell.connect.reopen(&mut self.ui);
        self.set_screen(AppScreen::ConnectServer);
    }

    /// The Connect button/Enter: validate the fields, persist them, and spawn
    /// the worker thread. Parse failures show inline without any thread.
    pub(super) fn begin_connect(&mut self) {
        let connect = &mut self.shell.connect;
        if connect.connecting() {
            return;
        }
        let state = self.ui.state_mut();
        let addr_text = state.get_str("server_addr").unwrap_or("").trim().to_owned();
        let name = state.get_str("player_name").unwrap_or("").trim().to_owned();
        let (host, port) = match petramond::net::address::parse_server_address(&addr_text) {
            Ok(parts) => parts,
            Err(e) => {
                connect.phase = ConnectPhase::Failed {
                    message: e.to_string(),
                };
                return;
            }
        };
        if name.is_empty() {
            connect.phase = ConnectPhase::Failed {
                message: "Enter a player name".to_owned(),
            };
            return;
        }
        connect.addr = addr_text.clone();
        connect.name = name.clone();
        persist_connect_fields(&addr_text, &name);
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
        // Claim the retained section cache in the Join manifest. Stale or
        // wrong-server claims are free: they hash-mismatch into ordinary
        // full sends (or heal through SectionCacheMiss).
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
                    &name,
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

    /// Drain the connect worker and act on what it decided, outside the
    /// screen's frame — for tests that wait on a real join.
    #[cfg(test)]
    pub(super) fn poll_connect_worker(&mut self) {
        if let Some(event) = self.shell.connect.poll() {
            self.run_shell_command(connect_event_command(event));
        }
    }

    /// Enter the joined REMOTE session — `start_game`'s tail for a handshaked
    /// connection. The camera position is irrelevant: the constructor snaps
    /// it to the restored player.
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

/// Remember the attempt on disk: `last_server` prefills the next open and
/// `player_name` becomes the sticky display name. Suppressed under test — the
/// suite must never rewrite the developer's real client.json.
fn persist_connect_fields(addr: &str, name: &str) {
    if cfg!(test) {
        return;
    }
    let mut settings = petramond::save::client::load();
    settings.last_server = Some(addr.to_owned());
    settings.player_name = Some(name.to_owned());
    if let Err(e) = petramond::save::client::store(&settings) {
        log::warn!("could not persist the connect fields: {e}");
    }
}

/// The whole blocking connect sequence, on the worker thread: DNS → TCP
/// connect (each resolved address, [`CONNECT_TIMEOUT`] apiece) → read
/// deadline → join handshake → connection threads + remote handle. The
/// cancel flag is honoured between blocking steps; a cancelled attempt's
/// outcome is stale by generation anyway, so the exact drop point only
/// affects how soon the socket closes.
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
    let join = match client_handshake(
        &mut stream,
        &identity,
        name,
        view_distance,
        &installed_mod_ids(),
        cache_claims,
    ) {
        Ok(join) => join,
        // No farewell frame after a mod refusal — just drop the socket.
        Err(HandshakeError::MissingMods(mods)) => return ConnectOutcome::Missing(mods),
        Err(e) => return ConnectOutcome::Failed(e.to_string()),
    };
    if cancelled() {
        // Dropping the raw socket is the leave: the server reader hits EOF.
        return ConnectOutcome::Failed("Cancelled".to_owned());
    }
    // Canonical order: the id remap from the join
    // tables, connection threads over the post-handshake stream, then the
    // handle that fronts them.
    let remap = petramond::net::remap::IdRemap::build(&join.join.tables);
    let conn = match petramond::net::connection::TcpClientConn::spawn(stream, remap) {
        Ok(conn) => conn,
        Err(e) => return ConnectOutcome::Failed(format!("Connection error: {e}")),
    };
    ConnectOutcome::Joined(join, ServerHandle::from_remote(conn))
}
