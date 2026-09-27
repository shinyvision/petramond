//! What the engine writes into a mod's file reads back through `mod_api`'s
//! codec — the one `mod_sdk::capture` re-exports — piece for piece, and a
//! `ChangedSince` states exactly what changed.

use std::collections::BTreeSet;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mod_api::capture::{
    check_piece, decode_documented, envelope_entries, record_envelope, records,
    ClientCapturedClock, ClientCapturedSession, ClientCapturedView, ClientEnvelope,
    ClientEventsPhase, ClientPieceKind, ClientPresence, ClientStateKey, ClientStateSelect,
};
use mod_api::ClientFileAnswer;
use petramond_util::test_dirs::TestScratchDir;

use crate::capture::body::{decode_body, CapturedCues};
use crate::capture::events::{Applied, EventsLogs, FrameInput};
use crate::capture::moment::Moment;
use crate::capture::state;
use crate::capture::view::{CueHand, CueMotion, CueShake, ViewCue};
use crate::modding::client::files::{self, FileRef};
use crate::net::protocol::{BlockDelta, SectionPayload, ServerToClient, TickUpdate, WorldEventMsg};
use crate::worker::JobPool;
use crate::world::{column_key, section_key};
use crate::world::{ReplicaWorld, ServerWorld};
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ};

const WAIT: Duration = Duration::from_secs(60);

/// A replica holding `radius`² columns of a stone floor with a lamp on it,
/// installed through its ordinary ingest, its first frame ended.
fn replica(radius: i32, pool: &Arc<JobPool>) -> ReplicaWorld {
    let mut server = ServerWorld::with_pool(0, 1, Arc::clone(pool));
    for cz in -radius..=radius {
        for cx in -radius..=radius {
            let mut c = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    c.set_block(x, 64, z, Block::Stone);
                }
            }
            c.set_block(3, 65, 3, Block::Torch);
            server.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
        }
    }
    let mut replica = ReplicaWorld::with_pool(0, 1, Arc::clone(pool));
    for cp in server.data().columns.keys().copied().collect::<Vec<_>>() {
        replica.install_remote_column(server.column_payload(cp).unwrap());
    }
    for pos in server.data().sections.keys().copied().collect::<Vec<_>>() {
        replica.install_remote_section(server.section_payload(pos).unwrap());
    }
    replica.changes_mut().end_frame();
    replica
}

fn moment() -> Moment {
    let mut moment = Moment::new(ClientCapturedSession {
        seed: 7,
        local_player: mod_api::PlayerId(2),
        mods: Vec::new(),
    });
    moment.tick = 40;
    moment.day_clock = 900;
    moment
}

fn write_state(
    world: &ReplicaWorld,
    select: ClientStateSelect,
    file: &FileRef,
    envelopes: Option<&FileRef>,
    pool: &Arc<JobPool>,
) -> [u64; 2] {
    let (tx, rx) = mpsc::channel();
    let snap = state::take(world, &moment(), &select, None);
    state::submit(snap, file.clone(), envelopes.cloned(), pool, move |done| {
        let _ = tx.send(done);
    })
    .expect("the file takes the record");
    match rx.recv_timeout(WAIT).expect("the record completes") {
        Ok(ClientFileAnswer::Done {
            range: Some(range), ..
        }) => range,
        other => panic!("the record did not land: {other:?}"),
    }
}

/// Everything queued to `file` so far has landed, then its bytes.
fn read_all(file: &FileRef) -> Vec<u8> {
    let (tx, rx) = mpsc::channel();
    files::sync(file, move |_| {
        let _ = tx.send(());
    });
    rx.recv_timeout(WAIT).expect("the file syncs");
    std::fs::read(file.path()).expect("the file exists")
}

/// The pieces a record's envelope lists, each checked on its own, keyed.
fn stated_keys(record: &[u8]) -> BTreeSet<ClientStateKey> {
    let ClientEnvelope::State(envelope) = record_envelope(record).expect("a whole record") else {
        panic!("a state record");
    };
    envelope
        .pieces
        .iter()
        .map(|info| {
            let [at, len] = info.range;
            let (head, _) = check_piece(&record[at as usize..(at + len) as usize])
                .expect("every listed piece checks on its own");
            assert_eq!((head.kind, head.body_crc), (info.kind, info.crc));
            head.state_key().expect("a state piece names its key")
        })
        .collect()
}

