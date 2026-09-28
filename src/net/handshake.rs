//! The client's join handshake, pure over any
//! `Read + Write` stream so it unit-tests over an in-memory transcript.
//!
//! Exact sequence:
//! `Hello{protocol}` → `HelloAck{challenge, requires_account, server_id, key_share}` (or
//! `HelloReject` = protocol mismatch) → `KeyExchange{key_share}`. Every frame after that is
//! sealed with the session keys (`net::secure`): `ModQuery` → `ModList{mods}` → compare ids
//! against the installed packs (missing = CLOSE the socket, no farewell frame — the caller drops
//! the stream) → `Join{credential, key, proof, view_distance, cached_sections}`, `proof` being
//! the identity key's signature over the exchange's binding (`net::identity`) → `JoinAccept(JoinData)`
//! (or `JoinReject`).
//!
//! The credential is resolved by a CALLBACK, after the key exchange: only then does
//! the client know whether this server wants a Petramond account ticket or a
//! plain name, and minting a ticket is itself a blocking network call that must
//! not happen for a server that did not ask for one. The ticket is minted for the
//! binding-derived id in [`ServerOffer::server_id`], never for the server's own id.
//!
//! The function is I/O-agnostic: the caller sets per-read deadlines on the
//! raw `TcpStream` (`set_read_timeout`, ~5 s) before calling; timeouts
//! surface as [`HandshakeError::Timeout`]. Reads never over-read a frame, so
//! the stream hands off cleanly to the connection threads afterwards, together
//! with the [`Channel`] that keeps sealing and opening its frames.

use std::collections::BTreeSet;
use std::io::{self, Read, Write};

use super::framing::{read_msg, read_opened, write_msg, write_sealed, MAX_FRAME};
use super::identity::PlayerIdentity;
use super::protocol::{
    ClientToServer, JoinCredential, JoinData, JoinRejectReason, ModEntry, SectionCacheClaim,
    ServerToClient,
};
use super::secure::{Channel, Ephemeral, Exchange, Role};
use super::PROTOCOL_VERSION;

#[derive(Debug)]
pub enum HandshakeError {
    Io(io::Error),
    Timeout,
    ProtocolMismatch {
        server: u16,
    },
    MissingMods(Vec<ModEntry>),
    Rejected(JoinRejectReason),
    Credential {
        message: String,
        sign_in_required: bool,
    },
    Closed,
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
            HandshakeError::Rejected(JoinRejectReason::BadProof) => {
                write!(f, "The server could not verify your player identity")
            }
            HandshakeError::Rejected(JoinRejectReason::InvalidName(why)) => write!(f, "{why}"),
            HandshakeError::Rejected(JoinRejectReason::AlreadyConnected) => {
                write!(f, "You are already connected to this server")
            }
            HandshakeError::Rejected(JoinRejectReason::ServerFull) => {
                write!(f, "The server is full")
            }
            HandshakeError::Rejected(JoinRejectReason::AccountRequired) => {
                write!(f, "This server requires a Petramond account")
            }
            HandshakeError::Rejected(JoinRejectReason::AccountNotAccepted) => {
                write!(f, "This server does not use Petramond accounts")
            }
            HandshakeError::Rejected(JoinRejectReason::AccountAlreadyOnline) => {
                write!(
                    f,
                    "Your Petramond account is already playing on this server"
                )
            }
            HandshakeError::Rejected(JoinRejectReason::AccountRejected(why)) => write!(f, "{why}"),
            HandshakeError::Rejected(JoinRejectReason::AccountUnavailable(why)) => {
                write!(f, "{why}")
            }
            HandshakeError::Credential { message, .. } => write!(f, "{message}"),
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

fn send_sealed<S: Write>(
    stream: &mut S,
    msg: &ClientToServer,
    channel: &mut Channel,
) -> Result<(), HandshakeError> {
    write_sealed(stream, msg, &mut channel.sealer).map_err(map_io)?;
    stream.flush().map_err(map_io)
}

fn reply<S: Read>(
    stream: &mut S,
    mut channel: Option<&mut Channel>,
) -> Result<ServerToClient, HandshakeError> {
    loop {
        let msg = match channel.as_deref_mut() {
            Some(channel) => read_opened(stream, MAX_FRAME, &mut channel.opener).map(|(m, _)| m),
            None => read_msg(stream),
        };
        match msg.map_err(map_io)? {
            // The server's writer keepalives after 2 s of outbound silence —
            // during the handshake too (admitting a join can take seconds on
            // a busy host: the spawn find runs worldgen). Liveness only,
            // never a reply; the server skips client keepalives the same way
            // (`step_pending`).
            ServerToClient::KeepAlive => continue,
            msg => return Ok(msg),
        }
    }
}

#[derive(Debug)]
pub struct HandshakeJoin {
    pub join: Box<JoinData>,
    pub server_mods: BTreeSet<String>,
}

pub struct ServerOffer<'a> {
    pub requires_account: bool,
    /// The id an account ticket is minted for: this connection's key-exchange binding, so a
    /// ticket only ever redeems on the server this client actually exchanged keys with.
    pub server_id: &'a str,
}

