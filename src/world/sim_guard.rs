//! Streaming-finality guard: the simulation must never mutate — or decide
//! anything from reads of — a section whose streamed content is not FINAL.
//!
//! Sections stream in asynchronously (gen job → install → saved-overlay
//! apply). Until that finishes, world reads there LIE (an absent cave-band
//! section reads as air) and writes RACE (a synchronously materialized base is
//! clobbered by the late gen result; a mutated base can be persisted and then
//! shadow the player's on-disk record forever). One fluid flow against a
//! half-streamed neighbourhood is enough to mark sections `modified` and
//! freeze the accident into the save.
//!
//! Enforced at the tick dispatch points (scheduled ticks, block updates,
//! random ticks — see `world::tick`) and at the write choke points
//! (`set_block_world`, `set_fluid_world`, `materialize_section`,
//! `harvest_section_snapshot`):
//!
//! - a cell is simulated only when every section its behaviour can read
//!   (±[`SIM_READ_REACH`] cells) is stream-final;
//! - a section is written only when it has no in-flight gen job and no
//!   in-flight saved overlay ([`World::stream_writable`]).
//!
//! Stream-final per section: loaded with nothing in flight; or absent with a
//! TRUTHFUL summary (`Empty` sky, `FullOpaque` deep stone, `FullWater` ocean
//! interior — physics reads match what would generate, and a write
//! materializes exactly that base). Absent `Mixed`/`Unknown` (reads lie) and
//! any in-flight state are not final.
//!
//! Gated work is not lost: work blocked on an IN-FLIGHT state retries
//! [`SIM_RETRY_DELAY`] ticks later (in-flight states resolve within ticks);
//! work blocked on genuinely unloaded terrain is dropped and re-armed by the
//! on-load fluid kick when that terrain streams in
//! (`world::stream::queue_loaded_section_fluid_updates`).

