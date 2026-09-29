//! Server-side world streaming + per-connection terrain replication.
//!
//! Each pump the server streams its OWN world around every session's player
//! (`update_load_multi` + `poll` + `pump_light_bakes`), then diffs each connection's
//! WANTED terrain shape against what it was already sent and emits
//! `ColumnData`/`SectionData`/`SectionUnload`/`ColumnUnload` messages. Over
//! the in-process pipe the payloads are `Arc` refcount bumps.
//!
//! The wanted/keep shapes are the streamer's own (`World::plan_terrain_send`
//! reuses `column_wanted`/`column_kept` over the anchor's facing target). Each
//! connection keeps its plan LIVE: a full diff runs only when the anchor's
//! quantized target moves (or the world's plan epoch does); between those every
//! section whose sendability may have changed — loaded, unloaded, lit, marked
//! dirty, finality flipped — reaches the plan as a world SEND EVENT and is
//! re-evaluated on its own, so a pump costs the events, never the loaded set.
//! A full-recompute oracle (`verify_plan`) asserts the live plan equals the
//! full diff after every sync in tests and under `PETRAMOND_SEND_PLAN_ORACLE`.

use crate::net::protocol::{SectionCacheClaim, ServerToClient, SECTION_CACHE_CAP};
use crate::world::{LoadAnchor, SendEvents, SentSections, ServerWorld, TerrainSendPlan};
use petramond_math::math::IVec3;
use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::world::load_targets::LoadTarget;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeSet;

use super::game::ServerGame;

/// Sections considered per pump per connection. The message-batch allowance is
/// normally the tighter bound.
#[cfg(test)]
const TERRAIN_SECTIONS_PER_PUMP: usize = 128;

/// Remote batches stay small enough to apply inside one client frame even when
/// section reconstruction is expensive. Several may be in flight to cover RTT.
const MAX_BATCH_MSGS: usize = 96;

/// Two-deep window lets server bank the next batch while client applies the current one (acks land
/// once per client frame). At 64 msgs/batch loopback delivery capped at ~3.8k sections/s, below
/// RD32 sprint-flight demand (~5k/s), and replica coverage collapsed under load. Wider window
/// raises the ceiling above demand; pacing stays client-driven. Window is still the hard bound on
/// the unbounded channel.
///
/// 192 (was 96): loopback batches use `Arc` payloads and are cheap to apply, so the remote
/// per-frame sizing doesn't bind here. Spawn's 3x3 columns are ~2x96 msgs, so the wider batch
/// lands them in one frame instead of trickling.
const LOCAL_MAX_BATCH_MSGS: usize = 192;

const MAX_UNACKED_BATCHES: u32 = 4;
const LOCAL_MAX_UNACKED_BATCHES: u32 = 2;

const INITIAL_CLIENT_RATE: f32 = 1600.0;

const CLIENT_RATE_BOUNDS: (f32, f32) = (50.0, 50_000.0);

/// Outbound-queue slots the streamer must always leave free for tick
/// updates, unloads, and broadcasts. Below this headroom a remote session
/// ships NO streaming messages this pump (terrain AND light — light defers
/// through `TerrainSync::pending_light`) and retries once the writer drains —
/// a full queue disconnects the client (`TcpServerConn::send`), so streaming,
/// which the SERVER rate-controls, must never be what fills it.
const STREAM_QUEUE_RESERVE: usize = crate::net::connection::SERVER_QUEUE_MSGS / 4;

fn stream_allowance(queue_room: usize) -> usize {
    queue_room.saturating_sub(STREAM_QUEUE_RESERVE)
}

#[cfg(test)]
fn terrain_budget(allowance: usize) -> usize {
    TERRAIN_SECTIONS_PER_PUMP.min(allowance / 2)
}

type PosKey = (i32, i32, i32);
/// A planned section under its ship key: nearest-first, then by position, so the ship order is
/// total and reproducible.
type PlanKey = (i64, PosKey);

#[inline]
fn pos_key(sp: SectionPos) -> PosKey {
    (sp.cx, sp.cy, sp.cz)
}

#[inline]
fn key_pos(k: PosKey) -> SectionPos {
    SectionPos::new(k.0, k.1, k.2)
}

fn plan_oracle_enabled() -> bool {
    if cfg!(test) {
        return true;
    }
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PETRAMOND_SEND_PLAN_ORACLE").is_some())
}

