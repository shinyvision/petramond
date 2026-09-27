use std::sync::Arc;

use mod_api::capture::{
    check_piece, record_envelope, CapturePieceHead, ClientCapturedClock, ClientCapturedEnv,
    ClientCapturedSession, ClientEnvelope, ClientFrameEnvelope, ClientPieceKind, ClientPopulation,
    ClientPresence, ClientRosterEntry, ClientStateKey,
};

use super::body::{
    decode_body, BatchRest, BatchRows, BatchWorld, CapturedActivity, CapturedCues, ColumnPayload,
    NameTables, SectionPayload, ServerToClient, ViewCue,
};
use crate::net::protocol::{
    ItemLane, ItemStateRow, MobLane, MobStateRow, PlayerLane, PlayerStateRow, SelfState,
    TickSection, TickUpdate, WorldEventMsg,
};
use crate::net::remap::IdRemap;
use crate::net::PROTOCOL_VERSION;
use crate::world::{decode_section_payload, PieceRange, SectionContent, TerrainEdit};

#[derive(Clone)]
pub struct Vocab {
    pub vocabulary: u64,
    pub remap: Option<Arc<IdRemap>>,
}

impl Vocab {
    pub fn of(tables: &NameTables) -> Self {
        let remap = IdRemap::build(tables);
        Self {
            vocabulary: super::body::vocabulary_of(tables),
            remap: (!remap.is_identity()).then(|| Arc::new(remap)),
        }
    }

    fn remap(&self, msg: &mut ServerToClient) {
        if let Some(remap) = &self.remap {
            remap.remap_to_client(msg);
        }
    }
}

pub fn at(label: &str, offset: u64, why: impl std::fmt::Display) -> String {
    format!("{label}@{offset}: {why}")
}

pub fn checked<'a>(
    bytes: &'a [u8],
    range: PieceRange,
    label: &str,
    vocab: &Vocab,
) -> Result<(CapturePieceHead, &'a [u8]), String> {
    let (head, body) = check_piece(bytes).map_err(|e| at(label, range.offset, e))?;
    if head.piece_len() != range.len {
        return Err(at(
            label,
            range.offset,
            format!(
                "a piece of {} bytes where the range holds {}",
                head.piece_len(),
                range.len
            ),
        ));
    }
    if head.protocol != PROTOCOL_VERSION {
        return Err(at(
            label,
            range.offset,
            format!(
                "captured under protocol v{}; this build reads v{PROTOCOL_VERSION}",
                head.protocol
            ),
        ));
    }
    if head.vocabulary != vocab.vocabulary {
        return Err(at(
            label,
            range.offset,
            "a piece of another id vocabulary than the presentation's tables",
        ));
    }
    Ok((head, body))
}

#[derive(Clone, Debug)]
pub enum StateBody {
    Section(SectionContent),
    Column(Arc<ColumnPayload>),
    Presence(ClientPresence),
    Population(ClientPopulation),
    Session(ClientCapturedSession),
    Tables(NameTables),
    Clock(ClientCapturedClock),
    Mob(MobStateRow),
    Item(ItemStateRow),
    Player(PlayerStateRow),
    Roster(Vec<ClientRosterEntry>),
    Environment(ClientCapturedEnv),
    Activity(CapturedActivity),
    Viewer(SelfState),
}

fn documented<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    let (flags, bytes) = mod_api::capture::split_frame(body).map_err(|e| e.to_string())?;
    if flags & 1 != 0 {
        return Err("a documented body stored compressed".into());
    }
    postcard::from_bytes(bytes).map_err(|e| e.to_string())
}

fn opaque<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    decode_body(body).map_err(|e| e.0)
}

