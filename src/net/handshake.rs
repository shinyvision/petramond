//! The client's join handshake (multiplayer Phase E), pure over any
//! `Read + Write` stream so it unit-tests over an in-memory transcript.
//!
//! Exact sequence (see WIKI/multiplayer.md):
//! `Hello{protocol}` → `HelloAck` (or `HelloReject` = protocol mismatch) →
//! `ModQuery` → `ModList{mods}` → compare ids against the installed packs
//! (missing = CLOSE the socket, no farewell frame — the caller drops the
//! stream) → `Join{player_name}` → `JoinAccept(JoinData)` (or `JoinReject`).
//!
//! The function is I/O-agnostic: the caller sets per-read deadlines on the
//! raw `TcpStream` (`set_read_timeout`, ~5 s) before calling; timeouts
//! surface as [`HandshakeError::Timeout`]. Reads never over-read a frame, so
//! the stream hands off cleanly to the connection threads afterwards.

use std::collections::BTreeSet;
use std::io::{self, Read, Write};

use super::framing::{read_msg, write_msg};
use super::protocol::{ClientToServer, JoinData, JoinRejectReason, ModEntry, ServerToClient};
use super::PROTOCOL_VERSION;

#[derive(Debug)]
pub(crate) enum HandshakeError {
    Io(io::Error),
    /// A reply did not arrive within the stream's read deadline.
    Timeout,
    /// The server speaks a different protocol version.
    ProtocolMismatch {
        server: u16,
    },
    /// The server runs mods this client does not have installed.
    MissingMods(Vec<ModEntry>),
    /// The server refused the join (e.g. the name is taken).
    Rejected(JoinRejectReason),
    /// The server closed the connection mid-handshake.
    Closed,
    /// The server answered with something unparseable / out of sequence.
    BadFrame,
}

impl std::fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandshakeError::Io(e) => write!(f, "Connection error: {e}"),
            HandshakeError::Timeout => write!(f, "The server did not respond"),
            HandshakeError::ProtocolMismatch { server } => write!(
                f,
                "Incompatible version (server protocol {server}, yours {PROTOCOL_VERSION})"
            ),
            HandshakeError::MissingMods(mods) => {
                write!(f, "Missing mods: ")?;
                for (i, m) in mods.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    if m.version.is_empty() {
                        write!(f, "{}", m.id)?;
                    } else {
                        write!(f, "{} v{}", m.id, m.version)?;
                    }
                }
                Ok(())
            }
            HandshakeError::Rejected(JoinRejectReason::NameTaken) => {
                write!(f, "That player name is already taken on this server")
            }
            HandshakeError::Closed => write!(f, "The server closed the connection"),
            HandshakeError::BadFrame => write!(f, "The server sent an invalid reply"),
        }
    }
}

fn map_io(e: io::Error) -> HandshakeError {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => HandshakeError::Timeout,
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted => HandshakeError::Closed,
        io::ErrorKind::InvalidData => HandshakeError::BadFrame,
        _ => HandshakeError::Io(e),
    }
}

fn send<S: Write>(stream: &mut S, msg: &ClientToServer) -> Result<(), HandshakeError> {
    write_msg(stream, msg).map_err(map_io)?;
    stream.flush().map_err(map_io)
}

fn reply<S: Read>(stream: &mut S) -> Result<ServerToClient, HandshakeError> {
    read_msg(stream).map_err(map_io)
}

/// Run the full client-side join handshake over `stream`. On `Ok` the stream
/// is positioned exactly after `JoinAccept` — hand it to
/// [`super::connection::TcpClientConn::spawn`] with
/// `IdRemap::build(&join.tables)`. On ANY `Err` the caller drops the stream
/// (in particular for [`HandshakeError::MissingMods`]: no farewell frame).
pub(crate) fn client_handshake<S: Read + Write>(
    stream: &mut S,
    player_name: &str,
    installed_mod_ids: &BTreeSet<String>,
) -> Result<Box<JoinData>, HandshakeError> {
    send(
        stream,
        &ClientToServer::Hello {
            protocol: PROTOCOL_VERSION,
        },
    )?;
    match reply(stream)? {
        ServerToClient::HelloAck { .. } => {}
        ServerToClient::HelloReject { server_protocol } => {
            return Err(HandshakeError::ProtocolMismatch {
                server: server_protocol,
            })
        }
        _ => return Err(HandshakeError::BadFrame),
    }

    send(stream, &ClientToServer::ModQuery)?;
    let mods = match reply(stream)? {
        ServerToClient::ModList { mods } => mods,
        _ => return Err(HandshakeError::BadFrame),
    };
    let missing: Vec<ModEntry> = mods
        .into_iter()
        .filter(|m| !installed_mod_ids.contains(&m.id))
        .collect();
    if !missing.is_empty() {
        return Err(HandshakeError::MissingMods(missing));
    }

    send(
        stream,
        &ClientToServer::Join {
            player_name: player_name.to_string(),
        },
    )?;
    match reply(stream)? {
        ServerToClient::JoinAccept(data) => Ok(data),
        ServerToClient::JoinReject { reason } => Err(HandshakeError::Rejected(reason)),
        _ => Err(HandshakeError::BadFrame),
    }
}

