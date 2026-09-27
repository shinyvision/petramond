use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

use crate::events::tick::TickEvents;
use crate::player::PlayerId;

use super::ServerGame;

pub type Evicted = (PlayerId, Option<String>);

fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

impl ServerGame {
    pub(super) fn isolated<R>(
        &mut self,
        s: usize,
        what: &str,
        f: impl FnOnce(&mut Self) -> R,
    ) -> Option<R> {
        match catch_unwind(AssertUnwindSafe(|| f(self))) {
            Ok(result) => Some(result),
            Err(payload) => {
                if s == 0 && self.sessions.has_local_session() {
                    resume_unwind(payload);
                }
                let id = self.sessions[s].id;
                log::error!(
                    "session {} ('{}') panicked in {what}: {}; kicking it",
                    id.0,
                    self.sessions[s].name,
                    panic_message(payload.as_ref())
                );
                self.sessions.mark_faulted(id);
                None
            }
        }
    }

    pub(super) fn for_each_session(
        &mut self,
        what: &str,
        events: &mut TickEvents,
        f: impl Fn(&mut Self, usize, &mut TickEvents),
    ) {
        for s in 0..self.sessions.len() {
            if self.sessions.is_faulted(s) {
                continue;
            }
            self.isolated(s, what, |server| f(server, s, events));
        }
    }

    pub(super) fn evict_faulted(&mut self) -> Vec<Evicted> {
        let mut evicted = Vec::new();
        for id in self.sessions.take_faulted() {
            let left = catch_unwind(AssertUnwindSafe(|| self.remove_remote_session(id)));
            let name = match left {
                Ok(name) => name,
                Err(payload) => {
                    log::error!(
                        "session {} could not leave cleanly ({}); dropped unsaved",
                        id.0,
                        panic_message(payload.as_ref())
                    );
                    if let Some(s) = self.sessions.index_of(id) {
                        self.sessions.leave(s);
                    }
                    None
                }
            };
            if let Some(name) = &name {
                self.chat.left(name);
            }
            evicted.push((id, name));
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use crate::net::protocol::ClientToServer;

    #[test]
    fn a_panicking_session_is_kicked_not_the_server() {
        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        let pos = server.sessions[0].player.pos;
        let s = server.add_session_for_test(crate::player::Player::new(pos));
        let id = server.sessions[s].id;
        let tick = server.world.current_tick();

        let ran = server.isolated::<()>(s, "a test stage", |_| panic!("injected"));
        assert!(ran.is_none());
        assert!(server.sessions.is_faulted(s));

        let out = server.pump_tagged(
            crate::events::tick::TICK_DT * 1.01,
            &mut vec![(id, ClientToServer::KeepAlive)],
            &[],
        );
        assert_eq!(out.kicked.len(), 1);
        assert_eq!(out.kicked[0].0, id);
        assert!(
            server.sessions.by_id(id).is_none(),
            "the faulted session left"
        );
        assert_eq!(server.sessions.len(), 1, "the host plays on");
        assert!(server.world.current_tick() > tick, "the world kept ticking");
        assert!(
            out.remote.iter().all(|(rid, _)| *rid != id),
            "nothing is routed to a kicked session"
        );
    }

    #[test]
    fn a_local_session_fault_still_unwinds() {
        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            server.isolated::<()>(0, "a test stage", |_| panic!("injected"))
        }));
        assert!(unwound.is_err());
    }
}
