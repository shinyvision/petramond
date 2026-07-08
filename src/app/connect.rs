//! The Connect to Server flow (multiplayer Phase E2): the screen's session
//! state and the ONE background thread that resolves the address, opens the
//! TCP connection, and runs the join handshake. The worker reports back over
//! an mpsc channel the ConnectServer screen drains each frame
//! ([`App::poll_connect_worker`]).
//!
//! Cancellation is cooperative (a flag checked between blocking steps) plus a
//! GENERATION guard: Cancel/Back bump the session's `gen` and drop the
//! receiver, so an outcome from an abandoned attempt can never adopt a game.

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use super::{App, AppScreen};
use crate::camera::Camera;
use crate::game::Game;
use crate::mathh::Vec3;
use crate::net::handshake::{client_handshake, installed_mod_ids, HandshakeError};
use crate::net::protocol::{JoinData, ModEntry};
use crate::server::handle::ServerHandle;

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
    Joined(Box<JoinData>, ServerHandle),
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
}

impl App {
    /// Open the Connect to Server screen from the title: fields prefilled
    /// from client.json (`last_server` + the resolved player name).
    pub(super) fn open_connect_server(&mut self) {
        let settings = crate::save::client::load();
        let addr = settings.last_server.clone().unwrap_or_default();
        let name = crate::save::client::resolve_player_name(&settings);
        self.connect = ConnectSession::default();
        self.enter_connect_screen(addr, name);
    }

    /// Back from the ModsMissing screen: same screen, the refused attempt's
    /// address and name intact.
    pub(super) fn reopen_connect_server(&mut self) {
        let (addr, name) = (self.connect.addr.clone(), self.connect.name.clone());
        self.connect.phase = ConnectPhase::Editing;
        self.enter_connect_screen(addr, name);
    }

    fn enter_connect_screen(&mut self, addr: String, name: String) {
        // Activate the document FIRST: switching kinds resets bound state,
        // which would wipe the seeds below on the screen's first frame.
        self.ui.ensure_active(crate::gui::GuiKind::ConnectServer);
        let state = self.ui.state_mut();
        state.set("server_addr", llama_ui::UiValue::Str(addr.clone()));
        state.set("player_name", llama_ui::UiValue::Str(name));
        // Ready to type immediately, editing from the prefill.
        self.ui
            .focus_text_input("server_addr", &addr, ADDR_MAX_CHARS);
        self.screen = AppScreen::ConnectServer;
        self.pointer.release_for_menu();
    }

