use super::admission::{MAX_PENDING_PER_IP, PRE_JOIN_MAX_FRAME};
use super::tests::{headless, joins_as};
use super::*;
use crate::net::handshake::{client_handshake, installed_mod_ids, HandshakeError, ManualClient};
use crate::net::identity::PlayerIdentity;
use petramond_util::test_time::TEST_HARD_DEADLINE;
use petramond_world::item::{ItemStack, ItemType};
use std::io::{Read, Write};
use std::net::TcpStream;

fn identity() -> PlayerIdentity {
    PlayerIdentity::generate().expect("os randomness")
}

fn connect(port: u16) -> TcpStream {
    let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to loopback");
    stream
        .set_read_timeout(Some(TEST_HARD_DEADLINE))
        .expect("read timeout");
    stream
}

fn join_reply(
    stream: &mut TcpStream,
    client: &mut ManualClient,
    name: &str,
    key: PlayerKey,
    proof: Vec<u8>,
) -> ServerToClient {
    client.send(
        stream,
        &ClientToServer::Join {
            credential: JoinCredential::Name(name.to_string()),
            key,
            proof,
            view_distance: 2,
            cached_sections: Vec::new(),
        },
    );
    client.recv(stream)
}

fn rejected(reply: ServerToClient) -> JoinRejectReason {
    match reply {
        ServerToClient::JoinReject { reason } => reason,
        other => panic!("expected JoinReject, got {other:?}"),
    }
}