#[test]
fn a_state_record_reads_back_piece_for_piece_through_the_mod_codec() {
    let pool = Arc::new(JobPool::new(2));
    let scratch = TestScratchDir::new("capture-state");
    // Enough terrain that the record streams in several chunks.
    let world = replica(8, &pool);
    let file = FileRef::in_dir(&scratch, "rec/world.pmc");
    let envelopes = FileRef::in_dir(&scratch, "rec/world.env");
    let range = write_state(
        &world,
        ClientStateSelect::All,
        &file,
        Some(&envelopes),
        &pool,
    );
    let bytes = read_all(&file);
    let walked: Vec<_> = records(&bytes, 0).collect();
    assert_eq!(walked.len(), 1, "one whole record: {walked:?}");
    let (at, head) = walked[0].clone().unwrap();
    assert_eq!([at, head.len], range);
    let record = &bytes[at as usize..(at + head.len) as usize];

    let keys = stated_keys(record);
    for pos in world.data().sections.keys() {
        assert!(keys.contains(&section_key(*pos)), "section {pos:?} stated");
    }
    for pos in world.data().columns.keys() {
        assert!(keys.contains(&column_key(*pos)), "column {pos:?} stated");
    }
    let ClientEnvelope::State(envelope) = record_envelope(record).unwrap() else {
        unreachable!()
    };
    let piece_of = |key: ClientStateKey| {
        let info = envelope.pieces.iter().find(|p| p.key == Some(key)).unwrap();
        &record[info.range[0] as usize..(info.range[0] + info.range[1]) as usize]
    };
    let clock: ClientCapturedClock = decode_documented(piece_of(ClientStateKey::Clock)).unwrap();
    assert_eq!((clock.tick, clock.day_clock), (40, 900));
    let presence: ClientPresence = decode_documented(piece_of(ClientStateKey::Presence)).unwrap();
    for pos in world.data().sections.keys() {
        let column = presence
            .columns
            .iter()
            .find(|c| c.pos == [pos.cx, pos.cz])
            .expect("its column is present");
        assert!(column.has_section(presence.cy_min, pos.cy));
    }
    // The opaque body is the engine's own replication value, exactly.
    let lamp = SectionPos::from_world(3, 65, 3).unwrap();
    let (_, body) = check_piece(piece_of(section_key(lamp))).unwrap();
    let payload: SectionPayload = decode_body(body).unwrap();
    assert_eq!(
        payload.blocks.0,
        world.section_payload(lamp).unwrap().blocks.0
    );

    let env_bytes = read_all(&envelopes);
    let entries: Vec<_> = envelope_entries(&env_bytes, 0).collect();
    assert_eq!(entries.len(), 1);
    let entry = entries[0].clone().unwrap().1;
    assert_eq!(entry.record, range, "the entry names the record it follows");
}

#[test]
fn changed_since_states_exactly_what_changed_and_presence_only_when_the_set_moved() {
    let pool = Arc::new(JobPool::new(2));
    let scratch = TestScratchDir::new("capture-changed");
    let mut world = replica(1, &pool);
    let file = FileRef::in_dir(&scratch, "world.pmc");
    let base = world.changes().revision();

    // One block edited: its section, and its column (the edit may move the
    // column's heights) — nothing else.
    let edited = IVec3::new(20, 64, 5);
    world.apply_remote_delta(BlockDelta {
        pos: edited,
        block_id: Block::Air.id(),
        fluid: None,
        state: None,
        cell_kv: Vec::new(),
    });
    let after_edit = world.changes_mut().end_frame().revision;
    let sp = SectionPos::from_world(edited.x, edited.y, edited.z).unwrap();
    let range = write_state(
        &world,
        ClientStateSelect::ChangedSince(base),
        &file,
        None,
        &pool,
    );
    let bytes = read_all(&file);
    let record = &bytes[range[0] as usize..(range[0] + range[1]) as usize];
    assert_eq!(
        stated_keys(record),
        [section_key(sp), column_key(sp.chunk_pos())]
            .into_iter()
            .collect()
    );

    // A section leaves: the terrain key set moved, so Presence — and the
    // section is not stated, being gone.
    let gone = SectionPos::from_world(-10, 64, -10).unwrap();
    world.uninstall_remote_section(gone);
    world.changes_mut().end_frame();
    let range = write_state(
        &world,
        ClientStateSelect::ChangedSince(after_edit),
        &file,
        None,
        &pool,
    );
    let bytes = read_all(&file);
    let record = &bytes[range[0] as usize..(range[0] + range[1]) as usize];
    assert_eq!(
        stated_keys(record),
        [ClientStateKey::Presence].into_iter().collect()
    );

    // A revision this world never issued is served whole, and says so.
    let range = write_state(
        &world,
        ClientStateSelect::ChangedSince(u64::MAX),
        &file,
        None,
        &pool,
    );
    let bytes = read_all(&file);
    let record = &bytes[range[0] as usize..(range[0] + range[1]) as usize];
    let ClientEnvelope::State(envelope) = record_envelope(record).unwrap() else {
        unreachable!()
    };
    assert_eq!(envelope.since, None);
    assert!(stated_keys(record).len() > world.data().sections.len());
}