pub fn client_handshake<S: Read + Write>(
    stream: &mut S,
    identity: &PlayerIdentity,
    credential: impl FnOnce(&ServerOffer) -> Result<JoinCredential, HandshakeError>,
    view_distance: i32,
    installed_mod_ids: &BTreeSet<String>,
    cached_sections: Vec<SectionCacheClaim>,
) -> Result<(HandshakeJoin, Channel), HandshakeError> {
    let ours = Ephemeral::generate().map_err(HandshakeError::Io)?;
    send(
        stream,
        &ClientToServer::Hello {
            protocol: PROTOCOL_VERSION,
        },
    )?;
    let (challenge, requires_account, server_id, server_share) = match reply(stream, None)? {
        ServerToClient::HelloAck {
            challenge,
            requires_account,
            server_id,
            key_share,
            ..
        } => (challenge, requires_account, server_id, key_share),
        ServerToClient::HelloReject { server_protocol } => {
            return Err(HandshakeError::ProtocolMismatch {
                server: server_protocol,
            })
        }
        _ => return Err(HandshakeError::BadFrame),
    };

    let client_share = ours.share();
    let session = ours
        .agree(
            Role::Client,
            &Exchange {
                server_id: &server_id,
                challenge: &challenge,
                client_share: &client_share,
                server_share: &server_share,
            },
        )
        .map_err(|_| HandshakeError::BadFrame)?;
    let mut channel = session.channel(Role::Client);
    send(
        stream,
        &ClientToServer::KeyExchange {
            key_share: client_share,
        },
    )?;

    send_sealed(stream, &ClientToServer::ModQuery, &mut channel)?;
    let mods = match reply(stream, Some(&mut channel))? {
        ServerToClient::ModList { mods } => mods,
        _ => return Err(HandshakeError::BadFrame),
    };
    let missing: Vec<ModEntry> = mods
        .iter()
        .filter(|m| !installed_mod_ids.contains(&m.id))
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(HandshakeError::MissingMods(missing));
    }
    let server_mods: BTreeSet<String> = mods.into_iter().map(|m| m.id).collect();

    let credential = credential(&ServerOffer {
        requires_account,
        server_id: &session.ticket_server_id(),
    })?;
    send_sealed(
        stream,
        &ClientToServer::Join {
            credential,
            key: identity.key(),
            proof: identity.sign_join(session.binding()),
            view_distance: view_distance.clamp(4, 64) as u8,
            cached_sections,
        },
        &mut channel,
    )?;
    match reply(stream, Some(&mut channel))? {
        ServerToClient::JoinAccept(join) => Ok((HandshakeJoin { join, server_mods }, channel)),
        ServerToClient::JoinReject { reason } => Err(HandshakeError::Rejected(reason)),
        _ => Err(HandshakeError::BadFrame),
    }
}

pub fn installed_mod_ids() -> BTreeSet<String> {
    crate::modding::modset::active(&BTreeSet::new())
        .into_iter()
        .map(|m| m.id)
        .collect()
}

/// A client that runs `Hello` and the key exchange by hand and then stops, so server tests can
/// send a `Join` of their own making over the keyed connection.
#[cfg(test)]
pub(crate) struct ManualClient {
    pub challenge: super::identity::JoinChallenge,
    pub requires_account: bool,
    pub server_id: String,
    pub session: super::secure::Session,
    channel: Channel,
}

