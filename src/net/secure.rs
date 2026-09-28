//! Per-connection keys, agreed during the join handshake.
//!
//! The server's `HelloAck` and the client's `KeyExchange` each carry an ephemeral X25519 share.
//! The shared secret keys one ChaCha20-Poly1305 sealer per direction for every later frame. A
//! binding hash over the whole exchange (server id, challenge and both shares) is what the join
//! proof signs and what an account ticket is minted for.
//!
//! This is what stops a relay. A server that runs its own exchange with each side gets a
//! different binding on each leg, so the server it relays to rejects the proof or the ticket. A
//! relay that forwards the shares untouched never learns the keys, so it cannot read, inject or
//! take over the session's frames.

use std::fmt;
use std::io;

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use ring::rand::SystemRandom;

use super::identity::JoinChallenge;

pub type KeyShare = [u8; 32];

pub type Binding = [u8; 32];

pub const TAG_LEN: usize = 16;

const BINDING_CONTEXT: &str = "petramond 2026-09-28 join binding v1";
const CLIENT_KEY_CONTEXT: &str = "petramond 2026-09-28 client-to-server frames v1";
const SERVER_KEY_CONTEXT: &str = "petramond 2026-09-28 server-to-client frames v1";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Role {
    Client,
    Server,
}

pub struct Ephemeral {
    private: EphemeralPrivateKey,
    share: KeyShare,
}

impl Ephemeral {
    pub fn generate() -> io::Result<Ephemeral> {
        let no_randomness = || io::Error::other("no OS randomness for a key share");
        let private = EphemeralPrivateKey::generate(&X25519, &SystemRandom::new())
            .map_err(|_| no_randomness())?;
        let public = private.compute_public_key().map_err(|_| no_randomness())?;
        let share = public.as_ref().try_into().map_err(|_| no_randomness())?;
        Ok(Ephemeral { private, share })
    }

    pub fn share(&self) -> KeyShare {
        self.share
    }

    /// Fails when the peer's share is not a usable X25519 point.
    pub fn agree(self, role: Role, exchange: &Exchange) -> io::Result<Session> {
        let (ours, theirs) = match role {
            Role::Client => (exchange.client_share, exchange.server_share),
            Role::Server => (exchange.server_share, exchange.client_share),
        };
        if *ours != self.share {
            return Err(io::Error::other(
                "the exchange names another key share as ours",
            ));
        }
        agree_ephemeral(
            self.private,
            &UnparsedPublicKey::new(&X25519, theirs),
            |secret| Session::derive(secret, exchange),
        )
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "unusable peer key share"))
    }
}

/// Everything both sides saw during the exchange, in the order the binding hashes it.
pub struct Exchange<'a> {
    pub server_id: &'a str,
    pub challenge: &'a JoinChallenge,
    pub client_share: &'a KeyShare,
    pub server_share: &'a KeyShare,
}

pub struct Session {
    binding: Binding,
    client_key: [u8; 32],
    server_key: [u8; 32],
}

impl Session {
    fn derive(secret: &[u8], exchange: &Exchange) -> Session {
        let mut h = blake3::Hasher::new_derive_key(BINDING_CONTEXT);
        h.update(&(exchange.server_id.len() as u64).to_le_bytes());
        h.update(exchange.server_id.as_bytes());
        h.update(exchange.challenge);
        h.update(exchange.client_share);
        h.update(exchange.server_share);
        h.update(secret);
        let binding = *h.finalize().as_bytes();
        let key = |context| {
            let mut h = blake3::Hasher::new_derive_key(context);
            h.update(secret);
            h.update(&binding);
            *h.finalize().as_bytes()
        };
        Session {
            binding,
            client_key: key(CLIENT_KEY_CONTEXT),
            server_key: key(SERVER_KEY_CONTEXT),
        }
    }

    pub fn binding(&self) -> &Binding {
        &self.binding
    }

    /// The server id an account ticket is minted for and verified against: this connection's
    /// binding, in the same 32-hex-digit form as the server's own id.
    pub fn ticket_server_id(&self) -> String {
        self.binding[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    pub fn channel(&self, role: Role) -> Channel {
        let (send, receive) = match role {
            Role::Client => (&self.client_key, &self.server_key),
            Role::Server => (&self.server_key, &self.client_key),
        };
        Channel {
            sealer: FrameSealer(Counter::new(send)),
            opener: FrameOpener(Counter::new(receive)),
        }
    }
}

/// The two directions of one connection's frame protection.
pub struct Channel {
    pub sealer: FrameSealer,
    pub opener: FrameOpener,
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Channel { .. }")
    }
}

struct Counter {
    key: LessSafeKey,
    next: u64,
}

impl Counter {
    fn new(key: &[u8; 32]) -> Counter {
        Counter {
            key: LessSafeKey::new(
                UnboundKey::new(&CHACHA20_POLY1305, key).expect("a 32-byte ChaCha20 key"),
            ),
            next: 0,
        }
    }

