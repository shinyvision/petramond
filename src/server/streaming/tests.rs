use super::*;
use crate::net::connection::SERVER_QUEUE_MSGS;
use crate::net::protocol::ClientToServer;
use crate::player::PlayerId;
use crate::server::game::PumpOutput;
use petramond_util::test_time::TEST_HARD_DEADLINE;
use std::time::Instant;

fn count_terrain(msgs: &[ServerToClient]) -> usize {
    msgs.iter()
        .filter(|m| {
            matches!(
                m,
                ServerToClient::ColumnData(_) | ServerToClient::SectionData(_)
            )
        })
        .count()
}

fn acks(out: &PumpOutput) -> Vec<(PlayerId, ClientToServer)> {
    out.remote
        .iter()
        .flat_map(|(id, msgs)| {
            msgs.iter()
                .filter(|m| matches!(m, ServerToClient::StreamBatchEnd { .. }))
                .map(move |_| {
                    (
                        *id,
                        ClientToServer::StreamBatchAck {
                            messages_per_second: 1e9,
                        },
                    )
                })
        })
        .collect()
}

fn batch_markers(msg: &ServerToClient) -> bool {
    matches!(
        msg,
        ServerToClient::StreamBatchStart | ServerToClient::StreamBatchEnd { .. }
    )
}

fn find_lit_air(world: &crate::world::ServerWorld, sp: SectionPos) -> Option<(i32, i32, i32)> {
    let (ox, oy, oz) = sp.origin_world();
    (0..16)
        .flat_map(|y| (0..16).flat_map(move |z| (0..16).map(move |x| (x, y, z))))
        .map(|(x, y, z)| (ox + x, oy + y, oz + z))
        .find(|&(x, y, z)| {
            world.data().chunk_block(x, y, z) == 0 && world.data().skylight_at_world(x, y, z) > 0
        })
}

#[test]
fn stream_allowance_pauses_below_the_reserve_and_terrain_stays_capped() {
    assert_eq!(stream_allowance(0), 0);
    assert_eq!(stream_allowance(STREAM_QUEUE_RESERVE), 0);
    assert!(stream_allowance(STREAM_QUEUE_RESERVE + 64) > 0);
    assert_eq!(terrain_budget(0), 0);
    assert_eq!(
        terrain_budget(stream_allowance(SERVER_QUEUE_MSGS)),
        TERRAIN_SECTIONS_PER_PUMP
    );
    assert_eq!(
        terrain_budget(stream_allowance(usize::MAX)),
        TERRAIN_SECTIONS_PER_PUMP
    );
}

#[test]
fn starved_sessions_pause_streaming_and_resume_without_losing_any() {
    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let player = crate::server::session_build::spawn_player(server.world.data().seed);
    let s = server.add_session_for_test(player);
    let remote_id = server.sessions[s].id;

    let deadline = Instant::now() + TEST_HARD_DEADLINE;
    let mut local_terrain = 0usize;
    while local_terrain == 0 {
        assert!(Instant::now() < deadline, "no terrain became shippable");
        let out = server.pump_tagged(0.01, &mut Vec::new(), &[(remote_id, 0)]);
        local_terrain += count_terrain(&out.msgs);
        for (id, msgs) in &out.remote {
            assert_eq!(*id, remote_id);
            assert!(
                msgs.iter().all(|m| matches!(m, ServerToClient::Tick(_))),
                "a zero-headroom session receives only tick updates"
            );
        }
    }

    let mut remote_terrain = 0usize;
    let mut inbox: Vec<(PlayerId, ClientToServer)> = Vec::new();
    while remote_terrain == 0 {
        assert!(Instant::now() < deadline, "paused terrain never resumed");
        let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, SERVER_QUEUE_MSGS)]);
        remote_terrain += out
            .remote
            .iter()
            .map(|(_, msgs)| count_terrain(msgs))
            .sum::<usize>();
        inbox = acks(&out);
    }
}

#[test]
fn stream_batches_window_on_acks_and_stall_without_them() {
    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let player = crate::server::session_build::spawn_player(server.world.data().seed);
    let s = server.add_session_for_test(player);
    let remote_id = server.sessions[s].id;

    let deadline = Instant::now() + TEST_HARD_DEADLINE;
    let mut batches = 0usize;
    while batches == 0 {
        assert!(Instant::now() < deadline, "no first batch");
        let out = server.pump_tagged(0.01, &mut Vec::new(), &[(remote_id, SERVER_QUEUE_MSGS)]);
        batches += out
            .remote
            .iter()
            .flat_map(|(_, msgs)| msgs)
            .filter(|m| matches!(m, ServerToClient::StreamBatchStart))
            .count();
    }
    assert_eq!(batches, 1, "the pre-ack window is exactly one batch");

    for _ in 0..50 {
        let out = server.pump_tagged(0.01, &mut Vec::new(), &[(remote_id, SERVER_QUEUE_MSGS)]);
        for (_, msgs) in &out.remote {
            assert!(
                msgs.iter().all(|m| matches!(m, ServerToClient::Tick(_))),
                "an unacked window ships nothing but tick updates"
            );
        }
    }

    let mut inbox = vec![(
        remote_id,
        ClientToServer::StreamBatchAck {
            messages_per_second: 1e9,
        },
    )];
    let mut resumed = 0usize;
    while resumed == 0 {
        assert!(Instant::now() < deadline, "streaming never resumed on ack");
        let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, SERVER_QUEUE_MSGS)]);
        resumed += out
            .remote
            .iter()
            .flat_map(|(_, msgs)| msgs)
            .filter(|m| matches!(m, ServerToClient::StreamBatchStart))
            .count();
        inbox = acks(&out);
    }
}