fn view(at: f64) -> (ViewCue, ClientCapturedView) {
    let cue = ViewCue {
        at,
        pos: petramond_math::world_pos::WorldPos::new(1.0, 70.0, 2.0),
        yaw: 0.5,
        pitch: 0.1,
        roll: 0.0,
        fov_y: 1.2,
        shake: CueShake::default(),
        motion: CueMotion::default(),
        hands: [CueHand::default(), CueHand::default()],
        hotbar: 3,
        animator: Default::default(),
        events: Vec::new(),
    };
    let pose = ClientCapturedView {
        player: mod_api::PlayerId(2),
        pos: [1.0, 70.0, 2.0],
        yaw: 0.5,
        pitch: 0.1,
        roll: 0.0,
        fov_y: 1.2,
    };
    (cue, pose)
}

#[test]
fn an_events_log_holds_every_frame_in_order_with_its_cues_and_view() {
    let pool = Arc::new(JobPool::new(2));
    let scratch = TestScratchDir::new("capture-events");
    let file = FileRef::in_dir(&scratch, "events.pmc");
    let envelopes = FileRef::in_dir(&scratch, "events.env");
    let mut logs = EventsLogs::default();
    logs.begin(9, "studio", file.clone(), Some(envelopes.clone()), 0)
        .unwrap();
    let cue = WorldEventMsg::BlockPlaced {
        pos: IVec3::new(1, 2, 3),
        block_id: Block::Stone.id(),
    };
    for (frame, tick) in [(1u64, 10u64), (2, 11), (3, 12)] {
        logs.submit(
            frame,
            FrameInput {
                world: 1,
                presented_tick: tick as f64 - 0.5,
                applied: vec![
                    Applied::Batch(Box::new(TickUpdate {
                        tick,
                        clock: tick * 2,
                        ..Default::default()
                    })),
                    Applied::Message(ServerToClient::PlayerLeft {
                        id: crate::player::PlayerId(4),
                    }),
                ],
                cues: Some(CapturedCues {
                    events: vec![cue.clone()],
                    dig: None,
                }),
                view: Some(view(tick as f64)),
                changes: Default::default(),
                predicted: Arc::from(Vec::new()),
            },
            &pool,
        );
    }
    let ended = logs.watch(9);
    logs.end(9, None);
    assert_eq!(ended.recv_timeout(WAIT), Ok(ClientEventsPhase::Ended));

    let bytes = read_all(&file);
    let frames: Vec<_> = records(&bytes, 0).map(Result::unwrap).collect();
    assert_eq!(frames.len(), 3);
    for (seq, (at, head)) in frames.iter().enumerate() {
        let record = &bytes[*at as usize..(*at + head.len) as usize];
        let ClientEnvelope::Frame(envelope) = record_envelope(record).unwrap() else {
            panic!("a frame record");
        };
        assert_eq!(envelope.seq, seq as u64);
        assert_eq!(envelope.batches, vec![10 + seq as u64]);
        assert_eq!(envelope.view.unwrap().yaw, 0.5);
        let kinds: Vec<ClientPieceKind> = envelope.pieces.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                ClientPieceKind::BatchWorld,
                ClientPieceKind::BatchRows,
                ClientPieceKind::BatchRest,
                ClientPieceKind::Message,
                ClientPieceKind::Cues,
                ClientPieceKind::View,
            ]
        );
        let cues = envelope
            .pieces
            .iter()
            .find(|p| p.kind == ClientPieceKind::Cues)
            .unwrap();
        let [p, n] = cues.range;
        let (_, body) = check_piece(&record[p as usize..(p + n) as usize]).unwrap();
        let cues: CapturedCues = decode_body(body).unwrap();
        assert_eq!(cues.events, vec![cue.clone()]);
    }
    let env_bytes = read_all(&envelopes);
    let named: Vec<[u64; 2]> = envelope_entries(&env_bytes, 0)
        .map(|entry| entry.unwrap().1.record)
        .collect();
    let written: Vec<[u64; 2]> = frames.iter().map(|(at, h)| [*at, h.len]).collect();
    assert_eq!(named, written, "one entry per record, in record order");
}

