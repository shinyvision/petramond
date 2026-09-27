//! The Apply contract: after any sequence of applies, plays and window
//! moves, the presented world equals applying the same pieces and events to
//! a replica NAIVELY (every piece installed in order and every message
//! ingested on the frame, with no window, no provenance, no cache and no
//! fold), and a key whose content did not change keeps its `Arc`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use mod_api::capture::{
    parse_piece_head, write_record_head, CaptureRecordHead, CaptureRecordKind, ClientCapturedClock,
    ClientCapturedEnv, ClientColumnPresence, ClientFrameEnvelope, ClientPieceInfo, ClientPieceKind,
    ClientPopulation, ClientPresence, ClientRosterEntry, ClientStateEnvelope, ClientStateKey,
    CAPTURE_RECORD_HEAD_LEN,
};
use petramond_math::math::IVec3;
use petramond_util::test_dirs::TestScratchDir;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_MIN_CY};

use super::super::body::{self, CapturedActivity, SectionPayload};
use super::super::feed::Moment;
use super::super::source::{FileRanges, SourceFile};
use super::super::unpack::{self, decode_frame, Frame, FrameItem, StateBody, Vocab};
use super::super::window::{Away, Window};
use super::{Op, Presentation};
use crate::modding::client::files::FileRef;
use crate::net::protocol::{
    BlockDelta, CellKvDelta, ColumnPayload, ItemLane, ItemStateRow, LightPayload, SectionBytes,
    SelfState, ServerToClient, SpatialSoundMsg, TickUpdate, WorldEventMsg,
};
use crate::player::PlayerId;
use crate::worker::JobPool;
use crate::world::{
    detached_section_payload, DetachedFold, PieceRange, ReplicaWorld, SectionContent, ServerWorld,
};

// --- capture files, written the way the format lays them out ---------------

struct CaptureFile {
    rel: &'static str,
    bytes: Vec<u8>,
}

fn info(piece: &[u8]) -> ClientPieceInfo {
    let head = parse_piece_head(piece).expect("a piece this test wrote");
    ClientPieceInfo {
        kind: head.kind,
        key: head.state_key(),
        batch: head.batch(),
        range: [0, piece.len() as u64],
        crc: head.body_crc,
        provisional: head.provisional,
    }
}

impl CaptureFile {
    fn new(rel: &'static str) -> Self {
        Self {
            rel,
            bytes: Vec::new(),
        }
    }

    /// A State record: head · pieces · envelope. Answers each piece's key
    /// and absolute range, and the record's.
    fn state(&mut self, tick: u64, pieces: Vec<Vec<u8>>) -> Snapshot {
        let at = self.bytes.len() as u64;
        let mut body = Vec::new();
        let mut infos = Vec::new();
        for p in &pieces {
            let mut i = info(p);
            i.range = [
                CAPTURE_RECORD_HEAD_LEN as u64 + body.len() as u64,
                p.len() as u64,
            ];
            infos.push(i);
            body.extend_from_slice(p);
        }
        let envelope = postcard::to_allocvec(&ClientStateEnvelope {
            revision: tick,
            since: None,
            tick,
            presented_tick: tick as f64,
            pieces: infos.clone(),
            absent: Vec::new(),
        })
        .unwrap();
        let envelope_at = (CAPTURE_RECORD_HEAD_LEN + body.len()) as u64;
        let head = CaptureRecordHead {
            kind: CaptureRecordKind::State,
            complete: true,
            len: envelope_at + envelope.len() as u64,
            envelope_at,
            envelope_len: envelope.len() as u64,
            envelope_crc: mod_api::capture::crc32(&envelope),
        };
        self.bytes.extend_from_slice(&write_record_head(&head));
        self.bytes.extend_from_slice(&body);
        self.bytes.extend_from_slice(&envelope);
        Snapshot {
            tick,
            record: [at, head.len],
            pieces: infos
                .iter()
                .map(|i| (i.key.unwrap(), [at + i.range[0], i.range[1]]))
                .collect(),
        }
    }

    /// A Frame record: head · envelope · pieces, in one piece of bytes.
    fn frame(&mut self, seq: u64, tick: u64, pieces: Vec<Vec<u8>>) -> [u64; 2] {
        let at = self.bytes.len() as u64;
        let mut envelope_len = 0usize;
        let envelope = loop {
            let mut offset = (CAPTURE_RECORD_HEAD_LEN + envelope_len) as u64;
            let infos = pieces
                .iter()
                .map(|p| {
                    let mut i = info(p);
                    i.range = [offset, p.len() as u64];
                    offset += p.len() as u64;
                    i
                })
                .collect();
            let envelope = postcard::to_allocvec(&ClientFrameEnvelope {
                seq,
                revision: tick,
                presented_tick: tick as f64 - 0.5,
                batches: vec![tick],
                view: None,
                pieces: infos,
                touched: Vec::new(),
                removed: Vec::new(),
            })
            .unwrap();
            if envelope.len() == envelope_len {
                break envelope;
            }
            envelope_len = envelope.len();
        };
        let len = (CAPTURE_RECORD_HEAD_LEN
            + envelope.len()
            + pieces.iter().map(Vec::len).sum::<usize>()) as u64;
        let head = CaptureRecordHead {
            kind: CaptureRecordKind::Frame,
            complete: true,
            len,
            envelope_at: CAPTURE_RECORD_HEAD_LEN as u64,
            envelope_len: envelope.len() as u64,
            envelope_crc: mod_api::capture::crc32(&envelope),
        };
        self.bytes.extend_from_slice(&write_record_head(&head));
        self.bytes.extend_from_slice(&envelope);
        for p in pieces {
            self.bytes.extend_from_slice(&p);
        }
        [at, len]
    }
}