#[test]
fn unload_bursts_clip_to_the_allowance_and_all_arrive() {
    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let player = crate::server::session_build::spawn_player(server.world.data().seed);
    let s = server.add_session_for_test(player);
    let remote_id = server.sessions[s].id;

    let mut awaiting: FxHashSet<SectionPos> = (0..3000)
        .map(|i| SectionPos::new(1000 + i, 0, 1000))
        .collect();
    for sp in awaiting.iter().copied() {
        server.sessions[s].transport.terrain.sent.insert(sp);
    }
    server.sessions[s].transport.terrain.backlog = true;

    let room = STREAM_QUEUE_RESERVE + 100;
    let deadline = Instant::now() + TEST_HARD_DEADLINE;
    let mut inbox: Vec<(PlayerId, ClientToServer)> = Vec::new();
    while !awaiting.is_empty() {
        assert!(Instant::now() < deadline, "clipped unloads never drained");
        let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, room)]);
        for (_, msgs) in &out.remote {
            let streamed = msgs
                .iter()
                .filter(|m| !matches!(m, ServerToClient::Tick(_)) && !batch_markers(m))
                .count();
            assert!(
                streamed <= 100,
                "one pump emitted {streamed} streaming messages into 100 \
                 slots of allowance"
            );
            for m in msgs {
                if let ServerToClient::SectionUnload { pos, .. } = m {
                    awaiting.remove(pos);
                }
            }
        }
        inbox = acks(&out);
    }
}

/// Light refreshes defer per connection: a rebake landing while a session's queue is starved must
/// reach it after the queue drains. Ship log gets drained once per pump, so without carryover
/// the refresh gets lost and the replica's light stays stale for good.
#[test]
fn light_refreshes_defer_for_starved_sessions_and_ship_later() {
    let mut server = crate::server::session_build::build_server_inline("", 1, 2);
    let player = crate::server::session_build::spawn_player(server.world.data().seed);
    let s = server.add_session_for_test(player);
    let remote_id = server.sessions[s].id;

    let deadline = Instant::now() + TEST_HARD_DEADLINE;
    let mut lit: Option<(SectionPos, (i32, i32, i32))> = None;
    let mut inbox: Vec<(PlayerId, ClientToServer)> = Vec::new();
    while lit.is_none() {
        assert!(
            Instant::now() < deadline,
            "no editable lit section streamed"
        );
        let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, SERVER_QUEUE_MSGS)]);
        lit = out.remote.iter().flat_map(|(_, msgs)| msgs).find_map(|m| {
            let ServerToClient::SectionData(p) = m else {
                return None;
            };
            p.skylight.as_ref()?;
            find_lit_air(&server.world, p.pos).map(|cell| (p.pos, cell))
        });
        inbox = acks(&out);
    }
    let (lit, lit_air) = lit.unwrap();

    // Dirty the section's light (a solid block changes the sky column),
    // then pump the remote at ZERO headroom until the rebake lands in its
    // pending carryover — the refresh exists, the starved session got no
    // message for it. The edit must genuinely change light: filling a lit
    // AIR cell with stone (a stone-onto-stone swap is light-equivalent
    // and correctly rebakes nothing).
    assert!(
        server.world.set_block_world(
            lit_air.0,
            lit_air.1,
            lit_air.2,
            petramond_world::block::Block::Stone
        ),
        "edit lands inside the streamed section"
    );
    while !server.sessions[s]
        .transport
        .terrain
        .pending_light
        .contains(&lit)
    {
        assert!(Instant::now() < deadline, "the rebake never landed");
        let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, 0)]);
        assert!(
            out.remote
                .iter()
                .flat_map(|(_, msgs)| msgs)
                .all(|m| { !matches!(m, ServerToClient::LightData(_)) }),
            "a zero-headroom session receives no light refreshes"
        );
    }

    let mut remote_relit = false;
    let mut inbox: Vec<(PlayerId, ClientToServer)> = Vec::new();
    while !remote_relit {
        assert!(
            Instant::now() < deadline,
            "the deferred refresh never shipped"
        );
        let out = server.pump_tagged(0.01, &mut inbox, &[(remote_id, SERVER_QUEUE_MSGS)]);
        remote_relit = out
            .remote
            .iter()
            .flat_map(|(_, msgs)| msgs)
            .any(|m| matches!(m, ServerToClient::LightData(p) if p.pos == lit));
        inbox = acks(&out);
    }
}