#[test]
fn pending_connections_are_capped_per_address() {
    let mut hub = RemoteHub::default();
    let port = hub.open_to_lan(0).expect("bind an ephemeral port");
    let extra = 3;
    let sockets: Vec<TcpStream> = (0..MAX_PENDING_PER_IP + extra)
        .map(|_| connect(port))
        .collect();

    let deadline = Instant::now() + TEST_HARD_DEADLINE;
    let mut closed = vec![false; sockets.len()];
    while closed.iter().filter(|c| **c).count() < extra {
        assert!(Instant::now() < deadline, "the surplus was never closed");
        hub.accept_new();
        assert!(hub.pending.len() <= MAX_PENDING_PER_IP, "the cap holds");
        for (socket, closed) in sockets.iter().zip(closed.iter_mut()) {
            socket.set_nonblocking(true).expect("nonblocking probe");
            let mut byte = [0u8; 1];
            *closed |= match (&*socket).read(&mut byte) {
                Ok(0) => true,
                Ok(_) => false,
                Err(e) => e.kind() != io::ErrorKind::WouldBlock,
            };
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(hub.pending.len(), MAX_PENDING_PER_IP);
    assert_eq!(closed.iter().filter(|c| **c).count(), extra);
    hub.shutdown();
}

#[test]
fn oversize_pre_join_frames_drop_the_connection() {
    let mut hub = RemoteHub::default();
    let port = hub.open_to_lan(0).expect("bind an ephemeral port");
    let mut server = crate::server::session_build::build_headless_session("", 5, 2);
    let mut socket = connect(port);
    let mut header = ((PRE_JOIN_MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
    header.push(0);
    socket.write_all(&header).expect("send header");

    let deadline = Instant::now() + TEST_HARD_DEADLINE;
    while hub.pending.is_empty() {
        assert!(Instant::now() < deadline, "the socket was never handed off");
        hub.accept_new();
        std::thread::sleep(Duration::from_millis(5));
    }
    let (local_tx, _local_rx) = std::sync::mpsc::channel();
    while !hub.pending.is_empty() {
        assert!(
            Instant::now() < deadline,
            "the oversize frame was never refused"
        );
        hub.pump(&mut server, &mut Vec::new(), &local_tx);
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut byte = [0u8; 1];
    assert!(
        !matches!(socket.read(&mut byte), Ok(n) if n > 0),
        "nothing is sent back; the socket closes"
    );
    hub.shutdown();
}

#[test]
fn joins_without_a_valid_identity_proof_are_refused() {
    let server = headless("", 5, 2);
    let mut host = crate::server::handle::spawn(server);
    let port = host.open_to_lan(0).expect("bind an ephemeral port");
    let (alice, mallory) = (identity(), identity());

    let mut s = connect(port);
    let mut client = ManualClient::hello(&mut s);
    let forged = mallory.sign_join(client.session.binding());
    assert_eq!(
        rejected(join_reply(
            &mut s,
            &mut client,
            "Alice",
            alice.key(),
            forged
        )),
        JoinRejectReason::BadProof
    );
    let earlier = *client.session.binding();

    let mut s = connect(port);
    let mut client = ManualClient::hello(&mut s);
    let replayed = alice.sign_join(&earlier);
    assert_eq!(
        rejected(join_reply(
            &mut s,
            &mut client,
            "Alice",
            alice.key(),
            replayed
        )),
        JoinRejectReason::BadProof,
        "a proof from another connection's exchange never verifies"
    );
    let bare = alice.sign_join(&client.challenge);
    let mut s = connect(port);
    let mut client = ManualClient::hello(&mut s);
    assert_eq!(
        rejected(join_reply(&mut s, &mut client, "Alice", alice.key(), bare)),
        JoinRejectReason::BadProof,
        "nor does a proof over the bare challenge a relay could forward"
    );

    let mut s = connect(port);
    let mut client = ManualClient::hello(&mut s);
    let proof = alice.sign_join(client.session.binding());
    assert!(matches!(
        rejected(join_reply(&mut s, &mut client, "Ann!", alice.key(), proof)),
        JoinRejectReason::InvalidName(_)
    ));

    let mut first = connect(port);
    client_handshake(
        &mut first,
        &alice,
        joins_as("Alice"),
        2,
        &installed_mod_ids(),
        Vec::new(),
    )
    .expect("alice joins");
    let mut second = connect(port);
    match client_handshake(
        &mut second,
        &alice,
        joins_as("Alice"),
        2,
        &installed_mod_ids(),
        Vec::new(),
    ) {
        Err(HandshakeError::Rejected(JoinRejectReason::AlreadyConnected)) => {}
        other => panic!("expected AlreadyConnected, got {other:?}"),
    }
    drop(first);
    host.shutdown_and_join();
}

#[test]
fn a_display_name_never_unlocks_another_identitys_save() {
    let dir = std::env::temp_dir().join(format!("petramond-impostor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("players")).expect("players dir");
    let (rachel, impostor) = (PlayerKey([0x51; 32]), PlayerKey([0x66; 32]));
    let mut saved =
        crate::player::Player::new(petramond_math::world_pos::WorldPos::new(0.5, 80.0, 0.5));
    saved.inventory.add(ItemStack::new(ItemType::Dirt, 64));
    std::fs::write(
        dir.join(format!("players/{rachel}.dat")),
        crate::save::player::encode(&saved, &crate::save::palette::Palette::identity()),
    )
    .expect("player file");

    let mut server = crate::server::session_build::build_headless_session("", 5, 2);
    let opened = crate::save::open_at(dir.clone()).expect("temp save opens");
    server.world.attach_save(opened.save, opened.saved);

    let (data, name) = server
        .admit_remote_player(rachel, "Rachel", 2, &[])
        .expect("rachel joins");
    assert_eq!(name, "Rachel");
    assert!(
        data.self_restore.inventory[0].is_some(),
        "rachel's own save restores"
    );
    server.remove_remote_session(data.player_id);

    let (data, name) = server
        .admit_remote_player(impostor, "rachel", 2, &[])
        .expect("the impostor is admitted as somebody else");
    assert_eq!(name, "rachel2", "the offline owner keeps the name");
    assert!(
        data.self_restore.inventory[0].is_none(),
        "and the impostor gets a fresh player, not rachel's items"
    );

    if let Some(save) = server.world.save_mut() {
        save.shutdown();
    }
    let _ = std::fs::remove_dir_all(&dir);
}