#[derive(Clone, Debug)]
struct Snapshot {
    tick: u64,
    record: [u64; 2],
    pieces: Vec<(ClientStateKey, [u64; 2])>,
}

mod focused;
mod profile;

// --- a session that changes every tick --------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next(100) < percent
    }
}

fn replica() -> ReplicaWorld {
    ReplicaWorld::with_pool(0, 2, Arc::new(JobPool::inline()))
}

fn own_state(n: u64) -> SelfState {
    SelfState {
        conditions: Vec::new(),
        health: n as i32,
        mode: 0,
        denied_actions: Default::default(),
        effects: Vec::new(),
        inventory_revision: n / 20,
        inventory: None,
        eating: None,
        eating_off_hand: false,
        move_scale: 1.0,
        fly_scale: 1.0,
        held_pose_main: None,
        held_pose_off: None,
        held_display: [None, None],
        bone_poses: Vec::new(),
        animator: Default::default(),
        sleeping: None,
        sleep_bed: None,
        transform: None,
    }
}

/// A recorded session: an events file with one frame per tick, and a state
/// file with a full state record every few ticks.
struct Session {
    state: CaptureFile,
    events: CaptureFile,
    snapshots: Vec<Snapshot>,
    /// Tick `t`'s frame record, at index `t - 1`.
    frames: Vec<[u64; 2]>,
    /// After tick `t` (index `t - 1`): every section its frame wrote, stated
    /// in full — the restatement a capturing mod writes a frame after a change.
    restated: Vec<Snapshot>,
}

fn vocabulary() -> u64 {
    body::vocabulary_of(&crate::net::remap::local_name_tables())
}

fn state_pieces(truth: &ReplicaWorld, moment: &Moment, tick: u64) -> Vec<Vec<u8>> {
    let v = vocabulary();
    let piece = |key: ClientStateKey, value: &dyn erased::Encode| value.piece(key, v);
    let mut out = Vec::new();
    let mut columns: Vec<ChunkPos> = truth.data().columns.keys().copied().collect();
    columns.sort_by_key(|c| (c.cx, c.cz));
    let cy_min = SECTION_MIN_CY;
    let presence = ClientPresence {
        cy_min,
        columns: columns
            .iter()
            .map(|&c| {
                let mut word = 0u64;
                for sp in truth.column_sections(c) {
                    word |= 1 << (sp.cy - cy_min);
                }
                ClientColumnPresence {
                    pos: [c.cx, c.cz],
                    sections: if word == 0 { Vec::new() } else { vec![word] },
                }
            })
            .collect(),
    };
    out.push(piece(ClientStateKey::Presence, &presence));
    out.push(piece(
        ClientStateKey::Population,
        &ClientPopulation {
            mobs: Vec::new(),
            items: moment.items.keys().copied().collect(),
            players: Vec::new(),
        },
    ));
    out.push(piece(
        ClientStateKey::Clock,
        &ClientCapturedClock {
            tick,
            day_clock: moment.clock.map_or(0, |c| c.1),
        },
    ));
    for &c in &columns {
        if let Some(col) = truth.column_content(c) {
            out.push(piece(ClientStateKey::Column([c.cx, c.cz]), &col));
        }
    }
    for &c in &columns {
        for sp in truth.column_sections(c) {
            let (content, _) = truth.section_content(sp).unwrap();
            let payload = detached_section_payload(&content.section, content.draws.clone());
            out.push(piece(
                ClientStateKey::Section([sp.cx, sp.cy, sp.cz]),
                &payload,
            ));
        }
    }
    for row in moment.items.values() {
        out.push(piece(ClientStateKey::Item(row.id), row));
    }
    out.push(piece(
        ClientStateKey::Roster,
        &moment
            .roster
            .iter()
            .map(|(id, name)| ClientRosterEntry {
                player: mod_api::PlayerId(id.0),
                name: name.clone(),
            })
            .collect::<Vec<_>>(),
    ));
    out.push(piece(
        ClientStateKey::Environment,
        &ClientCapturedEnv {
            params: moment.env.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        },
    ));
    out.push(piece(
        ClientStateKey::Activity,
        &CapturedActivity {
            loops: moment.loops.values().copied().collect(),
            dig: moment.dig,
            open_chests: moment.open_chests.clone(),
        },
    ));
    if let Some(own) = &moment.viewer {
        out.push(piece(ClientStateKey::Viewer, own));
    }
    out
}

/// Bodies by kind, without naming each type at the call.
mod erased {
    use super::*;

    pub trait Encode {
        fn piece(&self, key: ClientStateKey, vocabulary: u64) -> Vec<u8>;
    }

    impl<T: serde::Serialize> Encode for T {
        fn piece(&self, key: ClientStateKey, vocabulary: u64) -> Vec<u8> {
            body::state_piece(key, self, vocabulary).expect("a body this test encodes")
        }
    }
}

