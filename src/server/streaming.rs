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

/// Outbound-queue slots terrain must always leave free for tick updates,
/// light refreshes, and broadcasts. Below this headroom a remote session
/// ships no terrain this pump and retries once the writer drains — a full
/// queue disconnects the client (`TcpServerConn::send`), so terrain, which
/// the SERVER rate-controls, must never be what fills it.
const TERRAIN_QUEUE_RESERVE: usize = crate::net::connection::SERVER_QUEUE_MSGS / 4;

/// This pump's section budget for a connection with `queue_room` free
/// outbound slots: the flat cap, shrunk so the plan (each section plus its
/// possible column refresh = up to two messages) fits in the room above the
/// reserve. The local pipe passes `usize::MAX` (unbounded channel).
fn terrain_budget(queue_room: usize) -> usize {
    TERRAIN_SECTIONS_PER_PUMP.min(queue_room.saturating_sub(TERRAIN_QUEUE_RESERVE) / 2)
}

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
    /// wanted shape through its `TerrainSync`. `per_session` and `queue_room`
    /// (free outbound-queue slots per connection; `usize::MAX` = unbounded)
    /// are indexed like `sessions` (the pump built them that way).
    pub(super) fn pump_streaming(
        &mut self,
        per_session: &mut [Vec<ServerToClient>],
        queue_room: &[usize],
    ) {
        debug_assert_eq!(per_session.len(), self.sessions.len());
        debug_assert_eq!(queue_room.len(), self.sessions.len());
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
            self.send_terrain_for(s, anchors[s], terrain_budget(queue_room[s]), msgs);
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
    ///
    /// A zero `budget` (the connection's queue needs to drain) skips the pump
    /// WITHOUT touching `last_send_key`: a key is only ever marked done by a
    /// plan that actually ran under it, so paused terrain always resumes.
    fn send_terrain_for(
        &mut self,
        s: usize,
        anchor: LoadAnchor,
        budget: usize,
        msgs: &mut Vec<ServerToClient>,
    ) {
        if budget == 0 {
            return;
        }
        let key = self.world.terrain_send_key(anchor);
        let sync = &mut self.sessions[s].terrain;
        if sync.last_send_key == Some(key) && !sync.backlog {
            return;
        }
        let plan =
            self.world
                .plan_terrain_send(anchor, &sync.sent_columns, &sync.sent_sections, budget);
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
            msgs.push(ServerToClient::SectionData(Box::new(section)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::connection::SERVER_QUEUE_MSGS;
    use std::time::{Duration, Instant};

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

    /// The budget shuts off below the reserve and never exceeds the flat cap
    /// (the local pipe's `usize::MAX` room included) — the two edges that
    /// keep pacing from either starving a healthy client or overflowing a
    /// slow one.
    #[test]
    fn terrain_budget_pauses_below_the_reserve_and_stays_capped() {
        assert_eq!(terrain_budget(0), 0);
        assert_eq!(terrain_budget(TERRAIN_QUEUE_RESERVE), 0);
        assert!(terrain_budget(TERRAIN_QUEUE_RESERVE + 64) > 0);
        assert_eq!(terrain_budget(SERVER_QUEUE_MSGS), TERRAIN_SECTIONS_PER_PUMP);
        assert_eq!(terrain_budget(usize::MAX), TERRAIN_SECTIONS_PER_PUMP);
    }

    /// A remote session whose outbound queue reports no room gets NO terrain
    /// while other recipients keep streaming, and the pause must not mark the
    /// paused session's diff as done: once room returns, the withheld terrain
    /// ships. This is the anti-kick contract — bulk terrain paces itself to
    /// the connection instead of overflowing its bounded queue.
    #[test]
    fn starved_sessions_pause_terrain_and_resume_without_losing_any() {
        let (mut server, _) = crate::game::session::build_session("", 1, 2);
        let player = crate::game::session::spawn_player(server.world.seed);
        let s = server.add_session_for_test(player);
        let remote_id = server.sessions[s].id;

        // Starve the remote queue until the LOCAL session has received
        // terrain — the world demonstrably had shippable sections, and the
        // starved session got none of them.
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut local_terrain = 0usize;
        while local_terrain == 0 {
            assert!(Instant::now() < deadline, "no terrain became shippable");
            let out = server.pump_tagged(0.01, &mut Vec::new(), &[(remote_id, 0)]);
            local_terrain += count_terrain(&out.msgs);
            for (id, msgs) in &out.remote {
                assert_eq!(*id, remote_id);
                assert_eq!(
                    count_terrain(msgs),
                    0,
                    "a zero-headroom session must receive no terrain"
                );
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        // Room returns: the withheld terrain ships to the remote session.
        let mut remote_terrain = 0usize;
        while remote_terrain == 0 {
            assert!(Instant::now() < deadline, "paused terrain never resumed");
            let out = server.pump_tagged(0.01, &mut Vec::new(), &[(remote_id, SERVER_QUEUE_MSGS)]);
            remote_terrain += out
                .remote
                .iter()
                .map(|(_, msgs)| count_terrain(msgs))
                .sum::<usize>();
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