    fn nonce(&mut self) -> io::Result<Nonce> {
        let n = self.next;
        self.next = n
            .checked_add(1)
            .ok_or_else(|| io::Error::other("frame counter exhausted"))?;
        let mut bytes = [0u8; 12];
        bytes[4..].copy_from_slice(&n.to_le_bytes());
        Ok(Nonce::assume_unique_for_key(bytes))
    }
}

/// Seals outbound frames in order: each takes the next nonce, and the header is authenticated.
pub struct FrameSealer(Counter);

impl FrameSealer {
    pub fn seal(&mut self, header: &[u8], body: &mut Vec<u8>) -> io::Result<()> {
        let nonce = self.0.nonce()?;
        self.0
            .key
            .seal_in_place_append_tag(nonce, Aad::from(header), body)
            .map_err(|_| io::Error::other("seal frame"))
    }
}

/// Opens inbound frames in the order they were sealed. A replayed, reordered, altered or forged
/// frame fails, and the connection is dropped.
pub struct FrameOpener(Counter);

impl FrameOpener {
    /// Returns the plaintext's length; it sits at the front of `body`.
    pub fn open(&mut self, header: &[u8], body: &mut [u8]) -> io::Result<usize> {
        let nonce = self.0.nonce()?;
        self.0
            .key
            .open_in_place(nonce, Aad::from(header), body)
            .map(|plain| plain.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame failed authentication"))
    }
}

#[cfg(test)]
pub(crate) fn channels_for_test() -> (Channel, Channel) {
    let (client, server) = (
        Ephemeral::generate().unwrap(),
        Ephemeral::generate().unwrap(),
    );
    let (client_share, server_share) = (client.share(), server.share());
    let exchange = Exchange {
        server_id: "test-server",
        challenge: &[7; 32],
        client_share: &client_share,
        server_share: &server_share,
    };
    let c = client.agree(Role::Client, &exchange).unwrap();
    let s = server.agree(Role::Server, &exchange).unwrap();
    (c.channel(Role::Client), s.channel(Role::Server))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exchange<'a>(
        server_id: &'a str,
        client: &'a KeyShare,
        server: &'a KeyShare,
    ) -> Exchange<'a> {
        Exchange {
            server_id,
            challenge: &[9; 32],
            client_share: client,
            server_share: server,
        }
    }

    #[test]
    fn both_sides_agree_and_frames_open_in_order() {
        let (client, server) = (
            Ephemeral::generate().unwrap(),
            Ephemeral::generate().unwrap(),
        );
        let (cs, ss) = (client.share(), server.share());
        let c = client
            .agree(Role::Client, &exchange("id", &cs, &ss))
            .unwrap();
        let s = server
            .agree(Role::Server, &exchange("id", &cs, &ss))
            .unwrap();
        assert_eq!(c.binding(), s.binding());
        assert_eq!(c.ticket_server_id(), s.ticket_server_id());
        assert_eq!(c.ticket_server_id().len(), 32);

        let mut to_server = c.channel(Role::Client).sealer;
        let mut at_server = s.channel(Role::Server).opener;
        for n in 0..3u8 {
            let mut body = vec![n; 10];
            to_server.seal(b"hdr", &mut body).unwrap();
            assert_eq!(body.len(), 10 + TAG_LEN);
            let len = at_server.open(b"hdr", &mut body).unwrap();
            assert_eq!(&body[..len], &[n; 10]);
        }
    }

    #[test]
    fn a_relay_that_swaps_shares_gets_a_different_binding_on_each_leg() {
        let (client, relay_to_client) = (
            Ephemeral::generate().unwrap(),
            Ephemeral::generate().unwrap(),
        );
        let (relay_to_server, server) = (
            Ephemeral::generate().unwrap(),
            Ephemeral::generate().unwrap(),
        );
        let (c, rc) = (client.share(), relay_to_client.share());
        let (rs, s) = (relay_to_server.share(), server.share());
        let client_leg = client
            .agree(Role::Client, &exchange("victim", &c, &rc))
            .unwrap();
        let server_leg = server
            .agree(Role::Server, &exchange("victim", &rs, &s))
            .unwrap();
        assert_ne!(client_leg.binding(), server_leg.binding());
        assert_ne!(client_leg.ticket_server_id(), server_leg.ticket_server_id());
    }

    #[test]
    fn altered_replayed_or_misdirected_frames_fail_to_open() {
        let (mut client, mut server) = channels_for_test();
        let mut sealed = b"hello".to_vec();
        client.sealer.seal(b"h", &mut sealed).unwrap();

        let mut altered = sealed.clone();
        altered[0] ^= 1;
        assert!(server.opener.open(b"h", &mut altered).is_err());

        let (_, mut fresh) = channels_for_test();
        assert!(
            fresh.opener.open(b"h", &mut sealed.clone()).is_err(),
            "another session's key"
        );

        let (mut client, mut server) = channels_for_test();
        let mut first = b"one".to_vec();
        client.sealer.seal(b"h", &mut first).unwrap();
        let replay = first.clone();
        server.opener.open(b"h", &mut first).unwrap();
        assert!(
            server.opener.open(b"h", &mut replay.clone()).is_err(),
            "a replay"
        );

        let mut own = b"echo".to_vec();
        server.sealer.seal(b"h", &mut own).unwrap();
        assert!(
            server.opener.open(b"h", &mut own).is_err(),
            "a reflected frame"
        );
    }
}