    /// The Connect button/Enter: validate the fields, persist them, and spawn
    /// the worker thread. Parse failures show inline without any thread.
    pub(super) fn begin_connect(&mut self) {
        if self.connect.connecting() {
            return;
        }
        let state = self.ui.state_mut();
        let addr_text = state.get_str("server_addr").unwrap_or("").trim().to_owned();
        let name = state.get_str("player_name").unwrap_or("").trim().to_owned();
        let (host, port) = match crate::net::address::parse_server_address(&addr_text) {
            Ok(parts) => parts,
            Err(e) => {
                self.connect.phase = ConnectPhase::Failed {
                    message: e.to_string(),
                };
                return;
            }
        };
        if name.is_empty() {
            self.connect.phase = ConnectPhase::Failed {
                message: "Enter a player name".to_owned(),
            };
            return;
        }
        self.connect.addr = addr_text.clone();
        self.connect.name = name.clone();
        persist_identity(&addr_text, &name);

        self.connect.gen += 1;
        let gen = self.connect.gen;
        self.connect.cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&self.connect.cancel);
        let (tx, rx) = mpsc::channel();
        self.connect.rx = Some(rx);
        self.connect.phase = ConnectPhase::Connecting {
            label: "Connecting…",
        };
        std::thread::Builder::new()
            .name("llamacraft-connect".to_owned())
            .spawn(move || {
                let outcome = run_connect(&host, port, &name, &cancel, |label| {
                    let _ = tx.send((gen, ConnectOutcome::Progress(label)));
                });
                let _ = tx.send((gen, outcome));
            })
            .expect("spawn connect thread");
    }

    /// Abandon the in-flight attempt (Cancel/Back/ESC): flag the worker and
    /// make anything it already reported stale.
    pub(super) fn cancel_connect(&mut self) {
        self.connect.cancel.store(true, Ordering::Relaxed);
        self.connect.gen += 1;
        self.connect.rx = None;
        if self.connect.connecting() {
            self.connect.phase = ConnectPhase::Editing;
        }
    }

    /// Drain the worker's outcomes — the ConnectServer screen's per-frame
    /// prep. A join adopts the remote game; a mod refusal opens ModsMissing;
    /// failures land in the inline status label.
    pub(super) fn poll_connect_worker(&mut self) {
        loop {
            let Some(rx) = self.connect.rx.as_ref() else {
                return;
            };
            let (gen, outcome) = match rx.try_recv() {
                Ok(msg) => msg,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    // The worker died without a report (a panic): fail loud
                    // rather than spin on "Connecting…" forever.
                    self.connect.rx = None;
                    if self.connect.connecting() {
                        self.connect.phase = ConnectPhase::Failed {
                            message: "The connection attempt failed".to_owned(),
                        };
                    }
                    return;
                }
            };
            if gen != self.connect.gen {
                continue;
            }
            match outcome {
                ConnectOutcome::Progress(label) => {
                    if self.connect.connecting() {
                        self.connect.phase = ConnectPhase::Connecting { label };
                    }
                }
                ConnectOutcome::Joined(join, handle) => {
                    self.connect.rx = None;
                    self.connect.phase = ConnectPhase::Editing;
                    self.start_remote_game(join, handle);
                    return;
                }
                ConnectOutcome::Missing(mods) => {
                    self.connect.rx = None;
                    self.connect.phase = ConnectPhase::Editing;
                    self.connect.missing = mods;
                    self.screen = AppScreen::ModsMissing;
                    return;
                }
                ConnectOutcome::Failed(message) => {
                    self.connect.rx = None;
                    self.connect.phase = ConnectPhase::Failed { message };
                }
            }
        }
    }

    /// Enter the joined REMOTE session — `start_game`'s tail for a handshaked
    /// connection. The camera position is irrelevant: the constructor snaps
    /// it to the restored player.
    fn start_remote_game(&mut self, join: Box<JoinData>, handle: ServerHandle) {
        let cam = Camera::new(
            Vec3::new(8.0, 90.0, 8.0),
            self.shell_camera.aspect.max(0.01),
        );
        self.adopt_game(Game::new_remote(cam, join, handle, self.render_dist));
    }
}

/// Remember the attempt on disk: `last_server` prefills the next open and
/// `player_name` becomes the sticky identity. Suppressed under test — the
/// suite must never rewrite the developer's real client.json.
fn persist_identity(addr: &str, name: &str) {
    if cfg!(test) {
        return;
    }
    let mut settings = crate::save::client::load();
    settings.last_server = Some(addr.to_owned());
    settings.player_name = Some(name.to_owned());
    if let Err(e) = crate::save::client::store(&settings) {
        log::warn!("could not persist client identity: {e}");
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
    let join = match client_handshake(&mut stream, name, &installed_mod_ids()) {
        Ok(join) => join,
        // No farewell frame after a mod refusal — just drop the socket.
        Err(HandshakeError::MissingMods(mods)) => return ConnectOutcome::Missing(mods),
        Err(e) => return ConnectOutcome::Failed(e.to_string()),
    };
    if cancelled() {
        // Dropping the raw socket is the leave: the server reader hits EOF.
        return ConnectOutcome::Failed("Cancelled".to_owned());
    }
    // Canonical order (WIKI/multiplayer.md): the id remap from the join
    // tables, connection threads over the post-handshake stream, then the
    // handle that fronts them.
    let remap = crate::net::remap::IdRemap::build(&join.tables);
    let conn = match crate::net::connection::TcpClientConn::spawn(stream, remap) {
        Ok(conn) => conn,
        Err(e) => return ConnectOutcome::Failed(format!("Connection error: {e}")),
    };
    ConnectOutcome::Joined(join, ServerHandle::from_remote(conn))
}