pub struct TerrainSync {
    sent_columns: FxHashSet<ChunkPos>,
    sent_column_revisions: FxHashMap<ChunkPos, u64>,
    sent: SentSections,
    pending_light: FxHashSet<SectionPos>,
    /// The live plan: every loaded, wanted, unsent, ship-final section under its
    /// nearest-first key, and what left the keep shape or the server. Consumed
    /// incrementally across paced batches.
    planned: BTreeSet<PlanKey>,
    planned_keys: FxHashMap<SectionPos, i64>,
    planned_drop_sections: BTreeSet<PosKey>,
    planned_drop_columns: BTreeSet<ChunkPos>,
    /// (target key, world plan epoch) the plan was built under; `None` until the
    /// first sync. A change rebuilds the plan from the whole world.
    planned_under: Option<(u64, u64)>,
    /// Sections this connection un-sent itself (cache misses) since its last
    /// sync: they re-enter the plan through the same re-evaluation as a world
    /// send event.
    rechecks: Vec<SectionPos>,
    /// What this connection's client holds in its SECTION CACHE, by the
    /// server-domain content hash vouched at unload (value: hash + insertion
    /// stamp). Seeded from the Join manifest, grown by unload emission,
    /// consumed by the `SectionCached`-vs-`SectionData` decision. Capped at
    /// [`SECTION_CACHE_CAP`] with oldest-first eviction — the client runs the
    /// same policy over the same ordered unload stream, so the two maps stay
    /// aligned without eviction chatter; residual drift (a client that
    /// declined to park, a divergent claim) heals through `SectionCacheMiss`.
    client_cache: FxHashMap<SectionPos, (u64, u64)>,
    client_cache_stamp: u64,
    unacked_batches: u32,
    max_unacked: u32,
    client_rate: f32,
    /// Fractional message budget banked from `client_rate × dt` each pump
    /// (capped at one max batch); a batch spends its message count from it.
    /// Starts FULL: a joining connection gets one max-size opening batch
    /// (still window-bounded to a single in-flight batch until the first
    /// ack) instead of trickling `INITIAL_CLIENT_RATE × dt` messages per
    /// pump while the player stares at an empty spawn.
    batch_quota: f32,
    batch_limit: usize,
    window_limit: u32,
}

impl Default for TerrainSync {
    fn default() -> Self {
        TerrainSync {
            sent_columns: FxHashSet::default(),
            sent_column_revisions: FxHashMap::default(),
            sent: SentSections::default(),
            pending_light: FxHashSet::default(),
            planned: BTreeSet::new(),
            planned_keys: FxHashMap::default(),
            planned_drop_sections: BTreeSet::new(),
            planned_drop_columns: BTreeSet::new(),
            planned_under: None,
            rechecks: Vec::new(),
            client_cache: FxHashMap::default(),
            client_cache_stamp: 0,
            unacked_batches: 0,
            max_unacked: 1,
            client_rate: INITIAL_CLIENT_RATE,
            batch_quota: MAX_BATCH_MSGS as f32,
            batch_limit: MAX_BATCH_MSGS,
            window_limit: MAX_UNACKED_BATCHES,
        }
    }
}

impl TerrainSync {
    pub fn covers(&self, pos: IVec3) -> bool {
        SectionPos::from_world(pos.x, pos.y, pos.z).is_some_and(|sp| self.sent.contains(sp))
    }

    pub fn apply_batch_ack(&mut self, messages_per_second: f32) {
        self.unacked_batches = self.unacked_batches.saturating_sub(1);
        self.max_unacked = self.window_limit;
        if messages_per_second.is_finite() {
            self.client_rate =
                messages_per_second.clamp(CLIENT_RATE_BOUNDS.0, CLIENT_RATE_BOUNDS.1);
        }
    }

    fn note_client_cached(&mut self, sp: SectionPos, hash: u64) {
        let stamp = self.client_cache_stamp;
        self.client_cache_stamp += 1;
        self.client_cache.insert(sp, (hash, stamp));
        if self.client_cache.len() > SECTION_CACHE_CAP {
            if let Some(oldest) = self
                .client_cache
                .iter()
                .min_by_key(|(_, (_, stamp))| *stamp)
                .map(|(p, _)| *p)
            {
                self.client_cache.remove(&oldest);
            }
        }
    }

