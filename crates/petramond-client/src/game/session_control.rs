//! Controls over the running session rather than the player: saving and
//! shutdown, pause, chat, live graphics options, LAN, and noticing a lost
//! connection.

use super::Game;
use petramond::net::protocol::{ChatLine, ClientToServer};

impl Game {
    /// Ask the server to persist everything (world chunks, level.dat,
    /// per-player files, mod set) — a control message; the save happens on the
    /// server thread. For the QUIT path use [`Game::shutdown`], which also
    /// joins the thread.
    pub fn save_all(&mut self) {
        self.handle.save_all();
    }

    /// Quit this session: the server thread saves everything (what `save_all`
    /// did) and exits; returns once it is joined.
    pub fn shutdown(mut self) {
        self.handle.shutdown_and_join();
    }

    /// Singleplayer pause (the pause menu): the server keeps draining
    /// messages, streaming, and autosaving, but skips the fixed ticks and
    /// banks no tick debt. Honored server-side only while it has never been
    /// open to LAN (the sole connection is this local one); once opened, the
    /// server ignores Pause. While paused the app must keep calling
    /// [`Game::pump_network`] so server output is still consumed.
    pub fn set_paused(&mut self, paused: bool) {
        // A remote client never pauses the shared server (which also gates:
        // once opened to LAN, Pause is ignored) — belt and braces.
        if self.remote {
            return;
        }
        if self.handle.send(ClientToServer::Pause(paused)).is_err() {
            self.note_connection_lost();
        }
    }

    pub fn send_chat(&mut self, text: String) {
        self.outbox.push(ClientToServer::ChatSend { text });
    }

    /// Apply the particles graphics option to the client-local fleck system
    /// (mining dust, break/splash bursts). Presentation-only; the same scale
    /// gates the looping-emitter gather and thins each emitter's active
    /// particle window in the renderer.
    pub fn set_particles_mode(&mut self, mode: petramond::save::client::ParticlesMode) {
        self.particles.set_count_scale(mode.density());
    }

    /// Change the client view distance live: the replica re-shapes its
    /// mesh/light priority ring, and the server is asked to stream the new
    /// radius (it clamps to its own maximum).
    pub fn set_view_distance(&mut self, chunks: i32) {
        let chunks = chunks.clamp(4, 64);
        self.replica.set_render_dist(chunks);
        let msg = ClientToServer::SetViewDistance {
            chunks: chunks as u8,
        };
        if self.remote {
            self.outbox.push(msg);
        } else if self.handle.send(msg).is_err() {
            self.note_connection_lost();
        }
    }

    pub fn take_chat_lines(&mut self) -> Vec<ChatLine> {
        std::mem::take(&mut self.pending_chat_lines)
    }

    /// Whether this session fronts a REMOTE server (joined over TCP) rather
    /// than the in-process host thread.
    #[inline]
    pub fn is_remote(&self) -> bool {
        self.remote
    }

    /// Open the running HOST server to LAN on `port`; `Ok` carries the
    /// actual bound port. Host only — the pause menu hides the button for
    /// remote sessions (and a remote handle has no control channel to ask).
    pub fn open_to_lan(&mut self, port: u16) -> std::io::Result<u16> {
        debug_assert!(!self.remote, "open_to_lan is a host action");
        self.handle.open_to_lan(port)
    }

    /// One-shot: the latched connection-loss reason if it has not yet been
    /// surfaced. `Game::tick` reports through `GameEvents::connection_lost`;
    /// the app polls THIS on frames that skip the tick (shell screens over a
    /// live game — the pause menu) so a loss detected by
    /// [`Game::pump_network`] still reaches the Disconnected screen.
    pub fn take_connection_lost(&mut self) -> Option<String> {
        if self.connection_lost_reported {
            return None;
        }
        let reason = self.connection_lost.clone()?;
        self.connection_lost_reported = true;
        log::error!("{reason}; nothing further will be saved");
        Some(reason)
    }

    /// Latch the server as unreachable (crashed thread / closed channel /
    /// lost TCP connection); reported exactly once through
    /// `GameEvents::connection_lost`.
    pub(super) fn note_connection_lost(&mut self) {
        self.note_connection_lost_because("world stopped: the server is gone");
    }

    /// [`note_connection_lost`](Self::note_connection_lost) with an explicit
    /// reason (`ServerClosing` / a server `Disconnect`); the first reason
    /// latched wins.
    pub(super) fn note_connection_lost_because(&mut self, reason: &str) {
        if self.connection_lost.is_none() {
            self.connection_lost = Some(reason.to_string());
        }
    }
}
