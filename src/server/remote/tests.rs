use super::*;
use crate::net::connection::TcpClientConn;
use crate::net::framing::{read_msg, write_msg};
use crate::net::handle::ServerHandle;
use crate::net::handshake::{client_handshake, installed_mod_ids, HandshakeError, ServerOffer};
use crate::net::identity::PlayerIdentity;
use crate::net::protocol::{PlayerAction, PlayerUpdate, TargetRef};
use crate::net::remap::IdRemap;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_util::test_time::TEST_HARD_DEADLINE;
use petramond_world::block::Block;
use petramond_world::chunk::SectionPos;
use petramond_world::item::{ItemStack, ItemType};
use std::net::TcpStream;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

fn identity() -> PlayerIdentity {
    PlayerIdentity::generate().expect("os randomness")
}

fn key(byte: u8) -> PlayerKey {
    PlayerKey([byte; 32])
}

/// A headless server for the suite: OFFLINE, so a join carries a plain name and
/// nothing in these tests reaches the account service.
pub(super) fn headless(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
) -> crate::server::game::ServerGame {
    let mut server =
        crate::server::session_build::build_headless_session(world_name, new_seed, render_dist);
    server.account_policy = crate::account::AccountPolicy::Offline;
    server
}

/// The credential callback for an offline server: the plain name the test joins as.
pub(super) fn joins_as(
    name: &'static str,
) -> impl FnOnce(&ServerOffer) -> Result<crate::net::protocol::JoinCredential, HandshakeError> {
    move |offer| {
        assert!(
            !offer.requires_account,
            "the suite's servers are offline; a test must not need the account service"
        );
        Ok(crate::net::protocol::JoinCredential::Name(name.to_string()))
    }
}

fn connect(port: u16) -> TcpStream {
    let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to loopback");
    stream
        .set_read_timeout(Some(TEST_HARD_DEADLINE))
        .expect("read timeout");
    stream
}

/// Drain `handle` until `f` yields, sleeping between polls; None =
/// timeout. Acks every streaming batch like a live client so the
/// server's flow-control window keeps streaming. The sleep only parks an
/// EMPTY poll — while messages are flowing we re-poll immediately, so a
/// fast (unthrottled) server isn't rounded up to 10 ms per batch.
fn drain_until<T>(
    handle: &mut ServerHandle,
    timeout: Duration,
    mut f: impl FnMut(ServerToClient) -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + timeout;
    let mut msgs = Vec::new();
    while Instant::now() < deadline {
        handle.drain(&mut msgs);
        let received = msgs.len();
        for msg in msgs.drain(..) {
            if matches!(msg, ServerToClient::StreamBatchEnd { .. }) {
                let _ = handle.send(ClientToServer::StreamBatchAck {
                    messages_per_second: 1e9, // server clamps
                });
            }
            if let Some(hit) = f(msg) {
                return Some(hit);
            }
        }
        if received == 0 {
            std::thread::sleep(Duration::from_millis(2));
        } else {
            std::thread::yield_now();
        }
    }
    None
}

/// Duplicate names are never rejected: admission appends the lowest free
/// numeric suffix (case-insensitive vs every other identity), and the
/// suffixed name IS the session's display name. The same identity joining
/// twice, however, is refused.
#[test]
fn duplicate_join_names_dedupe_with_the_lowest_free_numeric_suffix() {
    let mut server = crate::server::session_build::build_server_inline("", 3, 2);
    let local = server.sessions[0].name.clone();
    let requested = if local.eq_ignore_ascii_case("Rachel") {
        "Visitor"
    } else {
        "Rachel"
    };
    let lower = requested.to_ascii_lowercase();
    let upper = requested.to_ascii_uppercase();
    let (_, first) = server
        .admit_remote_player(key(1), requested, 32, &[])
        .expect("admitted");
    assert_eq!(first, requested);
    let (_, second) = server
        .admit_remote_player(key(2), &lower, 32, &[])
        .expect("admitted");
    assert_eq!(
        second,
        format!("{lower}2"),
        "case-insensitive dedupe, suffix appended"
    );
    let (_, third) = server
        .admit_remote_player(key(3), &upper, 32, &[])
        .expect("admitted");
    assert_eq!(
        third,
        format!("{upper}3"),
        "the lowest FREE suffix (2 is taken)"
    );
    let names: Vec<&str> = server.sessions.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&requested) && names.contains(&second.as_str()));

    assert_eq!(
        server.admit_remote_player(key(2), "Other", 32, &[]).err(),
        Some(JoinRejectReason::AlreadyConnected),
        "one identity, one session"
    );
    let (_, host_name) = server
        .admit_remote_player(key(4), &local.to_ascii_lowercase(), 32, &[])
        .expect("admitted");
    assert_eq!(
        host_name,
        format!("{}2", local.to_ascii_lowercase()),
        "the local session's name is taken too"
    );
}