pub fn decode_state(
    head: &CapturePieceHead,
    body: &[u8],
    vocab: &Vocab,
) -> Result<(ClientStateKey, StateBody), String> {
    let key = head
        .state_key()
        .ok_or_else(|| format!("a {:?} piece where a state piece belongs", head.kind))?;
    let decoded = match head.kind {
        ClientPieceKind::Section => {
            let payload: SectionPayload = opaque(body)?;
            let expect = match key {
                ClientStateKey::Section(p) => p,
                _ => unreachable!("a section head names a section key"),
            };
            if [payload.pos.cx, payload.pos.cy, payload.pos.cz] != expect {
                return Err("a section piece whose body states another section".into());
            }
            StateBody::Section(section_content(payload, vocab)?)
        }
        ClientPieceKind::Column => {
            let mut msg = ServerToClient::ColumnData(opaque(body)?);
            vocab.remap(&mut msg);
            let ServerToClient::ColumnData(payload) = msg else {
                unreachable!("remapping keeps the variant")
            };
            StateBody::Column(Arc::new(payload))
        }
        ClientPieceKind::Presence => StateBody::Presence(documented(body)?),
        ClientPieceKind::Population => StateBody::Population(documented(body)?),
        ClientPieceKind::Session => StateBody::Session(documented(body)?),
        ClientPieceKind::Clock => StateBody::Clock(documented(body)?),
        ClientPieceKind::Roster => StateBody::Roster(documented(body)?),
        ClientPieceKind::Environment => StateBody::Environment(documented(body)?),
        ClientPieceKind::Tables => StateBody::Tables(opaque(body)?),
        ClientPieceKind::Mob => {
            let rows = BatchRows {
                mobs: MobLane::from(vec![opaque::<MobStateRow>(body)?]),
                ..Default::default()
            };
            let row = rows_tick(rows, vocab)
                .mobs()
                .and_then(|l| l.iter().next().cloned());
            StateBody::Mob(row.ok_or("a mob of a kind this build does not know")?)
        }
        ClientPieceKind::Item => {
            let rows = BatchRows {
                items: ItemLane::from(vec![opaque::<ItemStateRow>(body)?]),
                ..Default::default()
            };
            let row = rows_tick(rows, vocab)
                .items()
                .and_then(|l| l.iter().next().cloned());
            StateBody::Item(row.ok_or("an item this build does not know")?)
        }
        ClientPieceKind::Player => {
            let rows = BatchRows {
                players: PlayerLane::from(vec![opaque::<PlayerStateRow>(body)?]),
                ..Default::default()
            };
            let row = rows_tick(rows, vocab)
                .players()
                .and_then(|l| l.iter().next().cloned());
            StateBody::Player(row.ok_or("a player row this build cannot read")?)
        }
        ClientPieceKind::Activity => {
            let mut activity: CapturedActivity = opaque(body)?;
            if let Some(remap) = &vocab.remap {
                activity.loops = std::mem::take(&mut activity.loops)
                    .into_iter()
                    .filter_map(|l| {
                        let mut event = WorldEventMsg::SpatialSound(l);
                        remap.apply(&mut event).then_some(event)
                    })
                    .filter_map(|e| match e {
                        WorldEventMsg::SpatialSound(s) => Some(s),
                        _ => None,
                    })
                    .collect();
            }
            StateBody::Activity(activity)
        }
        ClientPieceKind::Viewer => {
            let own: SelfState = opaque(body)?;
            let mut msg = ServerToClient::Tick(Box::new(TickUpdate::new(0, 0).with(own)));
            vocab.remap(&mut msg);
            let ServerToClient::Tick(t) = msg else {
                unreachable!("remapping keeps the variant")
            };
            let own = t.self_state().cloned();
            StateBody::Viewer(own.ok_or("a viewer state this build cannot read")?)
        }
        other => return Err(format!("a {other:?} piece where a state piece belongs")),
    };
    Ok((key, decoded))
}

pub fn section_content(payload: SectionPayload, vocab: &Vocab) -> Result<SectionContent, String> {
    let payload = remapped_section(payload, vocab);
    decode_section_payload(payload).ok_or_else(|| "a malformed section body".to_string())
}

fn remapped_section(payload: SectionPayload, vocab: &Vocab) -> SectionPayload {
    if vocab.remap.is_none() {
        return payload;
    }
    let mut msg = ServerToClient::SectionData(Box::new(payload));
    vocab.remap(&mut msg);
    let ServerToClient::SectionData(p) = msg else {
        unreachable!("remapping keeps the variant")
    };
    *p
}

fn rows_tick(rows: BatchRows, vocab: &Vocab) -> TickUpdate {
    let mut msg = ServerToClient::Tick(Box::new(
        TickUpdate::new(0, 0)
            .with(rows.mobs)
            .with(rows.items)
            .with(rows.players),
    ));
    vocab.remap(&mut msg);
    let ServerToClient::Tick(t) = msg else {
        unreachable!("remapping keeps the variant")
    };
    *t
}

