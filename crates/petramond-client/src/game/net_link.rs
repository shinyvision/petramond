use std::time::Instant;

use petramond::net::handle::ServerHandle;
use petramond::net::protocol::{ClientToServer, ServerToClient};

pub(super) struct NetLink {
    handle: ServerHandle,
    remote: bool,
    outbox: Vec<ClientToServer>,
    frame: Vec<ClientToServer>,
    incoming: Vec<ServerToClient>,
    connection_lost: Option<String>,
    connection_lost_reported: bool,
    stream_batch_started: Option<Instant>,
    stream_rate_ema: Option<f32>,
}

impl NetLink {
    pub(super) fn new(handle: ServerHandle, remote: bool) -> Self {
        Self {
            handle,
            remote,
            outbox: Vec::new(),
            frame: Vec::new(),
            incoming: Vec::new(),
            connection_lost: None,
            connection_lost_reported: false,
            stream_batch_started: None,
            stream_rate_ema: None,
        }
    }

    #[inline]
    pub(super) fn is_remote(&self) -> bool {
        self.remote
    }

    pub(super) fn queue(&mut self, msg: ClientToServer) {
        self.outbox.push(msg);
    }

    pub(super) fn queue_all(&mut self, msgs: impl IntoIterator<Item = ClientToServer>) {
        self.outbox.extend(msgs);
    }

    pub(super) fn send_now(&mut self, msg: ClientToServer) {
        if self.handle.send(msg).is_err() {
            self.note_lost();
        }
    }

    pub(super) fn push_frame(&mut self, msg: ClientToServer) {
        self.frame.push(msg);
    }

    pub(super) fn frame_is_empty(&self) -> bool {
        self.frame.is_empty()
    }

    pub(super) fn flush_frame(&mut self) {
        self.frame.append(&mut self.outbox);
        let mut lost = false;
        for msg in self.frame.drain(..) {
            if lost {
                continue;
            }
            lost = self.handle.send(msg).is_err();
        }
        if lost {
            self.note_lost();
        }
    }

    pub(super) fn drain(&mut self) -> Vec<ServerToClient> {
        if self.handle.is_crashed() {
            self.note_lost();
        }
        let mut msgs = std::mem::take(&mut self.incoming);
        self.handle.drain(&mut msgs);
        msgs
    }

    pub(super) fn inject(&mut self, msg: ServerToClient) {
        self.incoming.push(msg);
    }

    pub(super) fn recycle(&mut self, msgs: Vec<ServerToClient>) {
        debug_assert!(msgs.is_empty(), "recycled buffers are drained");
        self.incoming = msgs;
    }

    pub(super) fn stream_batch_started(&mut self) {
        self.stream_batch_started = Some(Instant::now());
    }

    pub(super) fn stream_batch_ended(&mut self, count: u32) {
        let Some(started) = self.stream_batch_started.take() else {
            return;
        };
        let elapsed = started.elapsed().as_secs_f32().max(1e-4);
        let sampled = count as f32 / elapsed;
        let rate = match self.stream_rate_ema {
            Some(ema) => ema * 0.75 + sampled * 0.25,
            None => sampled,
        };
        self.stream_rate_ema = Some(rate);
        self.send_now(ClientToServer::StreamBatchAck {
            messages_per_second: rate,
        });
    }

    pub(super) fn note_lost(&mut self) {
        self.note_lost_because("world stopped: the server is gone");
    }

    pub(super) fn note_lost_because(&mut self, reason: &str) {
        if self.connection_lost.is_none() {
            self.connection_lost = Some(reason.to_string());
        }
    }

    pub(super) fn take_lost_report(&mut self) -> Option<String> {
        if self.connection_lost_reported {
            return None;
        }
        let reason = self.connection_lost.clone()?;
        self.connection_lost_reported = true;
        log::error!("{reason}; nothing further will be saved");
        Some(reason)
    }

    pub(super) fn save_all(&self) {
        self.handle.save_all();
    }

    pub(super) fn shutdown(&mut self) {
        self.handle.shutdown_and_join();
    }

    pub(super) fn open_to_lan(&self, port: u16) -> std::io::Result<u16> {
        debug_assert!(!self.remote, "open_to_lan is a host action");
        self.handle.open_to_lan(port)
    }

    #[cfg(test)]
    pub(super) fn take_outbox_for_test(&mut self) -> Vec<ClientToServer> {
        std::mem::take(&mut self.outbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_sends_its_own_messages_before_the_queued_ones() {
        let (handle, server) = ServerHandle::loopback();
        let mut link = NetLink::new(handle, false);
        link.queue(ClientToServer::ChatSend { text: "hi".into() });
        link.push_frame(ClientToServer::KeepAlive);
        link.flush_frame();
        assert!(link.frame_is_empty());
        assert!(matches!(
            server.inbox.try_recv(),
            Ok(ClientToServer::KeepAlive)
        ));
        assert!(matches!(
            server.inbox.try_recv(),
            Ok(ClientToServer::ChatSend { .. })
        ));
        assert!(link.take_lost_report().is_none());
    }

    #[test]
    fn a_dead_server_is_reported_once_with_the_first_reason() {
        let (handle, server) = ServerHandle::loopback();
        drop(server);
        let mut link = NetLink::new(handle, false);
        link.note_lost_because("the server closed");
        link.send_now(ClientToServer::KeepAlive);
        assert_eq!(
            link.take_lost_report().as_deref(),
            Some("the server closed")
        );
        assert!(link.take_lost_report().is_none(), "reported exactly once");
    }

    #[test]
    fn a_closed_stream_batch_acks_its_measured_rate() {
        let (handle, server) = ServerHandle::loopback();
        let mut link = NetLink::new(handle, false);
        link.stream_batch_ended(10);
        assert!(
            server.inbox.try_recv().is_err(),
            "End without Start acks nothing"
        );
        link.stream_batch_started();
        link.stream_batch_ended(10);
        match server.inbox.try_recv() {
            Ok(ClientToServer::StreamBatchAck {
                messages_per_second,
            }) => {
                assert!(messages_per_second > 0.0);
            }
            _ => panic!("expected a StreamBatchAck"),
        }
    }
}