    pub fn seed_client_cache(&mut self, claims: &[SectionCacheClaim]) {
        for claim in claims.iter().take(SECTION_CACHE_CAP) {
            self.note_client_cached(claim.pos, claim.hash);
        }
    }

    pub fn handle_cache_miss(&mut self, pos: SectionPos) {
        self.client_cache.remove(&pos);
        if self.sent.remove(pos) {
            self.pending_light.remove(&pos);
            self.rechecks.push(pos);
        }
    }

    fn configure_loopback(&mut self, loopback: bool) {
        let (batch_limit, window_limit) = if loopback {
            (LOCAL_MAX_BATCH_MSGS, LOCAL_MAX_UNACKED_BATCHES)
        } else {
            (MAX_BATCH_MSGS, MAX_UNACKED_BATCHES)
        };
        self.batch_limit = batch_limit;
        self.window_limit = window_limit;
        self.max_unacked = self.max_unacked.min(window_limit).max(1);
        self.batch_quota = self.batch_quota.min(batch_limit as f32);
    }

    fn set_planned(&mut self, sp: SectionPos, key: Option<i64>) {
        let old = self.planned_keys.get(&sp).copied();
        if old == key {
            return;
        }
        if let Some(old) = old {
            self.planned.remove(&(old, pos_key(sp)));
        }
        match key {
            Some(key) => {
                self.planned.insert((key, pos_key(sp)));
                self.planned_keys.insert(sp, key);
            }
            None => {
                self.planned_keys.remove(&sp);
            }
        }
    }

    fn set_drop_section(&mut self, sp: SectionPos, drop: bool) {
        if drop {
            self.planned_drop_sections.insert(pos_key(sp));
        } else {
            self.planned_drop_sections.remove(&pos_key(sp));
        }
    }

    fn install_plan(&mut self, plan: TerrainSendPlan, under: (u64, u64)) {
        self.planned.clear();
        self.planned_keys.clear();
        for (key, sp) in plan.section_keys {
            self.planned.insert((key, pos_key(sp)));
            self.planned_keys.insert(sp, key);
        }
        self.planned_drop_sections = plan.drop_sections.into_iter().map(pos_key).collect();
        self.planned_drop_columns = plan.drop_columns.into_iter().collect();
        self.planned_under = Some(under);
    }

    /// A SENT column through the drop rule. Returns whether its membership moved.
    fn reevaluate_column_drop(&mut self, world: &ServerWorld, target: LoadTarget, cp: ChunkPos) {
        let drop = self.sent_columns.contains(&cp) && world.column_drop_due(target, cp);
        if drop {
            self.planned_drop_columns.insert(cp);
        } else {
            self.planned_drop_columns.remove(&cp);
        }
    }

    /// One section through the ship rule and the drop rule against this
    /// connection's sent sets. Idempotent: every call reads the CURRENT world,
    /// so repeated or out-of-order events converge on the same plan.
    fn reevaluate_section(
        &mut self,
        world: &ServerWorld,
        target: LoadTarget,
        underground: bool,
        sp: SectionPos,
    ) {
        let cp = sp.chunk_pos();
        self.reevaluate_column_drop(world, target, cp);
        if self.sent.contains(sp) {
            self.set_planned(sp, None);
            let drop =
                !self.planned_drop_columns.contains(&cp) && world.section_drop_due(target, sp);
            self.set_drop_section(sp, drop);
        } else {
            self.set_drop_section(sp, false);
            self.set_planned(sp, world.section_send_key(target, underground, sp));
        }
    }

    /// Everything about one column: its drop membership and every section of it
    /// that is sent, loaded or planned.
    fn reevaluate_column(
        &mut self,
        world: &ServerWorld,
        target: LoadTarget,
        underground: bool,
        cp: ChunkPos,
    ) {
        self.reevaluate_column_drop(world, target, cp);
        let loaded = world
            .data()
            .section_column_cys
            .get(&cp)
            .copied()
            .unwrap_or(0);
        let held = self.sent.column_bits(cp);
        for cy in petramond_world::world::data::WorldData::column_section_range() {
            let sp = SectionPos::new(cp.cx, cy, cp.cz);
            let bit = 1u32 << (cy - petramond_world::chunk::SECTION_MIN_CY);
            if (loaded | held) & bit != 0 || self.planned_keys.contains_key(&sp) {
                self.reevaluate_section(world, target, underground, sp);
            }
        }
    }