fn frame_pieces(msgs: &[ServerToClient]) -> Vec<Vec<u8>> {
    let v = vocabulary();
    let mut out = Vec::new();
    for msg in msgs {
        match msg {
            ServerToClient::SectionData(p) => out.push(
                body::state_piece(
                    ClientStateKey::Section([p.pos.cx, p.pos.cy, p.pos.cz]),
                    &**p,
                    v,
                )
                .unwrap(),
            ),
            ServerToClient::ColumnData(p) => out.push(
                body::state_piece(ClientStateKey::Column([p.pos.cx, p.pos.cz]), p, v).unwrap(),
            ),
            ServerToClient::Tick(t) => {
                let (world, rows, rest) = body::split_batch(t);
                let mut key = [0u8; 12];
                key[0..8].copy_from_slice(&t.tick.to_le_bytes());
                for (kind, frame) in [
                    (
                        ClientPieceKind::BatchWorld,
                        body::encode_body(&world, true).unwrap(),
                    ),
                    (
                        ClientPieceKind::BatchRows,
                        body::encode_body(&rows, true).unwrap(),
                    ),
                    (
                        ClientPieceKind::BatchRest,
                        body::encode_body(&rest, true).unwrap(),
                    ),
                ] {
                    out.push(body::piece(kind, key, false, v, &frame));
                }
            }
            other => out.push(body::piece(
                ClientPieceKind::Message,
                [0; 12],
                false,
                v,
                &body::encode_body(other, true).unwrap(),
            )),
        }
    }
    out
}

/// What a replica ingests, as the presentation's naive twin applies it.
fn ingest(world: &mut ReplicaWorld, moment: &mut Moment, msgs: &[ServerToClient]) {
    let mut installed = Vec::new();
    for msg in msgs {
        match msg {
            ServerToClient::SectionData(p) => {
                installed.extend(world.install_remote_section_deferred((**p).clone()))
            }
            ServerToClient::ColumnData(p) => world.install_remote_column(p.clone()),
            ServerToClient::LightData(l) => world.install_remote_light(l.clone()),
            ServerToClient::SectionUnload { pos, .. } => {
                world.uninstall_remote_section(*pos);
            }
            ServerToClient::ColumnUnload { pos, .. } => {
                world.uninstall_remote_column(*pos);
            }
            ServerToClient::Tick(t) => {
                world.apply_remote_tick_terrain(
                    t.block_deltas().cloned().unwrap_or_default(),
                    t.block_draws().cloned().unwrap_or_default(),
                    t.cell_kv_deltas().cloned().unwrap_or_default(),
                );
                moment.batch(t);
            }
            other => moment.message(other),
        }
    }
    world.finish_remote_install_batch(&installed);
}

