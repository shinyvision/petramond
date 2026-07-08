//! Server-side world streaming + per-connection terrain replication
//! (multiplayer Phase C2c-ii).
//!
//! Each pump the server streams its OWN world around every session's player
//! (`update_load_multi` + `poll` + `pump_light_bakes` — the drive that used to
//! live on the client's per-frame `tick_world`), then diffs each connection's
//! WANTED terrain shape against what it was already sent and emits
//! `ColumnData`/`SectionData`/`SectionUnload`/`ColumnUnload` messages. Over
//! the in-process pipe the payloads are `Arc` refcount bumps.
//!
//! The wanted/keep shapes are the streamer's own (`World::plan_terrain_send`
//! reuses `column_wanted`/`column_kept` over the anchor's facing target), and
//! the diff is INCREMENTAL: it reruns only when the anchor's quantized target
//! or the world's terrain-content revision moved (`World::terrain_send_key`),
//! or while a previous plan hit the per-pump budget.

use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::IVec3;
use crate::net::protocol::ServerToClient;
use crate::world::LoadAnchor;
use rustc_hash::FxHashSet;

use super::game::ServerGame;

/// Sections shipped per pump per connection. Local pipe: payloads are Arc
/// bumps, so this mostly bounds the replica's install burst per frame.
const TERRAIN_SECTIONS_PER_PUMP: usize = 128;

/// One connection's terrain replication state: what it currently holds.
#[derive(Default)]
pub(crate) struct TerrainSync {
    sent_columns: FxHashSet<ChunkPos>,
    sent_sections: FxHashSet<SectionPos>,
    /// The `terrain_send_key` the last executed diff ran under.
    last_send_key: Option<u64>,
    /// The last diff hit the section budget: keep scanning next pump.
    backlog: bool,
}

impl TerrainSync {
    /// Whether the cell's owning section was sent to this connection — the
    /// per-recipient block-delta filter.
    pub(crate) fn covers(&self, pos: IVec3) -> bool {
        SectionPos::from_world(pos.x, pos.y, pos.z)
            .is_some_and(|sp| self.sent_sections.contains(&sp))
    }
}

impl ServerGame {
    /// Every session's streaming anchor: the player's eye section + horizontal
    /// view direction (what the client camera used to feed
    /// `update_load_facing`).
    fn load_anchors(&self) -> Vec<LoadAnchor> {
        self.sessions
            .iter()
            .map(|sess| {
                let eye = sess.player.eye();
                let f = sess.player.forward();
                LoadAnchor {
                    cx: (eye.x.floor() as i32).div_euclid(16),
                    cy: (eye.y.floor() as i32).div_euclid(16),
                    cz: (eye.z.floor() as i32).div_euclid(16),
                    fx: f.x,
                    fz: f.z,
                }
            })
            .collect()
    }

    /// One pump's streaming step: drive the server world's own streaming, then
    /// emit terrain messages for EVERY session — each connection diffs its own
    /// wanted shape through its `TerrainSync`. `per_session` is indexed like
    /// `sessions` (the pump built it that way).
    pub(super) fn pump_streaming(&mut self, per_session: &mut [Vec<ServerToClient>]) {
        debug_assert_eq!(per_session.len(), self.sessions.len());
        let anchors = self.load_anchors();
        if anchors.is_empty() {
            return;
        }
        self.world.update_load_multi(&anchors);
        let _ = self.world.poll();
        // Headless worlds drain (and request — see `headless_relight`) their
        // light bakes here; nothing else pumps them without a mesh queue.
        self.world.pump_light_bakes();
        let relit = self.world.take_light_ship_log();

        for (s, msgs) in per_session.iter_mut().enumerate() {
            // Light refreshes BEFORE this pump's terrain: a section shipping
            // right now isn't in the sent set yet and its payload already
            // carries the fresh cubes — no duplicate message.
            self.send_light_for(s, &relit, msgs);
            self.send_terrain_for(s, anchors[s], msgs);
        }
    }

    /// Ship `LightData` for every freshly-baked section this connection
    /// already holds. Sections it never received are skipped — their eventual
    /// `SectionData` carries current light.
    fn send_light_for(&self, s: usize, relit: &[SectionPos], msgs: &mut Vec<ServerToClient>) {
        for &sp in relit {
            if !self.sessions[s].terrain.sent_sections.contains(&sp) {
                continue;
            }
            if let Some(p) = self.world.light_payload(sp) {
                msgs.push(ServerToClient::LightData(p));
            }
        }
    }

    /// Diff session `s`'s wanted terrain against its sent sets and append the
    /// resulting messages: unloads first, then each new section preceded by
    /// its (re-freshed) column payload — column-before-section is the install
    /// contract, and re-shipping the column keeps the replica's heightmap and
    /// summaries current as more of the column lands server-side.
    fn send_terrain_for(&mut self, s: usize, anchor: LoadAnchor, msgs: &mut Vec<ServerToClient>) {
        let key = self.world.terrain_send_key(anchor);
        let sync = &mut self.sessions[s].terrain;
        if sync.last_send_key == Some(key) && !sync.backlog {
            return;
        }
        let plan = self.world.plan_terrain_send(
            anchor,
            &sync.sent_columns,
            &sync.sent_sections,
            TERRAIN_SECTIONS_PER_PUMP,
        );
        sync.last_send_key = Some(key);
        sync.backlog = plan.saturated;

        for cp in plan.drop_columns {
            sync.sent_columns.remove(&cp);
            sync.sent_sections.retain(|sp| sp.chunk_pos() != cp);
            msgs.push(ServerToClient::ColumnUnload(cp));
        }
        for sp in plan.drop_sections {
            sync.sent_sections.remove(&sp);
            msgs.push(ServerToClient::SectionUnload(sp));
        }

        // One column payload per batch that ships sections of that column.
        let mut refreshed: Vec<ChunkPos> = Vec::new();
        for sp in plan.sections {
            let cp = sp.chunk_pos();
            if !refreshed.contains(&cp) {
                let Some(column) = self.world.column_payload(cp) else {
                    continue; // column evicted mid-plan: skip its sections too
                };
                refreshed.push(cp);
                sync.sent_columns.insert(cp);
                msgs.push(ServerToClient::ColumnData(column));
            }
            let Some(section) = self.world.section_payload(sp) else {
                continue;
            };
            sync.sent_sections.insert(sp);
            msgs.push(ServerToClient::SectionData(section));
        }
    }
}