    /// Bring the plan up to date with the world: a full diff when the target or
    /// the world's plan epoch moved, otherwise one re-evaluation per send event
    /// and per own recheck. Runs EVERY pump for every session, before and
    /// independently of emission — events are taken from the world once, so a
    /// session that skips them (no allowance, a full window) would lose them.
    pub(crate) fn sync_plan(
        &mut self,
        world: &ServerWorld,
        anchor: LoadAnchor,
        events: &SendEvents,
    ) {
        let (target, underground) = world.send_frame(anchor);
        let under = (world.terrain_target_key(anchor), world.plan_epoch());
        if self.planned_under != Some(under) {
            let plan = world.plan_terrain_send(anchor, &self.sent_columns, &self.sent, usize::MAX);
            self.install_plan(plan, under);
        } else {
            for &cp in &events.columns {
                self.reevaluate_column(world, target, underground, cp);
            }
            for i in 0..events.sections.len() + self.rechecks.len() {
                let sp = events
                    .sections
                    .get(i)
                    .copied()
                    .unwrap_or_else(|| self.rechecks[i - events.sections.len()]);
                self.reevaluate_section(world, target, underground, sp);
            }
        }
        self.rechecks.clear();
        if plan_oracle_enabled() {
            self.verify_plan(world, anchor);
        }
    }

    /// The full-recompute oracle: the live plan must equal the full diff, in
    /// ship order. Always on in tests; `PETRAMOND_SEND_PLAN_ORACLE` turns it on
    /// in a running game (appbench, a dedicated server) to audit the event
    /// producers against real streaming.
    pub(crate) fn verify_plan(&self, world: &ServerWorld, anchor: LoadAnchor) {
        let full = world.plan_terrain_send(anchor, &self.sent_columns, &self.sent, usize::MAX);
        let live: Vec<(i64, PosKey)> = self.planned.iter().copied().collect();
        let want: Vec<(i64, PosKey)> = full
            .section_keys
            .iter()
            .map(|&(k, sp)| (k, pos_key(sp)))
            .collect();
        assert_plan_eq("sections", &live, &want);
        assert_eq!(
            self.planned_keys.len(),
            self.planned.len(),
            "send plan key index out of step with the ordered plan"
        );
        let live: Vec<PosKey> = self.planned_drop_sections.iter().copied().collect();
        let mut want: Vec<PosKey> = full.drop_sections.iter().copied().map(pos_key).collect();
        want.sort_unstable();
        assert_plan_eq("drop_sections", &live, &want);
        let live: Vec<ChunkPos> = self.planned_drop_columns.iter().copied().collect();
        let mut want = full.drop_columns;
        want.sort_unstable();
        assert_plan_eq("drop_columns", &live, &want);
    }

    fn plan_is_empty(&self) -> bool {
        self.planned.is_empty()
            && self.planned_drop_sections.is_empty()
            && self.planned_drop_columns.is_empty()
    }