fn record_session(side: i32, last_tick: u64, seed: u64, full_every: u64) -> Session {
    let mut rng = Rng(seed);
    let pool = Arc::new(JobPool::inline());
    let mut server = ServerWorld::with_pool(0, 2, pool);
    let mut columns = Vec::new();
    for cz in 0..side {
        for cx in 0..side {
            let mut c = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    c.set_block(x, 64, z, Block::Stone);
                }
            }
            c.set_block(3, 90 + 16 * ((cx + cz) as usize % 2), 3, Block::Stone);
            server.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
            columns.push(ChunkPos::new(cx, cz));
        }
    }
    let arrive =
        |server: &ServerWorld, c: ChunkPos| -> Vec<ServerToClient> {
            let mut msgs = vec![ServerToClient::ColumnData(
                server.column_payload(c).unwrap(),
            )];
            let mut sections: Vec<SectionPos> = server
                .data()
                .sections
                .keys()
                .filter(|s| s.chunk_pos() == c)
                .copied()
                .collect();
            sections.sort_by_key(|s| s.cy);
            msgs.extend(sections.into_iter().map(|p| {
                ServerToClient::SectionData(Box::new(server.section_payload(p).unwrap()))
            }));
            msgs
        };
    let mut truth = replica();
    let mut moment = Moment::default();
    let mut session = Session {
        state: CaptureFile::new("state.pmc"),
        events: CaptureFile::new("events.pmc"),
        snapshots: Vec::new(),
        frames: Vec::new(),
        restated: Vec::new(),
    };
    let blocks = [Block::Stone, Block::Air, Block::Dirt];
    let mut unloaded_section: Option<SectionPos> = None;
    let mut unloaded_column: Option<ChunkPos> = None;
    for t in 1..=last_tick {
        let mut msgs = Vec::new();
        if t == 1 {
            for &c in &columns {
                msgs.extend(arrive(&server, c));
            }
            msgs.push(ServerToClient::PlayerJoined {
                id: PlayerId(3),
                name: "someone".into(),
            });
        } else {
            if t % 20 == 10 && unloaded_section.is_none() {
                let loaded: Vec<SectionPos> = truth.data().sections.keys().copied().collect();
                let mut loaded = loaded;
                loaded.sort_by_key(|p| (p.cx, p.cy, p.cz));
                let pos = loaded[rng.next(loaded.len() as u64) as usize];
                msgs.push(ServerToClient::SectionUnload {
                    pos,
                    cache_hash: None,
                });
                unloaded_section = Some(pos);
            } else if t % 20 == 15 {
                if let Some(pos) = unloaded_section.take() {
                    if truth.data().columns.contains_key(&pos.chunk_pos()) {
                        msgs.push(ServerToClient::SectionData(Box::new(
                            server.section_payload(pos).unwrap(),
                        )));
                    }
                }
            }
            if t % 25 == 12 && unloaded_column.is_none() {
                let c = columns[rng.next(columns.len() as u64) as usize];
                msgs.push(ServerToClient::ColumnUnload {
                    pos: c,
                    cache_hashes: Vec::new(),
                });
                unloaded_column = Some(c);
            } else if t % 25 == 20 {
                if let Some(c) = unloaded_column.take() {
                    msgs.extend(arrive(&server, c));
                }
            }
            if t % 7 == 0 {
                let mut loaded: Vec<SectionPos> = truth.data().sections.keys().copied().collect();
                loaded.sort_by_key(|p| (p.cx, p.cy, p.cz));
                if !loaded.is_empty() {
                    msgs.push(ServerToClient::LightData(LightPayload {
                        pos: loaded[rng.next(loaded.len() as u64) as usize],
                        skylight: SectionBytes(Arc::from(
                            vec![(t % 16) as u8; 4096].into_boxed_slice(),
                        )),
                        blocklight: None,
                    }));
                }
            }
            match t {
                30 => msgs.push(ServerToClient::PlayerJoined {
                    id: PlayerId(4),
                    name: "late".into(),
                }),
                45 => msgs.push(ServerToClient::PlayerLeft { id: PlayerId(3) }),
                _ => {}
            }
        }
        let side_blocks = side as u64 * 16;
        let block_deltas = (0..1 + rng.next(3))
            .map(|_| {
                // Most writes land near the ground; some clear a pillar top,
                // so a heightmap scans down.
                let y = if rng.chance(20) {
                    64
                } else {
                    63 + rng.next(4) as i32
                };
                BlockDelta {
                    pos: IVec3::new(
                        rng.next(side_blocks) as i32,
                        y,
                        rng.next(side_blocks) as i32,
                    ),
                    block_id: blocks[rng.next(3) as usize].id(),
                    fluid: None,
                    state: None,
                    cell_kv: Vec::new(),
                }
            })
            .collect();
        let cell_kv_deltas = if t % 3 == 0 {
            vec![CellKvDelta {
                pos: IVec3::new(
                    rng.next(side_blocks) as i32,
                    64,
                    rng.next(side_blocks) as i32,
                ),
                key: "fixture:mark".into(),
                value: Some(vec![t as u8]),
            }]
        } else {
            Vec::new()
        };
        // One dropped item at a time; its id moves on every 40 ticks, and
        // the old one leaves the lane as the new one enters it.
        let item = |t: u64| 1 + (t / 40);
        let mut update = TickUpdate::new(t, t * 3);
        update.push_list::<BlockDelta>(block_deltas);
        update.push_list(cell_kv_deltas);
        update.push(ItemLane {
            despawned: (item(t - 1) != item(t))
                .then(|| item(t - 1))
                .into_iter()
                .collect(),
            spawned: Default::default(),
            updated: vec![ItemStateRow {
                id: item(t),
                item_id: 1,
                count: 1,
                data: None,
                pos: petramond_math::world_pos::WorldPos::new(t as f64 * 0.1, 65.0, 0.0),
                spin: t as f32,
                flight: None,
            }]
            .into(),
        });
        update.push(own_state(t));
        if t.is_multiple_of(5) {
            update.push(vec![("fixture:sky".to_string(), [t as f32, 0.0, 0.0, 1.0])]);
        }
        if t % 30 < 10 {
            update.push(vec![IVec3::new(0, 65, 0)]);
        }
        if t == 40 {
            update.push(vec![WorldEventMsg::SpatialSound(SpatialSoundMsg::Stop {
                handle: 9,
            })]);
        }
        msgs.push(ServerToClient::Tick(Box::new(update)));
        ingest(&mut truth, &mut moment, &msgs);
        if t == 1 {
            // A loop the capturing client heard start before the capture did:
            // the state states it, and batch 40 stops it.
            moment.loops.insert(9, looping());
        }
        let frame = session.events.frame(t - 1, t, frame_pieces(&msgs));
        session.frames.push(frame);
        let written = sections_written(&msgs);
        let v = vocabulary();
        let mut columns_written: Vec<ChunkPos> = written.iter().map(|sp| sp.chunk_pos()).collect();
        columns_written.sort_by_key(|c| (c.cx, c.cz));
        columns_written.dedup();
        let restatement = columns_written
            .into_iter()
            .filter_map(|c| {
                let col = truth.column_content(c)?;
                Some(body::state_piece(ClientStateKey::Column([c.cx, c.cz]), &col, v).unwrap())
            })
            .chain(written.into_iter().filter_map(|sp| {
                let (content, _) = truth.section_content(sp)?;
                let payload = detached_section_payload(&content.section, content.draws);
                Some(
                    body::state_piece(ClientStateKey::Section([sp.cx, sp.cy, sp.cz]), &payload, v)
                        .unwrap(),
                )
            }))
            .collect();
        session.restated.push(session.state.state(t, restatement));
        if t % full_every == 1 || t == last_tick {
            let snapshot = session.state.state(t, state_pieces(&truth, &moment, t));
            session.snapshots.push(snapshot);
        }
    }
    session
}

/// A looping sound, as the command that started it.
fn looping() -> SpatialSoundMsg {
    SpatialSoundMsg::PlayAt {
        handle: 9,
        sound_id: 0,
        pos: petramond_math::world_pos::WorldPos::new(0.0, 65.0, 0.0),
        volume: 1.0,
        pitch: 1.0,
    }
}

/// The sections `msgs` write, in position order.
fn sections_written(msgs: &[ServerToClient]) -> Vec<SectionPos> {
    let key = |p: SectionPos| (p.cx, p.cy, p.cz);
    let cell = |p: IVec3| SectionPos::from_world(p.x, p.y, p.z).map(key);
    let mut out = BTreeSet::new();
    for msg in msgs {
        match msg {
            ServerToClient::SectionData(p) => {
                out.insert(key(p.pos));
            }
            ServerToClient::LightData(l) => {
                out.insert(key(l.pos));
            }
            ServerToClient::Tick(t) => {
                out.extend(
                    t.block_deltas()
                        .into_iter()
                        .flatten()
                        .filter_map(|d| cell(d.pos)),
                );
                out.extend(
                    t.cell_kv_deltas()
                        .into_iter()
                        .flatten()
                        .filter_map(|d| cell(d.pos)),
                );
            }
            _ => {}
        }
    }
    out.into_iter()
        .map(|(x, y, z)| SectionPos::new(x, y, z))
        .collect()
}