/// The ids of every INSTALLED id-bearing pack — what this client can satisfy,
/// regardless of any per-world disables (those are the server's concern).
pub(crate) fn installed_mod_ids() -> BTreeSet<String> {
    crate::modding::modset::active(&BTreeSet::new())
        .into_iter()
        .map(|m| m.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::protocol::{NameTables, SelfRestore};
    use crate::server::player::PlayerId;

    /// A scripted in-memory duplex: the pre-baked server replies are read in
    /// order; everything the client writes is captured for exact-sequence
    /// asserts. EOF past the script = the server closed the connection.
    struct Scripted {
        replies: io::Cursor<Vec<u8>>,
        sent: Vec<u8>,
    }

    impl Scripted {
        fn new(replies: &[ServerToClient]) -> Scripted {
            let mut buf = Vec::new();
            for msg in replies {
                write_msg(&mut buf, msg).expect("script encodes");
            }
            Scripted {
                replies: io::Cursor::new(buf),
                sent: Vec::new(),
            }
        }

        fn sent_msgs(&self) -> Vec<ClientToServer> {
            let mut r = &self.sent[..];
            let mut out = Vec::new();
            while !r.is_empty() {
                out.push(read_msg(&mut r).expect("client frames decode"));
            }
            out
        }
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.replies.read(buf)
        }
    }

    impl Write for Scripted {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.sent.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn join_data() -> Box<JoinData> {
        Box::new(JoinData {
            player_id: PlayerId(3),
            seed: 9,
            clock: 6000,
            tables: NameTables::default(),
            self_restore: SelfRestore {
                pos: crate::mathh::Vec3::new(1.0, 70.0, 2.0),
                vel: crate::mathh::Vec3::ZERO,
                yaw: 0.5,
                pitch: -0.25,
                mode: 0,
                health: 18,
                bed_spawn: None,
                effects: Vec::new(),
                inventory: Vec::new(),
                active_slot: 2,
            },
            players: vec![(PlayerId(0), "Host".to_string())],
        })
    }

    fn mods(ids: &[&str]) -> Vec<ModEntry> {
        ids.iter()
            .map(|id| ModEntry {
                id: id.to_string(),
                version: "1.0".to_string(),
            })
            .collect()
    }

    fn installed(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn happy_path_sends_exactly_hello_modquery_join_in_order() {
        let mut s = Scripted::new(&[
            ServerToClient::HelloAck {
                protocol: PROTOCOL_VERSION,
            },
            ServerToClient::ModList {
                mods: mods(&["kitchen"]),
            },
            ServerToClient::JoinAccept(join_data()),
        ]);
        let data = client_handshake(&mut s, "Rachel", &installed(&["kitchen", "extra"]))
            .expect("handshake succeeds");
        assert_eq!(*data, *join_data());
        assert_eq!(
            s.sent_msgs(),
            vec![
                ClientToServer::Hello {
                    protocol: PROTOCOL_VERSION
                },
                ClientToServer::ModQuery,
                ClientToServer::Join {
                    player_name: "Rachel".to_string()
                },
            ],
            "the exact WIKI frame sequence, nothing more"
        );
    }

    #[test]
    fn a_protocol_mismatch_stops_after_hello() {
        let mut s = Scripted::new(&[ServerToClient::HelloReject { server_protocol: 3 }]);
        match client_handshake(&mut s, "Rachel", &installed(&[])) {
            Err(HandshakeError::ProtocolMismatch { server: 3 }) => {}
            other => panic!("expected ProtocolMismatch, got {other:?}"),
        }
        assert_eq!(
            s.sent_msgs(),
            vec![ClientToServer::Hello {
                protocol: PROTOCOL_VERSION
            }]
        );
    }

    #[test]
    fn missing_mods_close_the_connection_before_any_join_frame() {
        let mut s = Scripted::new(&[
            ServerToClient::HelloAck {
                protocol: PROTOCOL_VERSION,
            },
            ServerToClient::ModList {
                mods: mods(&["kitchen", "ghost_mod"]),
            },
        ]);
        match client_handshake(&mut s, "Rachel", &installed(&["kitchen"])) {
            Err(HandshakeError::MissingMods(missing)) => {
                assert_eq!(missing, mods(&["ghost_mod"]));
            }
            other => panic!("expected MissingMods, got {other:?}"),
        }
        assert_eq!(
            s.sent_msgs(),
            vec![
                ClientToServer::Hello {
                    protocol: PROTOCOL_VERSION
                },
                ClientToServer::ModQuery,
            ],
            "no Join (and no farewell) frame follows a mod refusal"
        );
    }

    #[test]
    fn a_join_reject_surfaces_the_reason() {
        let mut s = Scripted::new(&[
            ServerToClient::HelloAck {
                protocol: PROTOCOL_VERSION,
            },
            ServerToClient::ModList { mods: Vec::new() },
            ServerToClient::JoinReject {
                reason: JoinRejectReason::NameTaken,
            },
        ]);
        match client_handshake(&mut s, "Rachel", &installed(&[])) {
            Err(HandshakeError::Rejected(JoinRejectReason::NameTaken)) => {}
            other => panic!("expected Rejected(NameTaken), got {other:?}"),
        }
    }

    #[test]
    fn a_server_that_hangs_up_mid_handshake_reads_as_closed_not_a_panic() {
        let mut s = Scripted::new(&[ServerToClient::HelloAck {
            protocol: PROTOCOL_VERSION,
        }]);
        match client_handshake(&mut s, "Rachel", &installed(&[])) {
            Err(HandshakeError::Closed) => {}
            other => panic!("expected Closed, got {other:?}"),
        }
    }

    #[test]
    fn an_out_of_sequence_reply_is_a_bad_frame() {
        // A server answering Hello with a gameplay message is broken.
        let mut s = Scripted::new(&[ServerToClient::KeepAlive]);
        match client_handshake(&mut s, "Rachel", &installed(&[])) {
            Err(HandshakeError::BadFrame) => {}
            other => panic!("expected BadFrame, got {other:?}"),
        }
    }
}