#[derive(Clone, Debug)]
pub enum FrameItem {
    Terrain(TerrainEdit),
    Message(ServerToClient),
    Batch(Arc<TickUpdate>),
    Cues(CapturedCues),
    View(Box<ViewCue>),
}

#[derive(Clone, Debug)]
pub struct Frame {
    pub record: PieceRange,
    pub presented_tick: f64,
    pub due: u64,
    pub batches: Vec<u64>,
    pub items: Vec<FrameItem>,
}

impl Frame {
    pub fn newest_batch(&self) -> Option<u64> {
        self.batches.iter().copied().max()
    }
}

pub fn decode_frame(
    bytes: &[u8],
    record: PieceRange,
    label: &str,
    vocab: &Vocab,
) -> Result<Frame, String> {
    let envelope: ClientFrameEnvelope = match record_envelope(bytes) {
        Ok(ClientEnvelope::Frame(f)) => f,
        Ok(ClientEnvelope::State(_)) => {
            return Err(at(
                label,
                record.offset,
                "a state record where a frame belongs",
            ))
        }
        Err(e) => return Err(at(label, record.offset, e)),
    };
    let mut items = Vec::with_capacity(envelope.pieces.len());
    let mut batch: Option<PendingBatch> = None;
    for info in &envelope.pieces {
        let [start, len] = info.range;
        let end = start
            .checked_add(len)
            .filter(|&e| e <= bytes.len() as u64)
            .ok_or_else(|| {
                at(
                    label,
                    record.offset,
                    "a listed piece reaches past its record",
                )
            })?;
        let range = PieceRange {
            incarnation: record.incarnation,
            offset: record.offset + start,
            len,
        };
        let (head, body) = checked(&bytes[start as usize..end as usize], range, label, vocab)?;
        if head.body_crc != info.crc {
            return Err(at(
                label,
                range.offset,
                "a piece its envelope lists with another CRC",
            ));
        }
        let fail = |why: String| at(label, range.offset, why);
        if let Some(tick) = head.batch() {
            if batch.as_ref().is_some_and(|b| b.tick != tick) {
                if let Some(done) = batch.take() {
                    done.finish(vocab, &mut items);
                }
            }
            let b = batch.get_or_insert_with(|| PendingBatch::new(tick));
            match head.kind {
                ClientPieceKind::BatchWorld => b.world(opaque(body).map_err(fail)?),
                ClientPieceKind::BatchRows => b.rows(opaque(body).map_err(fail)?),
                _ => b.rest(opaque(body).map_err(fail)?),
            }
            continue;
        }
        if let Some(done) = batch.take() {
            done.finish(vocab, &mut items);
        }
        let item = match head.kind {
            ClientPieceKind::Section => {
                let payload: SectionPayload = opaque(body).map_err(fail)?;
                let payload = remapped_section(payload, vocab);
                FrameItem::Terrain(TerrainEdit::Section(Arc::new(payload), Some(range)))
            }
            ClientPieceKind::Column => {
                let mut msg = ServerToClient::ColumnData(opaque(body).map_err(fail)?);
                vocab.remap(&mut msg);
                let ServerToClient::ColumnData(payload) = msg else {
                    unreachable!("remapping keeps the variant")
                };
                FrameItem::Terrain(TerrainEdit::Column(Arc::new(payload), Some(range)))
            }
            ClientPieceKind::Message => {
                let mut msg: ServerToClient = opaque(body).map_err(fail)?;
                vocab.remap(&mut msg);
                match msg {
                    ServerToClient::LightData(l) => FrameItem::Terrain(TerrainEdit::Light(l)),
                    ServerToClient::SectionUnload { pos, .. } => {
                        FrameItem::Terrain(TerrainEdit::SectionUnload(pos))
                    }
                    ServerToClient::ColumnUnload { pos, .. } => {
                        FrameItem::Terrain(TerrainEdit::ColumnUnload(pos))
                    }
                    ServerToClient::SectionData(p) => {
                        FrameItem::Terrain(TerrainEdit::Section(Arc::new(*p), None))
                    }
                    ServerToClient::ColumnData(p) => {
                        FrameItem::Terrain(TerrainEdit::Column(Arc::new(p), None))
                    }
                    other => FrameItem::Message(other),
                }
            }
            ClientPieceKind::Cues => {
                let mut cues: CapturedCues = opaque(body).map_err(fail)?;
                if let Some(remap) = &vocab.remap {
                    cues.events.retain_mut(|e| remap.apply(e));
                }
                FrameItem::Cues(cues)
            }
            ClientPieceKind::View => {
                let mut view: ViewCue = opaque(body).map_err(fail)?;
                if let Some(remap) = &vocab.remap {
                    remap.apply(&mut view);
                }
                FrameItem::View(Box::new(view))
            }
            other => return Err(fail(format!("a {other:?} piece inside a frame"))),
        };
        items.push(item);
    }
    if let Some(done) = batch.take() {
        done.finish(vocab, &mut items);
    }
    let floor = envelope.presented_tick.max(0.0).floor() as u64;
    Ok(Frame {
        record,
        presented_tick: envelope.presented_tick,
        due: envelope
            .batches
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .max(floor),
        batches: envelope.batches,
        items,
    })
}