#[cfg(test)]
impl ManualClient {
    pub(crate) fn hello<S: Read + Write>(stream: &mut S) -> ManualClient {
        let ours = Ephemeral::generate().expect("os randomness");
        send(
            stream,
            &ClientToServer::Hello {
                protocol: PROTOCOL_VERSION,
            },
        )
        .expect("send hello");
        let (challenge, requires_account, server_id, server_share) =
            match reply(stream, None).expect("a reply") {
                ServerToClient::HelloAck {
                    challenge,
                    requires_account,
                    server_id,
                    key_share,
                    ..
                } => (challenge, requires_account, server_id, key_share),
                other => panic!("expected HelloAck, got {other:?}"),
            };
        let client_share = ours.share();
        let session = ours
            .agree(
                Role::Client,
                &Exchange {
                    server_id: &server_id,
                    challenge: &challenge,
                    client_share: &client_share,
                    server_share: &server_share,
                },
            )
            .expect("keys agree");
        send(
            stream,
            &ClientToServer::KeyExchange {
                key_share: client_share,
            },
        )
        .expect("send key share");
        let channel = session.channel(Role::Client);
        ManualClient {
            challenge,
            requires_account,
            server_id,
            session,
            channel,
        }
    }

    pub(crate) fn send<S: Write>(&mut self, stream: &mut S, msg: &ClientToServer) {
        send_sealed(stream, msg, &mut self.channel).expect("send sealed");
    }