    /// Emit this connection's terrain from its plan: unloads first, then each
    /// new section preceded by its (re-freshed) column payload —
    /// column-before-section is the install contract, and re-shipping the
    /// column keeps the replica's heightmap and summaries current as more of
    /// the column lands server-side.
    ///
    /// EVERY emitted message pays from `allowance`, unloads included — a
    /// server-side eviction sweep can drop thousands of sent sections at
    /// once, and an unpaced unload burst overflows the connection queue just
    /// like unpaced terrain did. Deferring emission is always safe: the plan
    /// and the sent sets move ONLY for messages actually emitted, so a paused
    /// connection resumes exactly where it stopped.
    pub(crate) fn emit_terrain(
        &mut self,
        world: &ServerWorld,
        allowance: &mut usize,
        msgs: &mut Vec<ServerToClient>,
    ) {
        if *allowance == 0 || self.plan_is_empty() {
            return;
        }
        while let Some(&cp) = self.planned_drop_columns.first() {
            if *allowance == 0 {
                break;
            }
            self.planned_drop_columns.remove(&cp);
            *allowance -= 1;
            self.sent_columns.remove(&cp);
            self.sent_column_revisions.remove(&cp);
            let dropped = self.sent.take_column(cp);
            let mut cache_hashes = Vec::new();
            for sp in dropped {
                if self.pending_light.contains(&sp) {
                    continue;
                }
                if let Some(payload) = world.section_payload(sp) {
                    let hash = payload.content_hash();
                    self.note_client_cached(sp, hash);
                    cache_hashes.push((sp.cy, hash));
                }
            }
            self.pending_light.retain(|sp| sp.chunk_pos() != cp);
            msgs.push(ServerToClient::ColumnUnload {
                pos: cp,
                cache_hashes,
            });
        }
        while self.planned_drop_columns.is_empty() {
            let Some(&k) = self.planned_drop_sections.first() else {
                break;
            };
            let sp = key_pos(k);
            let cp = sp.chunk_pos();
            let column_revision = world.data().column_payload_revision(cp);
            let fresh_column =
                self.sent_column_revisions.get(&cp).copied() != Some(column_revision);
            if *allowance < 1 + usize::from(fresh_column) {
                break;
            }
            if fresh_column {
                if let Some(column) = world.column_payload(cp) {
                    self.sent_column_revisions.insert(cp, column_revision);
                    *allowance -= 1;
                    msgs.push(ServerToClient::ColumnData(column));
                }
            }
            self.planned_drop_sections.remove(&k);
            *allowance -= 1;
            self.sent.remove(sp);
            let cache_hash = (!self.pending_light.remove(&sp))
                .then(|| world.section_payload(sp).map(|p| p.content_hash()))
                .flatten();
            if let Some(hash) = cache_hash {
                self.note_client_cached(sp, hash);
            }
            msgs.push(ServerToClient::SectionUnload {
                pos: sp,
                cache_hash,
            });
        }

        while self.planned_drop_columns.is_empty() && self.planned_drop_sections.is_empty() {
            let Some(&(key, k)) = self.planned.first() else {
                break;
            };
            let sp = key_pos(k);
            let cp = sp.chunk_pos();
            let column_revision = world.data().column_payload_revision(cp);
            let fresh_column =
                self.sent_column_revisions.get(&cp).copied() != Some(column_revision);
            if *allowance < 1 + usize::from(fresh_column) {
                break;
            }
            self.planned.remove(&(key, k));
            self.planned_keys.remove(&sp);
            if fresh_column {
                let Some(column) = world.column_payload(cp) else {
                    continue;
                };
                self.sent_columns.insert(cp);
                self.sent_column_revisions.insert(cp, column_revision);
                *allowance -= 1;
                msgs.push(ServerToClient::ColumnData(column));
            }
            let Some(section) = world.section_payload(sp) else {
                continue;
            };
            self.sent.insert(sp);
            *allowance -= 1;
            if let Some(&(hash, _)) = self.client_cache.get(&sp) {
                self.client_cache.remove(&sp);
                if section.content_hash() == hash {
                    msgs.push(ServerToClient::SectionCached { pos: sp, hash });
                    continue;
                }
            }
            msgs.push(ServerToClient::SectionData(Box::new(section)));
        }
    }
}

fn assert_plan_eq<T: PartialEq + std::fmt::Debug>(what: &str, live: &[T], want: &[T]) {
    if live == want {
        return;
    }
    let at = live
        .iter()
        .zip(want)
        .position(|(a, b)| a != b)
        .unwrap_or(live.len().min(want.len()));
    panic!(
        "incremental send plan diverged from the full plan in {what}: live {} entries, full {} \
         entries, first difference at {at}: live {:?} vs full {:?}",
        live.len(),
        want.len(),
        live.get(at),
        want.get(at)
    );
}

impl ServerGame {
    #[cfg(any(test, feature = "test-support"))]
    pub fn mark_section_sent_for_test(&mut self, session: usize, cell: IVec3) {
        let section = SectionPos::from_world(cell.x, cell.y, cell.z).expect("valid test cell");
        let sync = &mut self.sessions[session].transport.terrain;
        sync.sent.insert(section);
        sync.rechecks.push(section);
    }

    fn load_anchors(&self) -> Vec<LoadAnchor> {
        self.sessions
            .iter()
            .map(|sess| {
                let eye = sess.player.eye();
                LoadAnchor {
                    cx: (eye.x.floor() as i32).div_euclid(16),
                    cy: (eye.y.floor() as i32).div_euclid(16),
                    cz: (eye.z.floor() as i32).div_euclid(16),
                    radius: sess.transport.view_radius,
                }
            })
            .collect()
    }