// --- the naive twin -----------------------------------------------------------

/// A replica applying every apply naively: the reference.
struct Naive {
    world: ReplicaWorld,
    moment: Moment,
    position: f64,
    released: Option<u64>,
    queue: VecDeque<Arc<Frame>>,
}

fn read_piece(files: &Files, range: PieceRange) -> Vec<u8> {
    let bytes = files.bytes(range.incarnation);
    bytes[range.offset as usize..(range.offset + range.len) as usize].to_vec()
}

impl Naive {
    fn new() -> Self {
        Self {
            world: replica(),
            moment: Moment::default(),
            position: 0.0,
            released: None,
            queue: VecDeque::new(),
        }
    }

    /// `pieces` are the state pieces in the order the apply lists them.
    fn apply(
        &mut self,
        files: &Files,
        pieces: &[(ClientStateKey, PieceRange)],
        stretch: &[Arc<Frame>],
        rest: VecDeque<Arc<Frame>>,
        at: f64,
    ) -> Result<(), String> {
        let vocab = Vocab::of(&crate::net::remap::local_name_tables());
        let mut decoded = Vec::new();
        for &(key, range) in pieces {
            let bytes = read_piece(files, range);
            let (head, body_bytes) = unpack::checked(&bytes, range, "naive", &vocab)?;
            decoded.push((key, head, body_bytes.to_vec()));
        }
        // Checks first: a failing apply changes nothing.
        let stated: BTreeSet<ClientStateKey> = pieces.iter().map(|p| p.0).collect();
        let mut moment = self.moment.clone();
        moment.previous = None;
        let mut presence = None;
        let mut population = None;
        for (_, head, b) in &decoded {
            match head.kind {
                ClientPieceKind::Section | ClientPieceKind::Column => {}
                _ => match unpack::decode_state(head, b, &vocab)?.1 {
                    StateBody::Presence(p) => presence = Some(p),
                    StateBody::Population(p) => population = Some(p),
                    other => moment.restate(&other),
                },
            }
        }
        if let Some(p) = &presence {
            for col in &p.columns {
                let c = ChunkPos::new(col.pos[0], col.pos[1]);
                if !(self.world.column_content(c).is_some()
                    || stated.contains(&ClientStateKey::Column(col.pos)))
                {
                    return Err(format!("presence lists column {:?}", col.pos));
                }
                for cy in SECTION_MIN_CY..SECTION_MIN_CY + 64 {
                    if col.has_section(p.cy_min, cy)
                        && !self
                            .world
                            .data()
                            .sections
                            .contains_key(&SectionPos::new(c.cx, cy, c.cz))
                        && !stated.contains(&ClientStateKey::Section([c.cx, cy, c.cz]))
                    {
                        return Err(format!("presence lists section {:?}", [c.cx, cy, c.cz]));
                    }
                }
            }
        }
        if let Some(p) = &population {
            moment.restrict(p)?;
        }
        let mut installed = Vec::new();
        for (_, head, b) in &decoded {
            match head.kind {
                ClientPieceKind::Section => {
                    let payload: SectionPayload = body::decode_body(b).map_err(|e| e.0)?;
                    installed.extend(self.world.install_remote_section_deferred(payload));
                }
                ClientPieceKind::Column => {
                    let payload: ColumnPayload = body::decode_body(b).map_err(|e| e.0)?;
                    self.world.install_remote_column(payload);
                }
                _ => {}
            }
        }
        self.world.finish_remote_install_batch(&installed);
        if let Some(p) = presence.clone() {
            let p = super::super::fold::PresenceIndex::new(p);
            let columns: Vec<ChunkPos> = self.world.data().columns.keys().copied().collect();
            for c in columns {
                if !p.lists_column(c) {
                    self.world.uninstall_remote_column(c);
                    continue;
                }
                for sp in self.world.column_sections(c) {
                    if !p.lists_section(sp) {
                        self.world.uninstall_remote_section(sp);
                    }
                }
            }
        }
        self.moment = moment;
        for f in stretch {
            self.release(f);
        }
        self.released = self.moment.clock.map(|(tick, _)| tick).or(self.released);
        self.queue = rest;
        self.position = at;
        Ok(())
    }

    fn release(&mut self, f: &Frame) {
        let mut installed = Vec::new();
        for item in &f.items {
            if let FrameItem::Terrain(edit) = item {
                installed.extend(self.world.apply_terrain_edit(edit.clone()));
            }
        }
        self.world.finish_remote_install_batch(&installed);
        self.moment.frame(f);
        self.released = self.released.max(f.newest_batch());
    }

    fn time(&mut self, at: f64) {
        self.position = at;
        let limit = at.floor() as u64 + 1;
        while self.queue.front().is_some_and(|f| f.due <= limit) {
            let f = self.queue.pop_front().unwrap();
            self.release(&f);
        }
    }
}

// --- the presented world, read back whole --------------------------------------

struct Files {
    by_incarnation: BTreeMap<u64, Vec<u8>>,
}

impl Files {
    fn bytes(&self, incarnation: u64) -> &[u8] {
        &self.by_incarnation[&incarnation]
    }
}

fn decode_away_section(files: &Files, r: PieceRange) -> SectionContent {
    let vocab = Vocab::of(&crate::net::remap::local_name_tables());
    let bytes = read_piece(files, r);
    let (head, b) = unpack::checked(&bytes, r, "test", &vocab).unwrap();
    match unpack::decode_state(&head, b, &vocab).unwrap().1 {
        StateBody::Section(s) => s,
        other => panic!("a section range holds {other:?}"),
    }
}