#[test]
fn headless_disconnect_detaches_before_player_id_reuse() {
    let mut server = headless("", 3, 2);
    let dismounts = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&dismounts);
    server.mods.bus_mut().on_post(
        crate::events::PostEventKind::PlayerDismounted,
        0,
        move |_, event| {
            if matches!(
                event,
                crate::events::PostEvent::PlayerDismounted {
                    player: PlayerId(0),
                    mount: crate::mob::riding::Mount {
                        target: crate::mob::riding::MountTarget::Mob(77),
                        seat: 0,
                    }
                }
            ) {
                observed.fetch_add(1, AtomicOrdering::SeqCst);
            }
        },
    );

    let (first, _) = server
        .admit_remote_player(key(1), "First", 16, &[])
        .expect("admitted");
    assert_eq!(first.player_id, PlayerId(0));
    assert!(server
        .world
        .riding_mut()
        .mount(0, crate::mob::riding::MountTarget::Mob(77), 0));
    server.sessions[0].sim.mount = server.world.riding().mount_of(0);

    assert_eq!(
        server.remove_remote_session(PlayerId(0)).as_deref(),
        Some("First")
    );
    assert!(server.sessions.is_empty());
    assert_eq!(server.world.riding().mount_of(0), None);

    let (second, _) = server
        .admit_remote_player(key(2), "Second", 16, &[])
        .expect("admitted");
    assert_eq!(second.player_id, PlayerId(0), "the freed id recycles");
    assert_eq!(server.world.riding().mount_of(0), None);
    assert_eq!(server.sessions[0].sim.mount, None);

    server.pump_tagged(crate::events::tick::TICK_DT * 1.01, &mut Vec::new(), &[]);
    assert_eq!(dismounts.load(AtomicOrdering::SeqCst), 1);
    server.pump_tagged(crate::events::tick::TICK_DT * 1.01, &mut Vec::new(), &[]);
    assert_eq!(
        dismounts.load(AtomicOrdering::SeqCst),
        1,
        "one detach transition emits exactly once"
    );
}