use crate::world::{ServerWorld, World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::{SectionPos, SECTION_SIZE};
use petramond_world::section::SectionSummary;

pub(super) const SIM_READ_REACH: i32 = 5;

pub(super) const SIM_RETRY_DELAY: u64 = 5;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(super) enum SimReadiness {
    Ready,
    /// Blocked on an in-flight section (gen/overlay); resolves within ticks.
    Wait,
    /// Blocked on terrain that is not coming under the current load target.
    /// The on-load fluid kick re-arms the flow if it ever streams in.
    Drop,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum StreamState {
    Final,
    InFlight,
    Unresolved,
}

impl<S: WorldSide> World<S> {
    /// Block read for the mod ABI (`GetBlock`/`GetBlocks`): `None` not only
    /// for unloaded sections but also while the section's streamed content is
    /// not final (in-flight gen job or saved overlay). During that window a
    /// plain read LIES — the generated base is visible before the player's
    /// saved record overlays it — and a mod deciding from it corrupts its own
    /// state (the kitchen oven pruned every placed oven from its world-KV
    /// list this way). `None` means "state frozen, try again later", which is
    /// exactly right for both truly-unloaded and still-streaming sections.
    pub fn block_if_stream_final(&self, wx: i32, wy: i32, wz: i32) -> Option<Block> {
        let sp = SectionPos::from_world(wx, wy, wz)?;
        if !self.data.stream_writable(sp) {
            return None;
        }
        self.data.block_if_loaded(wx, wy, wz)
    }

    pub fn section_stream_final_at(&self, wx: i32, wy: i32, wz: i32) -> bool {
        SectionPos::from_world(wx, wy, wz)
            .is_some_and(|sp| self.data.sections.contains_key(&sp) && self.data.stream_writable(sp))
    }
}

impl ServerWorld {
    pub fn physics_cell_final_at(&self, wx: i32, wy: i32, wz: i32) -> bool {
        SectionPos::from_world(wx, wy, wz)
            .is_some_and(|sp| self.section_stream_state(sp, false) == StreamState::Final)
    }

    fn section_stream_state(&self, sp: SectionPos, quiet: bool) -> StreamState {
        if !SectionPos::cy_in_range(sp.cy) {
            return StreamState::Final;
        }
        if !quiet && !self.data.stream_writable(sp) {
            return StreamState::InFlight;
        }
        if self.data.sections.contains_key(&sp) {
            return StreamState::Final;
        }
        let cp = sp.chunk_pos();
        if let Some(col) = self.side.gen.column_gen.get(&cp) {
            if self.data.saved_section_contains(sp) {
                return StreamState::InFlight;
            }
            return match col.section_summary(sp.cy) {
                SectionSummary::Empty | SectionSummary::FullOpaque | SectionSummary::FullWater => {
                    StreamState::Final
                }
                _ => StreamState::Unresolved,
            };
        }
        if self.data.columns.contains_key(&cp) {
            return StreamState::Final;
        }
        if self.side.gen.pending.contains_key(&cp) {
            return StreamState::InFlight;
        }
        if self.column_wanted_by_any_target(cp) {
            return StreamState::InFlight;
        }
        StreamState::Unresolved
    }

    pub(super) fn sim_readiness_at(&self, pos: IVec3) -> SimReadiness {
        let quiet = self.side.gen.pending_sections.is_empty()
            && self.side.gen.awaited_overlays.is_empty()
            && self.side.gen.pending_overlays.is_empty();
        let s = SECTION_SIZE as i32;
        let (x0, x1) = (
            (pos.x - SIM_READ_REACH).div_euclid(s),
            (pos.x + SIM_READ_REACH).div_euclid(s),
        );
        let (y0, y1) = (
            (pos.y - SIM_READ_REACH).div_euclid(s),
            (pos.y + SIM_READ_REACH).div_euclid(s),
        );
        let (z0, z1) = (
            (pos.z - SIM_READ_REACH).div_euclid(s),
            (pos.z + SIM_READ_REACH).div_euclid(s),
        );
        let mut waiting = false;
        for cy in y0..=y1 {
            for cz in z0..=z1 {
                for cx in x0..=x1 {
                    match self.section_stream_state(SectionPos::new(cx, cy, cz), quiet) {
                        StreamState::Final => {}
                        StreamState::InFlight => waiting = true,
                        StreamState::Unresolved => return SimReadiness::Drop,
                    }
                }
            }
        }
        if waiting {
            SimReadiness::Wait
        } else {
            SimReadiness::Ready
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mob::Mob;
    use crate::world::testutil::flat_server_world;
    use crate::world::ServerWorld;
    use petramond_math::math::IVec3;
    use petramond_math::world_pos::WorldPos;
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_SIZE};

    fn run_ticks(w: &mut ServerWorld, n: u32) {
        let recipes = petramond_world::crafting::Recipes::default();
        for _ in 0..n {
            w.game_tick(&recipes);
        }
    }

    #[test]
    fn water_waits_for_an_in_flight_neighbor_section_then_flows() {
        let mut w = flat_server_world();
        let in_flight = SectionPos::new(1, 4, 0);
        w.insert_pending_section(in_flight);

        w.set_block_world(15, 65, 8, Block::Water);
        run_ticks(&mut w, 60);
        assert_eq!(
            w.data.chunk_block(16, 65, 8),
            Block::Air.id(),
            "water crossed a seam into an in-flight section"
        );

        w.remove_pending_section(in_flight);
        run_ticks(&mut w, 60);
        assert_eq!(
            w.data.chunk_block(16, 65, 8),
            Block::Water.id(),
            "flow never resumed after the in-flight section resolved"
        );
    }

    #[test]
    fn writes_into_an_in_flight_section_are_refused() {
        let mut w = flat_server_world();

        let loaded = SectionPos::new(1, 4, 0);
        w.insert_pending_section(loaded);
        assert!(!w.set_block_world(20, 70, 8, Block::Stone));
        assert!(!w.set_fluid_world(IVec3::new(20, 70, 8), Block::Water, 0));
        assert_eq!(w.data.chunk_block(20, 70, 8), Block::Air.id());

        let absent = SectionPos::new(1, 6, 0);
        w.insert_pending_section(absent);
        assert!(!w.set_block_world(20, 100, 8, Block::Stone));
        assert!(
            !w.data.sections.contains_key(&absent),
            "write materialized an in-flight section"
        );

        let awaited = SectionPos::new(0, 4, 0);
        w.side.gen.awaited_overlays.insert(awaited);
        w.note_stream_nonfinal(awaited);
        assert!(!w.set_block_world(8, 70, 8, Block::Stone));
        w.side.gen.awaited_overlays.remove(&awaited);
        w.settle_stream_nonfinal(awaited);
        assert!(w.set_block_world(8, 70, 8, Block::Stone));
    }

    #[test]
    fn checked_mob_spawn_requires_loaded_stream_final_body_cells() {
        let mut w = ServerWorld::new(0, 0);
        let column = ChunkPos::new(0, 0);
        w.data.ensure_column(column);
        let pos = WorldPos::new(8.5, 64.0, 8.5);

        assert!(w.physics_cell_final_at(8, 64, 8));
        assert!(!w.section_stream_final_at(8, 64, 8));
        assert!(
            w.spawn_mob_checked(Mob::Owl, pos, 0.0).is_none(),
            "a truthful absent-air summary is not a loaded entity destination"
        );

        w.insert_empty_column_for_test(column);
        let section = SectionPos::new(0, 4, 0);
        w.side.gen.awaited_overlays.insert(section);
        w.note_stream_nonfinal(section);
        assert!(
            w.spawn_mob_checked(Mob::Owl, pos, 0.0).is_none(),
            "a loaded base with an in-flight save overlay is not final"
        );
        w.side.gen.awaited_overlays.remove(&section);
        w.settle_stream_nonfinal(section);
        assert!(w.spawn_mob_checked(Mob::Owl, pos, 0.0).is_some());
    }

    #[test]
    fn harvest_skips_a_section_whose_overlay_is_in_flight() {
        let mut w = flat_server_world();
        let sp = SectionPos::new(0, 4, 0);
        assert!(w.set_block_world(1, 70, 1, Block::Stone));

        w.side.gen.awaited_overlays.insert(sp);
        w.note_stream_nonfinal(sp);
        assert!(
            w.harvest_section_snapshot(sp).is_none(),
            "persisting a base whose overlay is in flight would shadow the on-disk record"
        );
        w.side.gen.awaited_overlays.remove(&sp);
        w.settle_stream_nonfinal(sp);
        assert!(w.harvest_section_snapshot(sp).is_some());
    }

    #[test]
    fn mod_reads_and_cell_kv_writes_treat_an_in_flight_overlay_as_unloaded() {
        let mut w = flat_server_world();
        let sp = SectionPos::new(0, 4, 0);

        assert_eq!(w.block_if_stream_final(8, 64, 8), Some(Block::Stone));
        assert!(w.section_stream_final_at(8, 64, 8));

        w.side.gen.awaited_overlays.insert(sp);
        w.note_stream_nonfinal(sp);
        assert_eq!(
            w.block_if_stream_final(8, 64, 8),
            None,
            "a half-streamed section leaked its generated base to a mod read"
        );
        assert!(!w.section_stream_final_at(8, 64, 8));
        assert!(
            !w.cell_kv_set(8, 64, 8, "kitchen:state".into(), vec![1]),
            "a cell-KV write raced the in-flight overlay"
        );

        w.side.gen.awaited_overlays.remove(&sp);
        w.settle_stream_nonfinal(sp);
        assert_eq!(w.block_if_stream_final(8, 64, 8), Some(Block::Stone));
        assert!(w.cell_kv_set(8, 64, 8, "kitchen:state".into(), vec![1]));
    }

    #[test]
    fn kick_floods_across_a_seam_between_all_water_and_air_sections() {
        // Chunk (0,0): stone floor, water y65..=79, section (0,4,0) has water+stone but no air.
        // Chunk (1,0): floor only, section (1,4,0) has air, no water.
        // The only water-air contact sits right on the section seam - the per-section interior scan
        // can't see it from either side.
        let build = || {
            let mut w = ServerWorld::new(0, 1);
            let mut a = Chunk::new(0, 0);
            let mut b = Chunk::new(1, 0);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    a.set_block(x, 64, z, Block::Stone);
                    b.set_block(x, 64, z, Block::Stone);
                    for y in 65..=79 {
                        a.set_block(x, y, z, Block::Water);
                    }
                }
            }
            let w2 = {
                w.insert_chunk_for_test(ChunkPos::new(0, 0), a);
                w.insert_chunk_for_test(ChunkPos::new(1, 0), b);
                w
            };
            let s = w2
                .section_at_world_for_test(8, 70, 8)
                .expect("water section loaded");
            assert!(
                s.has_fluid() && !s.has_air(),
                "fixture: airless water section"
            );
            w2
        };

        let mut w = build();
        w.queue_loaded_section_fluid_updates(&[SectionPos::new(1, 4, 0)]);
        run_ticks(&mut w, 30);
        assert_eq!(
            w.data.chunk_block(16, 65, 8),
            Block::Water.id(),
            "air-side kick missed cross-seam water"
        );

        let mut w = build();
        w.queue_loaded_section_fluid_updates(&[SectionPos::new(0, 4, 0)]);
        run_ticks(&mut w, 30);
        assert_eq!(
            w.data.chunk_block(16, 65, 8),
            Block::Water.id(),
            "water-side kick missed cross-seam air"
        );
    }

    #[test]
    fn reload_kick_rearms_enclosed_mid_drain_water() {
        // Walled basin mid-drain: sourceless flowing water, only open face is up.
        // Old air-adjacency kick couldn't see this (air above never starts flow), so the sheet
        // froze at flowing levels forever after unload.
        // Kick has to re-arm from flow metadata to get draining going again.
        let mut w = ServerWorld::new(0, 1);
        let mut c = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                c.set_block(x, 64, z, Block::Stone);
            }
        }
        for z in 5..=10 {
            for x in 5..=10 {
                if x == 5 || x == 10 || z == 5 || z == 10 {
                    c.set_block(x, 65, z, Block::Stone);
                } else {
                    c.set_fluid(x, 65, z, Block::Water, 3);
                }
            }
        }
        w.insert_chunk_for_test(ChunkPos::new(0, 0), c);

        w.queue_loaded_section_fluid_updates(&[SectionPos::new(0, 4, 0)]);
        run_ticks(&mut w, 200);
        assert_eq!(
            w.data.chunk_block(7, 65, 7),
            Block::Air.id(),
            "reloaded sourceless flow in a walled basin must drain, not freeze"
        );
    }

    #[test]
    fn ingest_kick_rearms_dropped_checks_deep_in_a_kept_neighbour() {
        // The guard drops fired checks whose read box (±SIM_READ_REACH) touched
        // an absent section — including checks 2..=5 cells inside a section
        // that never unloaded. When the absent section lands, the kick must
        // re-arm the kept side that deep; the 1-cell inflow plane cannot.
        let mut w = ServerWorld::new(0, 1);
        let mut kept = petramond_world::section::Section::new(0, 4, 0);
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                kept.set_block(x, 0, z, Block::Stone);
            }
        }
        kept.set_fluid(12, 1, 8, Block::Water, 4);
        w.insert_section_for_test(SectionPos::new(0, 4, 0), kept);
        w.insert_section_for_test(
            SectionPos::new(1, 4, 0),
            petramond_world::section::Section::new(1, 4, 0),
        );

        w.queue_loaded_section_fluid_updates(&[SectionPos::new(1, 4, 0)]);
        assert!(
            !w.queue_block_update(IVec3::new(12, 65, 8)),
            "kick must reach the kept neighbour's mid-flow cell, not just the seam plane"
        );
        run_ticks(&mut w, 30);
        assert_eq!(
            w.data.chunk_block(12, 65, 8),
            Block::Air.id(),
            "re-armed sourceless flow dries"
        );
    }
}