fn decode_away_column(files: &Files, r: PieceRange) -> ColumnPayload {
    let vocab = Vocab::of(&crate::net::remap::local_name_tables());
    let bytes = read_piece(files, r);
    let (head, b) = unpack::checked(&bytes, r, "test", &vocab).unwrap();
    match unpack::decode_state(&head, b, &vocab).unwrap().1 {
        StateBody::Column(c) => (*c).clone(),
        other => panic!("a column range holds {other:?}"),
    }
}

type Terrain = (
    BTreeMap<(i32, i32, i32), SectionContent>,
    BTreeMap<(i32, i32), ColumnPayload>,
);

fn world_terrain(world: &ReplicaWorld) -> Terrain {
    let mut sections = BTreeMap::new();
    let mut columns = BTreeMap::new();
    for &c in world.data().columns.keys() {
        if let Some(col) = world.column_content(c) {
            columns.insert((c.cx, c.cz), col);
        }
        for sp in world.column_sections(c) {
            sections.insert((sp.cx, sp.cy, sp.cz), world.section_content(sp).unwrap().0);
        }
    }
    (sections, columns)
}

/// Everything the presentation states: resident from the replica, away
/// from its index (reading ranges, applying what waits on them).
fn presented_terrain(p: &Presentation, replica: &ReplicaWorld, files: &Files) -> Terrain {
    let (mut sections, mut columns) = world_terrain(replica);
    for (&c, away) in &p.stated.away {
        let mut fold = DetachedFold::new();
        match &away.column {
            Some(Away::Range(r)) => fold.seed_column(decode_away_column(files, *r), Some(*r)),
            Some(Away::Held(col)) => fold.seed_column((**col).clone(), None),
            None => {}
        }
        for (&cy, s) in &away.sections {
            let sp = SectionPos::new(c.cx, cy, c.cz);
            match s {
                Away::Range(r) => fold.seed_section(sp, decode_away_section(files, *r), Some(*r)),
                Away::Held(s) => fold.seed_section(sp, s.clone(), None),
            }
        }
        for edit in &away.pending {
            fold.apply(edit.clone());
        }
        if let Some((col, _)) = fold.column(c) {
            assert!(
                columns.insert((c.cx, c.cz), col).is_none(),
                "column {c:?} both resident and away"
            );
        }
        for sp in fold.column_sections(c) {
            let (content, _) = fold.section(sp).unwrap();
            sections.insert((sp.cx, sp.cy, sp.cz), content);
        }
    }
    (sections, columns)
}

fn first_difference(a: &Terrain, b: &Terrain) -> Option<String> {
    let keys = |t: &Terrain| t.0.keys().copied().collect::<Vec<_>>();
    if keys(a) != keys(b) {
        let (ka, kb): (BTreeSet<_>, BTreeSet<_>) =
            (keys(a).into_iter().collect(), keys(b).into_iter().collect());
        return Some(format!(
            "sections only presented {:?}, only naive {:?}",
            ka.difference(&kb).collect::<Vec<_>>(),
            kb.difference(&ka).collect::<Vec<_>>()
        ));
    }
    for (k, s) in &a.0 {
        if !s.same(&b.0[k]) {
            return Some(format!("section {k:?} differs"));
        }
    }
    if a.1.keys().collect::<Vec<_>>() != b.1.keys().collect::<Vec<_>>() {
        return Some(format!(
            "columns {:?} vs {:?}",
            a.1.keys().collect::<Vec<_>>(),
            b.1.keys().collect::<Vec<_>>()
        ));
    }
    for (k, c) in &a.1 {
        if *c != b.1[k] {
            return Some(format!("column {k:?} differs"));
        }
    }
    None
}

/// The resident sections' `Arc`s.
fn resident_arcs(world: &ReplicaWorld) -> BTreeMap<(i32, i32, i32), SectionContent> {
    world_terrain(world).0
}

// --- driving -----------------------------------------------------------------

/// Drive until every call has taken effect and the window is loaded.
fn settle(p: &mut Presentation, replica: &mut ReplicaWorld) -> super::DriveOut {
    let mut out = super::DriveOut::default();
    loop {
        let step = p.drive(replica, 1.0 / 60.0);
        if step.jumped {
            out.jumped = true;
        }
        out.landed.extend(step.landed);
        out.failed.extend(step.failed);
        if step.jumped {
            out.messages = step.messages;
        } else {
            out.messages.extend(step.messages);
        }
        let idle = p.ops.is_empty() && p.entering.is_empty() && p.requested.is_empty();
        if idle {
            return out;
        }
        p.wait_for_reads(replica);
    }
}

struct Harness {
    _dir: TestScratchDir,
    session: Session,
    state_file: SourceFile,
    events_file: SourceFile,
    files: Files,
}

fn harness(tag: &str, side: i32, last_tick: u64, seed: u64, full_every: u64) -> Harness {
    let dir = TestScratchDir::new(tag);
    let session = record_session(side, last_tick, seed, full_every);
    std::fs::write(dir.join(session.state.rel), &session.state.bytes).unwrap();
    std::fs::write(dir.join(session.events.rel), &session.events.bytes).unwrap();
    let state_file = SourceFile::new(FileRef::in_bucket(&dir, session.state.rel));
    let events_file = SourceFile::new(FileRef::in_bucket(&dir, session.events.rel));
    let files = Files {
        by_incarnation: [
            (state_file.incarnation, session.state.bytes.clone()),
            (events_file.incarnation, session.events.bytes.clone()),
        ]
        .into_iter()
        .collect(),
    };
    Harness {
        _dir: dir,
        session,
        state_file,
        events_file,
        files,
    }
}