/// The full remote-join loop over real TCP on 127.0.0.1: open to LAN on an
/// ephemeral port, handshake + join a remote client (restored from a
/// pre-seeded, pre-identity player file it adopts on first join), stream it
/// terrain, place a block from the remote side and see the delta come back,
/// dedupe a duplicate name, ignore Pause while remote players exist, and
/// broadcast joins/leaves.
#[test]
fn full_lan_join_place_pause_gate_and_leave() {
    // Includes worldgen, mod startup, threaded streaming and the TCP narrative.
    let test_end = Instant::now() + Duration::from_secs(30);
    let remain = || {
        let left = test_end.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "full_lan narrative exceeded the hard 30 s test budget"
        );
        left
    };

    let dir = petramond_util::test_dirs::TestScratchDir::new("lan");
    std::fs::create_dir_all(dir.join("players")).expect("temp players dir");
    // Pre-seed the joining player's save as a PRE-IDENTITY name-keyed file
    // (the visitor's identity adopts it on first join): standing on the
    // seed's dry-land
    // spawn pick (a placement within legitimate reach must exist around
    // it — the reach eye is ring-bounded, so the visitor builds from
    // where it actually stands) + dirt to place (a fresh spawn would be
    // empty-handed).
    let spawn = petramond_worldgen::spawn::find_spawn(7);
    let visitor_feet = WorldPos::new(
        spawn.x as f64 + 0.5,
        (spawn.y + 1) as f64,
        spawn.z as f64 + 0.5,
    );
    let mut visitor = crate::player::Player::new(visitor_feet);
    visitor.inventory.add(ItemStack::new(ItemType::Dirt, 64));
    // Written before the world opens: the identity palette is what a fresh
    // world pins (registry order).
    let pal = crate::save::palette::Palette::identity();
    std::fs::write(
        dir.join("players/Visitor.dat"),
        crate::save::player::encode(&visitor, &pal),
    )
    .expect("player file");
    // The later join/leave cycles (dedupe + broadcast ordering) get restored
    // players too — their FINAL names are predictable. A fresh nameless spawn
    // would run `find_spawn` (~0.4 s of worldgen search) synchronously inside
    // the server's admit path, dominating this test for no coverage gain
    // (spawn search has its own tests).
    for extra in ["vISITOR2", "Guest"] {
        std::fs::write(
            dir.join(format!("players/{extra}.dat")),
            crate::save::player::encode(&visitor, &pal),
        )
        .expect("player file");
    }

    let host_player = crate::server::session_build::LocalPlayer {
        key: identity().key(),
        name: crate::save::client::resolve_player_name(&crate::save::client::load()),
    };
    let (mut server, _, _) =
        crate::server::session_build::build_server("", 7, 2, Some(host_player));
    // A threaded-pool listen server, but still a SUITE server: offline, so the
    // joins below carry plain names and nothing reaches the account service.
    server.account_policy = crate::account::AccountPolicy::Offline;
    let opened = crate::save::open_at(dir.to_path_buf()).expect("temp save opens");
    server.world.attach_save(opened.save, opened.saved);
    // Pre-build a tiny stone pad at the visitor's feet (threaded pool, but a
    // single column) so the place claim targets a known cell instead of
    // scanning streamed worldgen under CPU contention.
    let (pcx, pcy, pcz) = (
        spawn.x.div_euclid(16),
        spawn.y.div_euclid(16),
        spawn.z.div_euclid(16),
    );
    server.world.update_load(pcx, pcy, pcz);
    // Stream-finality can refuse writes while gen/overlay is in flight —
    // poll + retry until the pad sticks (common under a parallel suite).
    'pad: loop {
        let _ = remain();
        server.world.poll();
        if !server
            .world
            .data()
            .section_loaded_at(spawn.x, spawn.y, spawn.z)
        {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let mut ok = true;
        for dx in -2..=2 {
            for dz in -2..=2 {
                let x = spawn.x + dx;
                let z = spawn.z + dz;
                let _ = server.world.set_block_world(x, spawn.y, z, Block::Stone);
                let _ = server.world.set_block_world(x, spawn.y + 1, z, Block::Air);
                let _ = server.world.set_block_world(x, spawn.y + 2, z, Block::Air);
                if server.world.data().chunk_block(x, spawn.y, z) != Block::Stone.id() {
                    ok = false;
                }
            }
        }
        if ok {
            break 'pad;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let place_target = IVec3::new(spawn.x + 2, spawn.y, spawn.z);
    let mut host = crate::server::handle::spawn(server);
    // One fixed tick per loop iteration, compute-bound. The pool stays
    // THREADED so handshake RTs are not stuck behind inline gen on the
    // server thread.
    host.unthrottle_for_test();

    let port = host.open_to_lan(0).expect("bind an ephemeral port");
    assert_ne!(port, 0, "the reply carries the actual bound port");
    assert_eq!(
        host.open_to_lan(0).expect("idempotent"),
        port,
        "a second open reports the same port"
    );

    // A wrong protocol version is refused with HelloReject.
    {
        let mut probe = connect(port);
        write_msg(&mut probe, &ClientToServer::Hello { protocol: 9999 }).expect("send");
        match read_msg::<ServerToClient, _>(&mut probe).expect("a reply") {
            ServerToClient::HelloReject { server_protocol } => {
                assert_eq!(server_protocol, PROTOCOL_VERSION);
            }
            other => panic!("expected HelloReject, got {other:?}"),
        }
    }

    // The real join. A render distance of 2 streams ~25 columns (enough for
    // a buildable spot near the visitor's feet) instead of ~1000 — the
    // wire path under test is identical.
    let visitor_id = identity();
    let mut stream = connect(port);
    let join = client_handshake(
        &mut stream,
        &visitor_id,
        joins_as("Visitor"),
        2,
        &installed_mod_ids(),
        Vec::new(),
    )
    .expect("handshake succeeds")
    .join;
    assert_eq!(join.player_id, PlayerId(1));
    assert_eq!(join.seed, 7);
    assert_eq!(join.self_restore.transform.pos, visitor_feet);
    assert_eq!(
        join.self_restore.inventory[0],
        Some(crate::net::protocol::ItemSlotWire {
            item_id: ItemType::Dirt.0,
            count: 64,
            data: None,
        }),
        "the join restore carries the saved inventory"
    );
    assert_eq!(join.players.len(), 1, "only the host was connected");
    assert_eq!(join.players[0].0, PlayerId(0));
    let remap = IdRemap::build(&join.tables);
    assert!(remap.is_identity(), "same process, same registries");
    let conn = TcpClientConn::spawn(stream, remap).expect("connection threads");
    let mut remote = ServerHandle::from_remote(conn);

    // The host is told about the join.
    let joined = drain_until(&mut host, remain(), |msg| match msg {
        ServerToClient::PlayerJoined { id, name } => Some((id, name)),
        _ => None,
    })
    .expect("host hears PlayerJoined");
    assert_eq!(joined, (PlayerId(1), "Visitor".to_string()));

    // Wait until the pad's section is in the remote's sent set (deltas are
    // filtered by `terrain.covers`) and a lit payload has proven light ships.
    let pad_section = SectionPos::from_world(place_target.x, place_target.y, place_target.z)
        .expect("pad is inside the section grid");
    let mut lit_sections = 0usize;
    let mut pad_streamed = false;
    // The visitor's own row spawns in its first batch and afterwards rides
    // only when it changed (per-connection deltas), so remember the sighting.
    let mut own_row = false;
    drain_until(&mut remote, remain(), |msg| {
        match msg {
            ServerToClient::SectionData(p) => {
                if p.skylight.is_some() {
                    lit_sections += 1;
                }
                if p.pos == pad_section {
                    pad_streamed = true;
                }
            }
            ServerToClient::Tick(update) => {
                own_row |= update
                    .players()
                    .is_some_and(|lane| lane.iter().any(|r| r.id == PlayerId(1)));
            }
            _ => {}
        }
        (pad_streamed && lit_sections > 0 && own_row).then_some(())
    })
    .expect("pad section streams with lit terrain and a visitor row");
    assert!(lit_sections > 0, "baked light rides TCP section payloads");
    let target = place_target;
    let placed_at = IVec3::new(target.x, target.y + 1, target.z);

    // Place from the restored feet (the pre-built pad is within reach of
    // that pose); the F1 drift ring rejects a hover claim far from the
    // server's own integration, so we claim where the save put us.
    let update = PlayerUpdate {
        transform: crate::net::protocol::Transform {
            pos: visitor_feet,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
        },
        on_ground: true,
        sneak: false,
        gameplay: true,
        break_held: false,
        use_held: false,
        target: Some(TargetRef::face(target, IVec3::Y)),
        hotbar_slot: 0,
        held_rotation: 0,
        wishdir: Vec3::ZERO,
        jump: false,
        sprint: false,
    };
    remote
        .send(ClientToServer::PlayerUpdate(update))
        .expect("live connection");
    remote
        .send(ClientToServer::Action(PlayerAction::UseClick {
            mob: None,
            target: Some(TargetRef::face(target, IVec3::Y)),
            request_id: None,
            predicted: false,
            jabbed: false,
        }))
        .expect("live connection");
    // Prefer inventory revision (always on self_state) over block_deltas:
    // deltas are filtered by `terrain.covers`, which can lag under load even
    // after the pad section streamed once.
    drain_until(&mut remote, remain(), |msg| {
        let ServerToClient::Tick(update) = msg else {
            return None;
        };
        if update.block_deltas().is_some_and(|deltas| {
            deltas
                .iter()
                .any(|d| d.pos == placed_at && d.block_id == Block::Dirt.0)
        }) {
            return Some(());
        }
        let self_state = update.self_state()?;
        let inv = self_state.inventory.as_ref()?;
        match inv.first() {
            Some(Some(slot)) if slot.item_id == ItemType::Dirt.0 && slot.count == 63 => Some(()),
            _ => None,
        }
    })
    .expect("the remote placement consumes one dirt (delta or inventory)");

    // Pause is ignored while the server has been opened to LAN: ticks
    // keep flowing to the remote client. The pinned clock consumes the
    // Pause message within a couple of iterations, so a short settle
    // replaces the old wall-clock 200 ms wait.
    host.send(ClientToServer::Pause(true)).expect("live pipe");
    std::thread::sleep(Duration::from_millis(50));
    let mut drained = Vec::new();
    remote.drain(&mut drained);
    let before = drained
        .iter()
        .filter_map(|m| match m {
            ServerToClient::Tick(u) => Some(u.tick),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    drain_until(&mut remote, remain(), |msg| match msg {
        ServerToClient::Tick(u) if u.tick > before => Some(()),
        _ => None,
    })
    .expect("ticks keep flowing: Pause is ignored once open to LAN");

    // A second client with the same name (case-insensitive) is ADMITTED
    // under the lowest free numeric suffix, never refused. It asks for the
    // same small render distance as the visitor — this connection never
    // needs terrain, and a big request makes the server generate a huge
    // load target for nothing.
    {
        let mut dup = connect(port);
        let data = client_handshake(
            &mut dup,
            &identity(),
            joins_as("vISITOR"),
            2,
            &installed_mod_ids(),
            Vec::new(),
        )
        .expect("a duplicate name joins deduped, not rejected")
        .join;
        let dup_id = data.player_id;
        let name = drain_until(&mut host, remain(), |msg| match msg {
            ServerToClient::PlayerJoined { id, name } if id == dup_id => Some(name),
            _ => None,
        })
        .expect("host hears the deduped join");
        assert_eq!(
            name, "vISITOR2",
            "the requested name gains a numeric suffix"
        );
        drop(dup); // socket drop -> leave path
        drain_until(&mut host, remain(), |msg| match msg {
            ServerToClient::PlayerLeft { id } if id == dup_id => Some(()),
            _ => None,
        })
        .expect("the deduped guest's leave lands before the next join");
    }

    // A third player joins, then vanishes (socket drop, no Disconnect):
    // everyone else hears PlayerJoined then PlayerLeft.
    let guest_id = {
        let mut guest = connect(port);
        let data = client_handshake(
            &mut guest,
            &identity(),
            joins_as("Guest"),
            2,
            &installed_mod_ids(),
            Vec::new(),
        )
        .expect("guest joins")
        .join;
        assert_eq!(
            data.players.len(),
            2,
            "the guest sees both connected players"
        );
        data.player_id
        // `guest` drops here: the server reader hits EOF -> leave path.
    };
    for (name, handle) in [("host", &mut host), ("visitor", &mut remote)] {
        // One pass for both events: they may land in the same drain batch.
        let mut joined = false;
        let mut left = false;
        drain_until(handle, remain(), |msg| {
            match msg {
                ServerToClient::PlayerJoined { id, .. } if id == guest_id => joined = true,
                ServerToClient::PlayerLeft { id } if id == guest_id => {
                    assert!(joined, "{name}: join broadcasts before leave");
                    left = true;
                }
                _ => {}
            }
            (joined && left).then_some(())
        })
        .unwrap_or_else(|| panic!("{name} hears the guest join then leave"));
    }

    // A clean remote quit (farewell Disconnect through the handle drop
    // path) runs the leave path: the host hears PlayerLeft and the
    // visitor's player file is saved with the post-placement inventory.
    remote.shutdown_and_join();
    let left = drain_until(&mut host, remain(), |msg| match msg {
        ServerToClient::PlayerLeft { id } => Some(id),
        _ => None,
    });
    assert_eq!(left, Some(PlayerId(1)), "host hears the visitor leave");

    let saved_count = loop {
        let left = remain();
        if let Some(data) = std::fs::read(dir.join(format!("players/{}.dat", visitor_id.key())))
            .ok()
            .and_then(|bytes| crate::save::player::decode(&bytes, &pal).ok())
        {
            let count = data
                .inventory
                .slot(0)
                .map(|s| (s.item, s.count))
                .unwrap_or((ItemType::Dirt, 0));
            if count == (ItemType::Dirt, 63) {
                break count;
            }
        }
        if left < Duration::from_millis(25) {
            break (ItemType::Dirt, 0);
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_eq!(
        saved_count,
        (ItemType::Dirt, 63),
        "the leave path saved the visitor with the placed block consumed"
    );

    host.shutdown_and_join();
}

/// The HEADLESS server shape end-to-end: built with NO local session,
/// the world freezes while empty (pause-when-empty), the first TCP join
/// claims PlayerId(0) and streams terrain through the ack-windowed
/// batches, and after the last leave the world freezes again — a rejoin
/// resumes from (nearly) the frozen tick instead of wall-clock time
/// having passed.
#[test]
fn headless_server_join_leave_cycle_freezes_the_world_when_empty() {
    let mut server = headless("", 11, 2);
    assert!(!server.sessions.has_local_session());
    assert!(server.sessions.is_empty());
    assert!(server.clock.lan_ever_opened(), "the pause gate starts open");
    // In-process smoke first: pumping an EMPTY headless server runs no
    // ticks, produces no recipients, and panics nowhere.
    let t0 = server.world.current_tick();
    for _ in 0..5 {
        let out = server.pump_tagged(0.05, &mut Vec::new(), &[]);
        assert!(out.msgs.is_empty() && out.remote.is_empty());
    }
    assert_eq!(server.world.current_tick(), t0, "empty server: frozen");

    let mut host = crate::server::handle::spawn(server);
    let port = host.open_to_lan(0).expect("bind an ephemeral port");

    // First join claims id 0 — no local session holds it on headless.
    let head = identity();
    let mut stream = connect(port);
    let join = client_handshake(
        &mut stream,
        &head,
        joins_as("Head"),
        16,
        &installed_mod_ids(),
        Vec::new(),
    )
    .expect("join")
    .join;
    assert_eq!(join.player_id, PlayerId(0));
    let conn = TcpClientConn::spawn(stream, IdRemap::build(&join.tables)).expect("conn threads");
    let mut remote = ServerHandle::from_remote(conn);

    // Connected: the world runs and terrain streams (drain_until acks
    // the batches like a live client). Terrain FIRST — the whole
    // render-dist-2 window can finish streaming before the first tick
    // lands, and a drain that waited for a tick would silently discard
    // every section payload it swept past.
    drain_until(&mut remote, TEST_HARD_DEADLINE, |msg| {
        matches!(msg, ServerToClient::SectionData(_)).then_some(())
    })
    .expect("terrain streams to the headless server's first player");
    let first = drain_until(&mut remote, TEST_HARD_DEADLINE, |msg| match msg {
        ServerToClient::Tick(u) => Some(u.tick),
        _ => None,
    })
    .expect("ticks flow to the joined player");
    let last_seen = drain_until(&mut remote, TEST_HARD_DEADLINE, |msg| match msg {
        ServerToClient::Tick(u) if u.tick > first + 5 => Some(u.tick),
        _ => None,
    })
    .expect("the world advances while a player is connected");

    // Clean leave (farewell Disconnect through the handle drop path):
    // the session list empties and the world freezes. One second of wall
    // time is ~20 ticks if the sim kept running; stay under the hard budget.
    remote.shutdown_and_join();
    std::thread::sleep(Duration::from_secs(1));

    let mut stream = connect(port);
    let join = client_handshake(
        &mut stream,
        &head,
        joins_as("Head"),
        16,
        &installed_mod_ids(),
        Vec::new(),
    )
    .expect("rejoin")
    .join;
    assert_eq!(join.player_id, PlayerId(0), "the freed id recycles");
    let conn = TcpClientConn::spawn(stream, IdRemap::build(&join.tables)).expect("conn threads");
    let mut remote = ServerHandle::from_remote(conn);
    let resumed = drain_until(&mut remote, TEST_HARD_DEADLINE, |msg| match msg {
        ServerToClient::Tick(u) => Some(u.tick),
        _ => None,
    })
    .expect("ticks resume on rejoin");
    assert!(
        resumed < last_seen + 12,
        "the world froze while empty: tick {last_seen} -> {resumed} across \
         1 s of empty wall time (an unfrozen sim would be ~20 ahead)"
    );

    remote.shutdown_and_join();
    host.shutdown_and_join();
}

/// Read replies until `f` takes one, skipping keepalives.
fn reply<T>(stream: &mut TcpStream, mut f: impl FnMut(ServerToClient) -> Option<T>) -> T {
    loop {
        match read_msg::<ServerToClient, _>(stream).expect("a reply") {
            ServerToClient::KeepAlive => continue,
            other => {
                let shown = format!("{other:?}");
                if let Some(hit) = f(other) {
                    return hit;
                }
                panic!("unexpected reply: {shown}");
            }
        }
    }
}

/// `Hello` a raw socket; the `HelloAck`'s `(challenge, requires_account, server_id)`.
fn hello(stream: &mut TcpStream) -> (crate::net::identity::JoinChallenge, bool, String) {
    write_msg(
        stream,
        &ClientToServer::Hello {
            protocol: PROTOCOL_VERSION,
        },
    )
    .expect("send");
    reply(stream, |msg| match msg {
        ServerToClient::HelloAck {
            challenge,
            requires_account,
            server_id,
            ..
        } => Some((challenge, requires_account, server_id)),
        _ => None,
    })
}

/// A proven `Join` offering `credential`, and the server's `JoinReject` reason.
fn refused_join(
    stream: &mut TcpStream,
    challenge: &crate::net::identity::JoinChallenge,
    credential: crate::net::protocol::JoinCredential,
) -> JoinRejectReason {
    let me = identity();
    write_msg(
        stream,
        &ClientToServer::Join {
            credential,
            key: me.key(),
            proof: me.sign_join(challenge),
            view_distance: 2,
            cached_sections: Vec::new(),
        },
    )
    .expect("send");
    reply(stream, |msg| match msg {
        ServerToClient::JoinReject { reason } => Some(reason),
        _ => None,
    })
}

/// An ONLINE server's handshake: it advertises what it wants up front, and
/// refuses the wrong kind of credential BY NAME rather than dropping the socket
/// — the client was told the policy, so a mismatch is a reason to show.
///
/// Redeeming a real ticket is deliberately not exercised here: it is a live
/// HTTPS round trip to the account service, and a unit suite that reaches the
/// network is a unit suite that fails on a train.
#[test]
fn an_online_server_advertises_its_policy_and_refuses_a_plain_name() {
    let mut server = headless("", 5, 2);
    server.account_policy = crate::account::AccountPolicy::Online;
    let mut host = crate::server::handle::spawn(server);
    host.unthrottle_for_test();
    let port = host.open_to_lan(0).expect("bind an ephemeral port");

    let mut probe = connect(port);
    let (challenge, requires_account, server_id) = hello(&mut probe);
    assert!(requires_account, "an online server says so up front");
    assert_eq!(server_id.len(), 32, "a full-width opaque server id");
    assert_eq!(
        refused_join(
            &mut probe,
            &challenge,
            crate::net::protocol::JoinCredential::Name("Sneaky".to_string()),
        ),
        JoinRejectReason::AccountRequired
    );

    // The mirror image: an OFFLINE server cannot redeem a ticket, and the name
    // it would need lives inside one — so it refuses that too, by its own name.
    let mut offline = crate::server::handle::spawn(headless("", 5, 2));
    offline.unthrottle_for_test();
    let offline_port = offline.open_to_lan(0).expect("bind an ephemeral port");
    let mut probe = connect(offline_port);
    let (challenge, requires_account, _) = hello(&mut probe);
    assert!(!requires_account, "an offline server asks for no account");
    assert_eq!(
        refused_join(
            &mut probe,
            &challenge,
            crate::net::protocol::JoinCredential::Ticket(server_id),
        ),
        JoinRejectReason::AccountNotAccepted
    );

    offline.shutdown_and_join();
    host.shutdown_and_join();
}

/// A verified account is filed under its stable account id, never its
/// username: a website rename between visits moves the display name and
/// nothing else, and one account is one session whatever it is called.
///
/// This shipped broken once — the username keyed the record, so a rename (or
/// a first sign-in) silently opened the world on a fresh spawn with the old
/// character still on disk.
#[test]
fn a_verified_account_is_filed_under_its_id_not_its_username() {
    let mut server = headless("", 13, 2);
    let account = super::joins::account_key(77);

    let (data, first) = server
        .admit_remote_player(account, "Explorer", 8, &[])
        .expect("admitted");
    assert_eq!(first, "Explorer", "the display name is the username");
    assert_eq!(
        server.sessions.by_id(data.player_id).map(|s| s.key),
        Some(account),
        "a verified account files under its id"
    );
    assert_eq!(
        server
            .admit_remote_player(account, "ShinyVision", 8, &[])
            .err(),
        Some(JoinRejectReason::AlreadyConnected),
        "a renamed account is still the same session"
    );

    server.remove_remote_session(data.player_id);
    let (data, renamed) = server
        .admit_remote_player(account, "ShinyVision", 8, &[])
        .expect("rejoins");
    assert_eq!(renamed, "ShinyVision");
    assert_eq!(
        server.sessions.by_id(data.player_id).map(|s| s.key),
        Some(account),
        "a rename moves the display name and nothing else"
    );
}