/// A still frame is skipped, but not one whose hands fired an event (a
/// place jab) or changed what they hold: the eye alone is not the view.
#[test]
fn a_still_frame_is_written_when_its_hands_moved() {
    let pool = Arc::new(JobPool::new(1));
    let scratch = TestScratchDir::new("capture-still");
    let file = FileRef::in_dir(&scratch, "events.pmc");
    let mut logs = EventsLogs::default();
    logs.begin(3, "studio", file.clone(), None, 0).unwrap();
    let jab = |mut v: (ViewCue, ClientCapturedView)| {
        v.0.events = vec![(crate::player::RigId::default(), 1)];
        v
    };
    let swapped = |mut v: (ViewCue, ClientCapturedView)| {
        v.0.hotbar = 4;
        v
    };
    let frames = [
        view(1.0),
        view(1.1),
        jab(view(1.2)),
        jab(view(1.3)),
        view(1.4),
        swapped(view(1.5)),
        swapped(view(1.6)),
    ];
    for (frame, v) in frames.into_iter().enumerate() {
        logs.submit(
            frame as u64 + 1,
            FrameInput {
                world: 1,
                presented_tick: v.0.at,
                applied: Vec::new(),
                cues: None,
                view: Some(v),
                changes: Default::default(),
                predicted: Arc::from(Vec::new()),
            },
            &pool,
        );
    }
    let ended = logs.watch(3);
    logs.end(3, None);
    assert_eq!(ended.recv_timeout(WAIT), Ok(ClientEventsPhase::Ended));
    let bytes = read_all(&file);
    assert_eq!(
        records(&bytes, 0).count(),
        5,
        "the first, both jabs, the rest after them, the hotbar switch"
    );
}

/// The frame-side cost of writing a whole RD 32 world (~36k sections) as
/// one State record, and how long its bytes take to land. Manual:
/// `cargo test --profile fasttest -p petramond capture_profile -- --ignored --nocapture`.
#[test]
#[ignore]
fn capture_profile() {
    let pool = Arc::new(JobPool::new(JobPool::default_threads()));
    let scratch = TestScratchDir::new("capture-profile");
    let mut world = ReplicaWorld::with_pool(0, 32, Arc::clone(&pool));
    let template = {
        let small = replica(0, &pool);
        let floor = SectionPos::from_world(0, 64, 0).unwrap();
        (
            small.section_payload(floor).unwrap(),
            small.column_payload(floor.chunk_pos()).unwrap(),
        )
    };
    let started = Instant::now();
    for cz in -32..=32 {
        for cx in -32..=32 {
            let mut column = template.1.clone();
            column.pos = ChunkPos::new(cx, cz);
            world.install_remote_column(column);
            for cy in 0..9 {
                let mut section = template.0.clone();
                section.pos = SectionPos::new(cx, cy, cz);
                world.install_remote_section_deferred(section);
            }
        }
    }
    world.changes_mut().end_frame();
    println!(
        "{} sections, {} columns built in {:?}",
        world.data().sections.len(),
        world.data().columns.len(),
        started.elapsed()
    );
    for round in 0..8 {
        let file = FileRef::in_dir(&scratch, &format!("world-{round}.pmc"));
        let (tx, rx) = mpsc::channel();
        let call = Instant::now();
        let snap = state::take(&world, &moment(), &ClientStateSelect::All, None);
        let taken = call.elapsed();
        state::submit(snap, file.clone(), None, &pool, move |done| {
            let _ = tx.send(done);
        })
        .unwrap();
        let on_frame = call.elapsed();
        let done = rx.recv_timeout(Duration::from_secs(600)).unwrap().unwrap();
        let ClientFileAnswer::Done {
            range: Some([_, len]),
            ..
        } = done
        else {
            panic!("{done:?}");
        };
        println!(
            "round {round}: snapshot {taken:?}, on the frame {on_frame:?}, landed after {:?}, {} MB",
            call.elapsed(),
            len / (1 << 20)
        );
    }
}