impl Harness {
    fn frames_from(&self, tick: u64, count: u64) -> FileRanges {
        let first = tick.max(1);
        let last = (first + count - 1).min(self.session.frames.len() as u64);
        if first > last {
            return FileRanges {
                file: self.events_file.clone(),
                ranges: Vec::new(),
            };
        }
        let start = self.session.frames[first as usize - 1][0];
        let end = {
            let f = self.session.frames[last as usize - 1];
            f[0] + f[1]
        };
        FileRanges {
            file: self.events_file.clone(),
            ranges: vec![[start, end - start]],
        }
    }

    fn decoded_frames(&self, from_tick: u64, count: u64) -> Vec<Arc<Frame>> {
        let vocab = Vocab::of(&crate::net::remap::local_name_tables());
        let first = from_tick.max(1);
        let last = (first + count - 1).min(self.session.frames.len() as u64);
        (first..=last.max(first - 1))
            .map(|t| {
                let [at, len] = self.session.frames[t as usize - 1];
                let bytes = &self.session.events.bytes[at as usize..(at + len) as usize];
                let range = PieceRange {
                    incarnation: self.events_file.incarnation,
                    offset: at,
                    len,
                };
                Arc::new(decode_frame(bytes, range, "events", &vocab).unwrap())
            })
            .collect()
    }

    fn range(&self, r: [u64; 2]) -> PieceRange {
        PieceRange {
            incarnation: self.state_file.incarnation,
            offset: r[0],
            len: r[1],
        }
    }
}

/// One apply as both sides take it.
struct ApplyCase {
    state: Vec<FileRanges>,
    /// The state pieces in the order the apply lists them.
    pieces: Vec<(ClientStateKey, PieceRange)>,
    events_from: u64,
    events_count: u64,
    at: f64,
}

fn random_apply(h: &Harness, rng: &mut Rng) -> ApplyCase {
    let snaps = &h.session.snapshots;
    let snap = &snaps[rng.next(snaps.len() as u64) as usize];
    // Each part is one ranges list and the pieces it lists, in order.
    let part = |snap: &Snapshot, rng: &mut Rng, whole: bool, keep: u64| {
        if whole {
            return (
                vec![snap.record],
                snap.pieces
                    .iter()
                    .map(|&(k, r)| (k, h.range(r)))
                    .collect::<Vec<_>>(),
            );
        }
        let mut ranges = Vec::new();
        let mut pieces = Vec::new();
        for &(k, r) in &snap.pieces {
            let whole_world = matches!(k, ClientStateKey::Presence | ClientStateKey::Population);
            if (whole_world && rng.chance(40)) || (!whole_world && rng.chance(keep)) {
                ranges.push(r);
                pieces.push((k, h.range(r)));
            }
        }
        (ranges, pieces)
    };
    let whole = rng.chance(30);
    let mut parts = vec![part(snap, rng, whole, 70)];
    if rng.chance(30) {
        // Another capture's pieces over (or under) these: the last wins.
        let other = &snaps[rng.next(snaps.len() as u64) as usize];
        parts.push(part(other, rng, false, 20));
        if rng.chance(50) {
            parts.reverse();
        }
    }
    let state = parts
        .iter()
        .map(|(ranges, _)| FileRanges {
            file: h.state_file.clone(),
            ranges: ranges.clone(),
        })
        .collect();
    let pieces = parts.into_iter().flat_map(|(_, p)| p).collect();
    let at = snap.tick as f64 + rng.next(12) as f64 + rng.next(4) as f64 * 0.25;
    ApplyCase {
        state,
        pieces,
        events_from: snap.tick + 1,
        events_count: rng.next(20) + 1,
        at,
    }
}