    /// One pump's streaming step: drive the server world's own streaming, then
    /// emit terrain messages for EVERY session — each connection diffs its own
    /// wanted shape through its `TerrainSync`. `per_session` and `queue_room`
    /// (free outbound-queue slots per connection; `usize::MAX` = unbounded)
    /// are indexed like `sessions` (the pump built them that way); `dt` is
    /// the pump's wall-clock step, feeding the batch quota.
    ///
    /// Refreshes are BANKED first (against the pre-terrain sent set — a
    /// section terrain ships this same pump carries current light in its own
    /// payload, so it must not double-ship) but EMITTED last: terrain owns
    /// the allowance. During a load, seam rebakes outnumber new sections
    /// ~2:1, and light-first spent most of a saturated writer's throughput
    /// correcting light on terrain the client already had while the world
    /// itself trickled. Deferring is also cheaper in total: a frontier
    /// section rebakes several times as its neighbours land, and pending
    /// entries fetch their payload at SHIP time, so those rebakes coalesce
    /// into one message once the writer frees up.
    pub(super) fn pump_streaming(
        &mut self,
        dt: f32,
        per_session: &mut [Vec<ServerToClient>],
        queue_room: &[usize],
    ) {
        debug_assert_eq!(per_session.len(), self.sessions.len());
        debug_assert_eq!(queue_room.len(), self.sessions.len());
        // Taken even with no session: a joining connection rebuilds its plan from the world, so
        // events logged while nobody listened are noise that must not pile up.
        let events = self.world.take_send_events();
        let anchors = self.load_anchors();
        if anchors.is_empty() {
            return;
        }
        // Emission runs before world's streaming step. Moved anchor's plan drops terrain against
        // the new target, but unload_far hasn't evicted it yet, so unload vouching still has
        // content to hash. World's own products (sections, relit chunks) land one pump later.
        let relit = self.world.take_light_ship_log();
        let local_at_zero = self.sessions.has_local_session();
        for (s, msgs) in per_session.iter_mut().enumerate() {
            self.bank_light_refreshes(s, &relit);
            let sync = &mut self.sessions[s].transport.terrain;
            sync.configure_loopback(s == 0 && local_at_zero);
            sync.sync_plan(&self.world, anchors[s], &events);
            self.send_batch_for(s, dt, queue_room[s], msgs);
        }

        self.world.update_load_multi(&anchors);
        let _ = self.world.poll();
        self.world.pump_light_bakes();
    }

    fn send_batch_for(
        &mut self,
        s: usize,
        dt: f32,
        queue_room: usize,
        msgs: &mut Vec<ServerToClient>,
    ) {
        let sync = &mut self.sessions[s].transport.terrain;
        sync.batch_quota =
            (sync.batch_quota + sync.client_rate * dt.max(0.0)).min(sync.batch_limit as f32);
        if sync.unacked_batches >= sync.max_unacked {
            return;
        }
        let quota = sync.batch_quota as usize;
        let mut allowance = quota
            .min(sync.batch_limit)
            .min(stream_allowance(queue_room));
        if allowance == 0 {
            return;
        }
        let start = msgs.len();
        sync.emit_terrain(&self.world, &mut allowance, msgs);
        self.send_light_for(s, &mut allowance, msgs);
        let count = msgs.len() - start;
        if count == 0 {
            return;
        }
        msgs.insert(start, ServerToClient::StreamBatchStart);
        msgs.push(ServerToClient::StreamBatchEnd {
            count: count as u32,
        });
        let sync = &mut self.sessions[s].transport.terrain;
        sync.batch_quota -= count as f32;
        sync.unacked_batches += 1;
    }

    fn bank_light_refreshes(&mut self, s: usize, relit: &[SectionPos]) {
        let sync = &mut self.sessions[s].transport.terrain;
        for &sp in relit {
            if sync.sent.contains(sp) {
                sync.pending_light.insert(sp);
            }
        }
    }

    fn send_light_for(&mut self, s: usize, allowance: &mut usize, msgs: &mut Vec<ServerToClient>) {
        let sync = &mut self.sessions[s].transport.terrain;
        if sync.pending_light.is_empty() {
            return;
        }
        let batch: Vec<SectionPos> = sync
            .pending_light
            .iter()
            .take(*allowance)
            .copied()
            .collect();
        for sp in batch {
            sync.pending_light.remove(&sp);
            let Some(p) = self.world.light_payload(sp) else {
                continue;
            };
            *allowance -= 1;
            msgs.push(ServerToClient::LightData(p));
        }
    }
}

#[cfg(test)]
mod tests;
