//! The session's link to its server: the [`ServerHandle`], the per-frame
//! message batch, streaming flow-control feedback, and connection-loss
//! latching. Nothing else in the client touches the handle — gameplay code
//! queues messages here and the frame driver flushes them.

use std::time::{Duration, Instant};

use petramond::net::handle::ServerHandle;
use petramond::net::protocol::{ClientToServer, ServerToClient};

/// How often the replica's presentation backlog is reported to the server.
const BACKLOG_REPORT_INTERVAL: Duration = Duration::from_millis(100);

pub(super) struct NetLink {
    /// The handle to the SIMULATION — `ServerGame` on its own self-clocked
    /// thread, or a remote server's TCP connection. Input reaches it only as
    /// messages; state comes back only as drained server→client messages.
    handle: ServerHandle,
    /// Whether this session is a REMOTE client (built over a TCP
    /// connection). Gates host-only actions: pause, open-to-LAN,
    /// save-and-quit.
    remote: bool,
    /// One-shot client→server messages queued by the app-facing methods since
    /// the last frame, appended after this frame's `PlayerUpdate` + click
    /// edges.
    outbox: Vec<ClientToServer>,
    /// Per-frame scratch for the assembled message batch (capacity reused;
    /// [`flush_frame`](Self::flush_frame) drains it every frame).
    frame: Vec<ClientToServer>,
    /// Per-frame scratch for drained server messages (capacity reused).
    incoming: Vec<ServerToClient>,
    /// `Some(reason)` once the server is unreachable (thread crashed, or a
    /// send/drain hit a closed channel). Latched once; the first reason wins.
    connection_lost: Option<String>,
    /// Whether `connection_lost` was already surfaced (log + event) — the
    /// error is reported exactly once.
    connection_lost_reported: bool,
    /// When the currently-open streaming batch's `StreamBatchStart` was
    /// applied; `StreamBatchEnd` closes it into a rate sample and an ack.
    stream_batch_started: Option<Instant>,
    /// EMA over measured batch apply rates (streaming messages/second) — what
    /// `StreamBatchAck` reports so the server sizes future batches to this
    /// client's real throughput.
    stream_rate_ema: Option<f32>,
    /// When the replica's presentation backlog was last reported.
    backlog_reported_at: Option<Instant>,
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
            backlog_reported_at: None,
        }
    }

    #[inline]
    pub(super) fn is_remote(&self) -> bool {
        self.remote
    }

    /// Queue a one-shot message for the next frame's batch.
    pub(super) fn queue(&mut self, msg: ClientToServer) {
        self.outbox.push(msg);
    }

    /// Queue several one-shot messages, in order.
    pub(super) fn queue_all(&mut self, msgs: impl IntoIterator<Item = ClientToServer>) {
        self.outbox.extend(msgs);
    }

    /// Send `msg` right away, outside the frame batch — for control and
    /// flow-control messages that must flow even on frames that never reach
    /// the frame driver. A closed channel latches the connection as lost.
    pub(super) fn send_now(&mut self, msg: ClientToServer) {
        if self.handle.send(msg).is_err() {
            self.note_lost();
        }
    }

    /// Append one message to this frame's batch (the frame driver's
    /// `PlayerUpdate` and click edges, in consumption order).
    pub(super) fn push_frame(&mut self, msg: ClientToServer) {
        self.frame.push(msg);
    }

    /// Whether this frame's batch is still empty (the driver asserts it
    /// before assembling a new one).
    pub(super) fn frame_is_empty(&self) -> bool {
        self.frame.is_empty()
    }

    /// Close this frame's batch — everything queued since the last frame goes
    /// after the frame's own messages — and send it. The first failed send
    /// latches the connection as lost and drops the rest.
    pub(super) fn flush_frame(&mut self) {
        self.frame.append(&mut self.outbox);
        let mut lost = false;
        for msg in self.frame.drain(..) {
            if lost {
                continue; // drain the rest; the server is gone
            }
            lost = self.handle.send(msg).is_err();
        }
        if lost {
            self.note_lost();
        }
    }

    /// Take every pending server→client message, in order. Hand the buffer
    /// back through [`recycle`](Self::recycle) so its capacity is reused. A
    /// crashed server latches the connection as lost here.
    pub(super) fn drain(&mut self) -> Vec<ServerToClient> {
        if self.handle.is_crashed() {
            self.note_lost();
        }
        let mut msgs = std::mem::take(&mut self.incoming);
        self.handle.drain(&mut msgs);
        msgs
    }

    /// Return a drained (now empty) message buffer for reuse.
    pub(super) fn recycle(&mut self, msgs: Vec<ServerToClient>) {
        debug_assert!(msgs.is_empty(), "recycled buffers are drained");
        self.incoming = msgs;
    }

    /// A streaming batch opened (`StreamBatchStart`): start its timing window.
    pub(super) fn stream_batch_started(&mut self) {
        self.stream_batch_started = Some(Instant::now());
    }

    /// A streaming batch of `count` messages closed (`StreamBatchEnd`): fold
    /// the measured apply rate into the EMA and ack it RIGHT AWAY (acks must
    /// flow even on frames that never reach the frame driver, or the server's
    /// window starves). The server clamps whatever we report.
    pub(super) fn stream_batch_ended(&mut self, count: u32) {
        let Some(started) = self.stream_batch_started.take() else {
            return; // End without Start: tolerate, nothing to measure
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

    /// Report the replica's presentation backlog, at most every
    /// [`BACKLOG_REPORT_INTERVAL`]; `backlog` is only evaluated when a report
    /// is due.
    pub(super) fn report_terrain_backlog(&mut self, backlog: impl FnOnce() -> (u32, u32)) {
        if self
            .backlog_reported_at
            .is_some_and(|at| at.elapsed() < BACKLOG_REPORT_INTERVAL)
        {
            return;
        }
        let (mesh_sections, upload_columns) = backlog();
        let msg = ClientToServer::TerrainBacklog {
            mesh_sections,
            upload_columns,
        };
        if self.handle.send(msg).is_ok() {
            self.backlog_reported_at = Some(Instant::now());
        }
    }

    /// Latch the server as unreachable (crashed thread / closed channel /
    /// lost TCP connection); reported exactly once.
    pub(super) fn note_lost(&mut self) {
        self.note_lost_because("world stopped: the server is gone");
    }

    /// [`note_lost`](Self::note_lost) with an explicit reason
    /// (`ServerClosing` / a server `Disconnect`); the first reason latched
    /// wins.
    pub(super) fn note_lost_because(&mut self, reason: &str) {
        if self.connection_lost.is_none() {
            self.connection_lost = Some(reason.to_string());
        }
    }

    /// One-shot: the latched connection-loss reason if it has not yet been
    /// surfaced (logged as it is taken).
    pub(super) fn take_lost_report(&mut self) -> Option<String> {
        if self.connection_lost_reported {
            return None;
        }
        let reason = self.connection_lost.clone()?;
        self.connection_lost_reported = true;
        log::error!("{reason}; nothing further will be saved");
        Some(reason)
    }

    /// Ask the server to save everything now (it keeps running).
    pub(super) fn save_all(&self) {
        self.handle.save_all();
    }

    /// Shut the server down (saving) and join it.
    pub(super) fn shutdown(&mut self) {
        self.handle.shutdown_and_join();
    }

    /// Open the running HOST server to LAN on `port`.
    pub(super) fn open_to_lan(&self, port: u16) -> std::io::Result<u16> {
        debug_assert!(!self.remote, "open_to_lan is a host action");
        self.handle.open_to_lan(port)
    }

    /// Take the queued one-shot messages, for a test harness that services
    /// the server end synchronously.
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

    #[test]
    fn backlog_reports_are_throttled() {
        let (handle, server) = ServerHandle::loopback();
        let mut link = NetLink::new(handle, false);
        link.report_terrain_backlog(|| (1, 2));
        link.report_terrain_backlog(|| panic!("not due yet"));
        assert!(matches!(
            server.inbox.try_recv(),
            Ok(ClientToServer::TerrainBacklog {
                mesh_sections: 1,
                upload_columns: 2
            })
        ));
        assert!(server.inbox.try_recv().is_err());
    }
}