    /// The next reply that is not a keepalive.
    pub(crate) fn recv<S: Read>(&mut self, stream: &mut S) -> ServerToClient {
        reply(stream, Some(&mut self.channel)).expect("a sealed reply")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::framing::{encode_frame, encode_sealed};
    use crate::net::identity::JoinChallenge;
    use crate::net::protocol::{NameTables, SelfRestore};
    use crate::net::secure::{Binding, KeyShare, Session};
    use crate::player::PlayerId;
    use petramond_math::world_pos::WorldPos;

    /// A scripted server. Replies go out in order, in the clear up to and including the
    /// `HelloAck` (which gets this server's key share filled in), then sealed with the keys it
    /// agrees from the client's `KeyExchange` — just as a real server would.
    struct Scripted {
        script: std::collections::VecDeque<ServerToClient>,
        out: Vec<u8>,
        read_at: usize,
        sent: Vec<u8>,
        ephemeral: Option<Ephemeral>,
        acked: Option<(JoinChallenge, String)>,
        session: Option<Session>,
        channel: Option<Channel>,
        seal_after_ack: bool,
    }

    impl Scripted {
        fn new(replies: &[ServerToClient]) -> Scripted {
            Scripted {
                script: replies.iter().cloned().collect(),
                out: Vec::new(),
                read_at: 0,
                sent: Vec::new(),
                ephemeral: Some(Ephemeral::generate().expect("os randomness")),
                acked: None,
                session: None,
                channel: None,
                seal_after_ack: true,
            }
        }

        fn client_share(&self) -> KeyShare {
            let mut r = &self.sent[..];
            loop {
                match read_msg::<ClientToServer, _>(&mut r) {
                    Ok(ClientToServer::KeyExchange { key_share }) => return key_share,
                    Ok(_) => {}
                    Err(e) => panic!("the client never sent its key share: {e}"),
                }
            }
        }

        fn keyed(&mut self) -> &mut Channel {
            if self.channel.is_none() {
                let client_share = self.client_share();
                let (challenge, server_id) = self.acked.clone().expect("acked before keying");
                let ours = self.ephemeral.take().expect("one exchange");
                let server_share = ours.share();
                let session = ours
                    .agree(
                        Role::Server,
                        &Exchange {
                            server_id: &server_id,
                            challenge: &challenge,
                            client_share: &client_share,
                            server_share: &server_share,
                        },
                    )
                    .expect("keys agree");
                self.channel = Some(session.channel(Role::Server));
                self.session = Some(session);
            }
            self.channel.as_mut().expect("keyed above")
        }

        fn binding(&self) -> Binding {
            *self.session.as_ref().expect("keyed").binding()
        }

        fn next_reply(&mut self) -> Option<Vec<u8>> {
            let mut msg = self.script.pop_front()?;
            if self.acked.is_some() && self.seal_after_ack {
                return Some(encode_sealed(&msg, &mut self.keyed().sealer).expect("seals"));
            }
            if let ServerToClient::HelloAck {
                challenge,
                server_id,
                key_share,
                ..
            } = &mut msg
            {
                *key_share = self.ephemeral.as_ref().expect("unkeyed").share();
                self.acked = Some((*challenge, server_id.clone()));
            }
            Some(encode_frame(&msg).expect("script encodes"))
        }

        fn sent_msgs(&self) -> Vec<ClientToServer> {
            let mut r = &self.sent[..];
            let mut out = Vec::new();
            while !r.is_empty() && !matches!(out.last(), Some(ClientToServer::KeyExchange { .. })) {
                out.push(read_msg(&mut r).expect("client frames decode"));
            }
            if let Some(session) = &self.session {
                let mut opener = session.channel(Role::Server).opener;
                while !r.is_empty() {
                    let (msg, _) =
                        read_opened(&mut r, MAX_FRAME, &mut opener).expect("sealed frames open");
                    out.push(msg);
                }
            }
            assert!(r.is_empty(), "every sent byte belongs to a frame");
            out
        }
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.read_at == self.out.len() {
                let Some(frame) = self.next_reply() else {
                    return Ok(0);
                };
                self.out = frame;
                self.read_at = 0;
            }
            let n = buf.len().min(self.out.len() - self.read_at);
            buf[..n].copy_from_slice(&self.out[self.read_at..self.read_at + n]);
            self.read_at += n;
            Ok(n)
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
            player_name: "Joiner".into(),
            seed: 9,
            clock: 6000,
            tables: NameTables::default(),
            self_restore: SelfRestore {
                transform: crate::net::protocol::Transform {
                    pos: WorldPos::new(1.0, 70.0, 2.0),
                    vel: petramond_math::math::Vec3::ZERO,
                    yaw: 0.5,
                    pitch: -0.25,
                },
                mode: 0,
                health: 18,
                bed_spawn: None,
                effects: Vec::new(),
                inventory: Vec::new(),
                active_slot: 2,
                craft_craftable_only: false,
                unlocked_recipes: Vec::new(),
            },
            crafting_recipes: Vec::new(),
            players: vec![(PlayerId(0), "Host".to_string())],
            client_policy: Default::default(),
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

    const CHALLENGE: [u8; 32] = [0x42; 32];

    fn identity() -> PlayerIdentity {
        PlayerIdentity::generate().expect("os randomness")
    }

    fn ack() -> ServerToClient {
        ServerToClient::HelloAck {
            protocol: PROTOCOL_VERSION,
            challenge: CHALLENGE,
            requires_account: false,
            server_id: "test-server".to_string(),
            key_share: [0; 32],
        }
    }

    fn offers_name(_: &ServerOffer) -> Result<JoinCredential, HandshakeError> {
        Ok(JoinCredential::Name("Rachel".to_string()))
    }

    #[test]
    fn happy_path_sends_exactly_hello_modquery_join_in_order() {
        let mut s = Scripted::new(&[
            ack(),
            ServerToClient::ModList {
                mods: mods(&["kitchen"]),
            },
            ServerToClient::JoinAccept(join_data()),
        ]);
        let me = identity();
        let (data, _) = client_handshake(
            &mut s,
            &me,
            offers_name,
            16,
            &installed(&["kitchen", "extra"]),
            Vec::new(),
        )
        .expect("handshake succeeds");
        assert_eq!(*data.join, *join_data());
        assert_eq!(
            data.server_mods,
            installed(&["kitchen"]),
            "the ModList rides out as the session's client-mod enablement set \
             (the locally installed 'extra' is NOT in it)"
        );
        let sent = s.sent_msgs();
        let proof = match &sent[..] {
            [ClientToServer::Hello {
                protocol: PROTOCOL_VERSION,
            }, ClientToServer::KeyExchange { .. }, ClientToServer::ModQuery, ClientToServer::Join {
                credential: JoinCredential::Name(name),
                key,
                proof,
                view_distance: 16,
                cached_sections,
            }] if name == "Rachel" && *key == me.key() && cached_sections.is_empty() => proof,
            other => panic!("the exact frame sequence, nothing more; got {other:?}"),
        };
        assert!(
            crate::net::identity::verify_join(&s.binding(), &me.key(), proof),
            "the Join proves the identity against this connection's key exchange"
        );
        assert!(
            !crate::net::identity::verify_join(&CHALLENGE, &me.key(), proof),
            "the bare challenge alone does not verify: a relay cannot reuse the proof"
        );
    }

    #[test]
    fn a_protocol_mismatch_stops_after_hello() {
        let mut s = Scripted::new(&[ServerToClient::HelloReject { server_protocol: 3 }]);
        match client_handshake(
            &mut s,
            &identity(),
            offers_name,
            16,
            &installed(&[]),
            Vec::new(),
        ) {
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
            ack(),
            ServerToClient::ModList {
                mods: mods(&["kitchen", "ghost_mod"]),
            },
        ]);
        match client_handshake(
            &mut s,
            &identity(),
            offers_name,
            16,
            &installed(&["kitchen"]),
            Vec::new(),
        ) {
            Err(HandshakeError::MissingMods(missing)) => {
                assert_eq!(missing, mods(&["ghost_mod"]));
            }
            other => panic!("expected MissingMods, got {other:?}"),
        }
        let sent = s.sent_msgs();
        assert!(
            matches!(
                &sent[..],
                [
                    ClientToServer::Hello {
                        protocol: PROTOCOL_VERSION
                    },
                    ClientToServer::KeyExchange { .. },
                    ClientToServer::ModQuery,
                ]
            ),
            "no Join (and no farewell) frame follows a mod refusal; got {sent:?}"
        );
    }

    #[test]
    fn a_join_reject_surfaces_the_reason() {
        let mut s = Scripted::new(&[
            ack(),
            ServerToClient::ModList { mods: Vec::new() },
            ServerToClient::JoinReject {
                reason: JoinRejectReason::BadProof,
            },
        ]);
        match client_handshake(
            &mut s,
            &identity(),
            offers_name,
            16,
            &installed(&[]),
            Vec::new(),
        ) {
            Err(HandshakeError::Rejected(JoinRejectReason::BadProof)) => {}
            other => panic!("expected Rejected(BadProof), got {other:?}"),
        }
    }

    #[test]
    fn a_server_that_hangs_up_mid_handshake_reads_as_closed_not_a_panic() {
        let mut s = Scripted::new(&[ack()]);
        match client_handshake(
            &mut s,
            &identity(),
            offers_name,
            16,
            &installed(&[]),
            Vec::new(),
        ) {
            Err(HandshakeError::Closed) => {}
            other => panic!("expected Closed, got {other:?}"),
        }
    }

    #[test]
    fn an_out_of_sequence_reply_is_a_bad_frame() {
        let mut s = Scripted::new(&[ServerToClient::ServerClosing]);
        match client_handshake(
            &mut s,
            &identity(),
            offers_name,
            16,
            &installed(&[]),
            Vec::new(),
        ) {
            Err(HandshakeError::BadFrame) => {}
            other => panic!("expected BadFrame, got {other:?}"),
        }
    }

    #[test]
    fn keepalives_between_replies_are_skipped_not_bad_frames() {
        let mut s = Scripted::new(&[
            ServerToClient::KeepAlive,
            ack(),
            ServerToClient::KeepAlive,
            ServerToClient::KeepAlive,
            ServerToClient::ModList { mods: Vec::new() },
            ServerToClient::KeepAlive,
            ServerToClient::JoinAccept(join_data()),
        ]);
        let (data, _) = client_handshake(
            &mut s,
            &identity(),
            offers_name,
            16,
            &installed(&[]),
            Vec::new(),
        )
        .expect("keepalive-interleaved handshake succeeds");
        assert_eq!(*data.join, *join_data());
    }

    #[test]
    fn an_unsealed_reply_after_the_key_exchange_is_a_bad_frame() {
        let mut s = Scripted::new(&[ack(), ServerToClient::ModList { mods: Vec::new() }]);
        s.seal_after_ack = false;
        match client_handshake(
            &mut s,
            &identity(),
            offers_name,
            16,
            &installed(&[]),
            Vec::new(),
        ) {
            Err(HandshakeError::BadFrame) => {}
            other => panic!("expected BadFrame, got {other:?}"),
        }
    }

    #[test]
    fn a_ticket_is_minted_for_the_binding_not_the_announced_server_id() {
        let mut s = Scripted::new(&[
            ack(),
            ServerToClient::ModList { mods: Vec::new() },
            ServerToClient::JoinAccept(join_data()),
        ]);
        let mut offered = None;
        client_handshake(
            &mut s,
            &identity(),
            |offer: &ServerOffer| {
                offered = Some(offer.server_id.to_string());
                Ok(JoinCredential::Ticket("ticket".into()))
            },
            16,
            &installed(&[]),
            Vec::new(),
        )
        .expect("handshake succeeds");
        let offered = offered.expect("the credential was asked for");
        assert_ne!(offered, "test-server");
        assert_eq!(
            offered,
            s.session.as_ref().unwrap().ticket_server_id(),
            "both ends name the ticket the same way"
        );
    }
}