struct PendingBatch {
    tick: u64,
    world: BatchWorld,
    rows: BatchRows,
    rest: BatchRest,
}

impl PendingBatch {
    fn new(tick: u64) -> Self {
        Self {
            tick,
            world: BatchWorld::default(),
            rows: BatchRows::default(),
            rest: BatchRest {
                tick,
                ..Default::default()
            },
        }
    }

    fn world(&mut self, w: BatchWorld) {
        self.world.block_deltas.extend(w.block_deltas);
        self.world.cell_kv_deltas.extend(w.cell_kv_deltas);
        self.world.block_draws.extend(w.block_draws);
    }

    fn rows(&mut self, r: BatchRows) {
        join_lane(&mut self.rows.mobs, r.mobs);
        join_lane(&mut self.rows.items, r.items);
        join_lane(&mut self.rows.players, r.players);
    }

    fn rest(&mut self, r: BatchRest) {
        self.rest.clock = r.clock;
        self.rest.sections.extend(r.sections);
    }

    fn finish(self, vocab: &Vocab, items: &mut Vec<FrameItem>) {
        let BatchWorld {
            block_deltas,
            cell_kv_deltas,
            block_draws,
        } = self.world;
        let mut t = TickUpdate::new(self.tick, self.rest.clock);
        t.push_list(block_deltas);
        t.push_list(block_draws);
        t.push_list(cell_kv_deltas);
        let BatchRows {
            mobs: mob_lane,
            items: item_lane,
            players: player_lane,
        } = self.rows;
        if !mob_lane.is_empty() {
            t.push(mob_lane);
        }
        if !item_lane.is_empty() {
            t.push(item_lane);
        }
        if !player_lane.is_empty() {
            t.push(player_lane);
        }
        t.sections.extend(self.rest.sections);
        let mut msg = ServerToClient::Tick(Box::new(t));
        vocab.remap(&mut msg);
        let ServerToClient::Tick(mut t) = msg else {
            unreachable!("remapping keeps the variant")
        };
        let (mut deltas, mut draws, mut kv) = (Vec::new(), Vec::new(), Vec::new());
        t.sections.retain_mut(|section| match section {
            TickSection::BlockDeltas(d) => {
                deltas.append(d);
                false
            }
            TickSection::BlockDraws(d) => {
                draws.append(d);
                false
            }
            TickSection::CellKvDeltas(d) => {
                kv.append(d);
                false
            }
            _ => true,
        });
        if !(deltas.is_empty() && draws.is_empty() && kv.is_empty()) {
            items.push(FrameItem::Terrain(TerrainEdit::Tick { deltas, draws, kv }));
        }
        items.push(FrameItem::Batch(Arc::new(*t)));
    }
}

fn join_lane<R: Clone, K>(
    lane: &mut crate::net::protocol::EntityLane<R, K>,
    more: crate::net::protocol::EntityLane<R, K>,
) {
    let join = |a: &crate::net::protocol::RowSet<R>, b: crate::net::protocol::RowSet<R>| {
        a.iter()
            .cloned()
            .chain(b.iter().cloned())
            .collect::<Vec<R>>()
            .into()
    };
    lane.despawned.extend(more.despawned);
    lane.spawned = join(&lane.spawned, more.spawned);
    lane.updated = join(&lane.updated, more.updated);
}