#[test]
fn every_apply_equals_the_naive_apply_and_keeps_unchanged_arcs() {
    let h = harness("present-contract", 4, 160, 0x9E37_79B9_7F4A_7C15, 10);
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let pool = Arc::new(JobPool::new(2));
    let mut p = Presentation::new(crate::net::remap::local_name_tables(), pool);
    let mut replica = replica();
    let mut naive = Naive::new();
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    let last = h.session.frames.len() as u64;
    for step in 0..220 {
        let window_moved = rng.chance(15);
        if window_moved {
            let side = 4;
            let w = (!rng.chance(10)).then(|| Window {
                center: ChunkPos::new(rng.next(side) as i32, rng.next(side) as i32),
                radius: rng.next(3) as i32,
            });
            p.set_window(w);
        }
        let before = resident_arcs(&replica);
        let (what, landed) = match rng.next(10) {
            0..=5 => {
                let case = random_apply(&h, &mut rng);
                let events = h.frames_from(case.events_from, case.events_count);
                let limit = case.at.floor() as u64 + 1;
                let all = h.decoded_frames(case.events_from, case.events_count);
                let split = all.iter().position(|f| f.due > limit).unwrap_or(all.len());
                let (stretch, rest) = all.split_at(split);
                p.push(Op::Apply {
                    id: step + 1,
                    state: case.state.clone(),
                    events: vec![events],
                    at: case.at,
                });
                let cancel = rng.chance(8);
                if cancel {
                    assert!(p.cancel(step + 1));
                }
                let out = settle(&mut p, &mut replica);
                if cancel {
                    ("cancelled", false)
                } else {
                    let expected = naive.apply(
                        &h.files,
                        &case.pieces,
                        stretch,
                        rest.iter().cloned().collect(),
                        case.at,
                    );
                    match (&expected, out.failed.first()) {
                        (Ok(()), None) => {
                            // The loops still sounding restart on the landed
                            // moment's newest batch.
                            let newest = out.messages.iter().rev().find_map(|m| match m {
                                ServerToClient::Tick(t) => {
                                    Some(t.events().cloned().unwrap_or_default())
                                }
                                _ => None,
                            });
                            let restarts: Vec<WorldEventMsg> =
                                crate::net::spatial_loops::loop_restarts(&p.moment.loops).collect();
                            assert_eq!(newest, Some(restarts.clone()), "step {step}: restarts");
                            if !restarts.is_empty() {
                                *counts.entry("loops restarted").or_default() += 1;
                            }
                            ("landed", true)
                        }
                        (Err(_), Some(_)) => ("failed alike", false),
                        (e, f) => panic!("step {step}: naive {e:?}, presented {f:?}"),
                    }
                }
            }
            6..=7 => {
                let at = if rng.chance(20) {
                    naive.position.floor() + rng.next(4) as f64 * 0.25
                } else {
                    (naive.position + rng.next(6) as f64 + 0.5).min(last as f64)
                };
                if at < naive.position && at.floor() != naive.position.floor() {
                    ("time refused", false)
                } else {
                    p.push(Op::Time(at));
                    naive.time(at);
                    settle(&mut p, &mut replica);
                    ("played", false)
                }
            }
            _ => {
                let from = naive
                    .queue
                    .back()
                    .map_or(naive.released.unwrap_or(0), |f| f.due)
                    + 1;
                if from > last {
                    ("nothing to queue", false)
                } else {
                    let count = rng.next(8) + 1;
                    p.push(Op::Queue(vec![h.frames_from(from, count)]));
                    naive.queue.extend(h.decoded_frames(from, count));
                    // What the position already needs releases at once.
                    naive.time(naive.position);
                    settle(&mut p, &mut replica);
                    ("queued", false)
                }
            }
        };
        *counts.entry(what).or_default() += 1;
        let presented = presented_terrain(&p, &replica, &h.files);
        let reference = world_terrain(&naive.world);
        assert_eq!(
            first_difference(&presented, &reference),
            None,
            "step {step}: after {what}"
        );
        assert_eq!(p.moment, naive.moment, "step {step}: moment after {what}");
        assert_eq!(p.position, naive.position, "step {step}: position");
        assert_eq!(p.released_through, naive.released, "step {step}: released");
        if landed && !window_moved {
            // A key whose content did not change keeps its `Arc`, its mesh and
            // its light, unless a neighbour's change remeshes it (marking a
            // section for a remesh is a write to it).
            let after = resident_arcs(&replica);
            let changed: BTreeSet<(i32, i32, i32)> = before
                .iter()
                .filter(|(k, prior)| after.get(k).is_none_or(|now| !prior.same(now)))
                .map(|(k, _)| *k)
                .chain(after.keys().filter(|k| !before.contains_key(k)).copied())
                .collect();
            let near_change = |(x, y, z): (i32, i32, i32)| {
                changed.iter().any(|&(a, b, c)| {
                    (a - x).abs() <= 1 && (b - y).abs() <= 1 && (c - z).abs() <= 1
                })
            };
            for (k, prior) in &before {
                if let Some(now) = after.get(k) {
                    if prior.same(now) && !near_change(*k) {
                        assert!(
                            Arc::ptr_eq(&prior.section, &now.section),
                            "step {step}: section {k:?} kept its content but not its Arc"
                        );
                        *counts.entry("arcs kept").or_default() += 1;
                    }
                }
            }
        }
    }
    assert!(
        counts.get("landed").copied().unwrap_or(0) >= 60
            && counts.get("played").copied().unwrap_or(0) >= 20
            && counts.get("queued").copied().unwrap_or(0) >= 10
            && counts.get("arcs kept").copied().unwrap_or(0) >= 100
            && counts.get("loops restarted").copied().unwrap_or(0) >= 5,
        "every path taken: {counts:?}"
    );
}

// --- a capturing mod's seek ----------------------------------------------------

/// What a seek to `tau` passes when the world presents through batch `from`
/// (`None`: nothing yet): the piece holding each key's content at the
/// batch closing the pair (its newest restatement, else the full state),
/// for every key that changed in between, and the pair's two frames.
fn seek(h: &Harness, from: Option<u64>, tau: f64) -> (Vec<FileRanges>, FileRanges) {
    let b = tau.floor() as u64 + 1;
    let events = h.frames_from(b - 1, 2);
    let snaps = &h.session.snapshots;
    let full = snaps
        .iter()
        .rev()
        .find(|s| s.tick <= b)
        .expect("a full state at or before");
    let Some(from) = from else {
        let mut ranges = vec![full.record];
        ranges.extend(
            h.session.restated[full.tick as usize..b as usize]
                .iter()
                .map(|r| r.record),
        );
        return (
            vec![FileRanges {
                file: h.state_file.clone(),
                ranges,
            }],
            events,
        );
    };
    let (lo, hi) = (from.min(b), from.max(b));
    let mut keys: BTreeSet<ClientStateKey> = BTreeSet::new();
    for r in &h.session.restated[lo as usize..hi as usize] {
        keys.extend(r.pieces.iter().map(|p| p.0));
    }
    let ranges = keys
        .iter()
        .filter_map(|k| {
            h.session.restated[full.tick as usize..b as usize]
                .iter()
                .rev()
                .chain(std::iter::once(full))
                .find_map(|r| r.pieces.iter().find(|p| p.0 == *k).map(|p| p.1))
        })
        .collect();
    (
        vec![FileRanges {
            file: h.state_file.clone(),
            ranges,
        }],
        events,
    )
}

fn window_over(side: i32) -> Option<Window> {
    Some(Window {
        center: ChunkPos::new(side / 2, side / 2),
        radius: side,
    })
}
