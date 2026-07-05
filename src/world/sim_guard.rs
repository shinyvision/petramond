//! Streaming-finality guard: the simulation must never mutate — or decide
//! anything from reads of — a section whose streamed content is not FINAL.
//!
//! Sections stream in asynchronously (gen job → install → saved-overlay
//! apply). Until that finishes, world reads there LIE (an absent cave-band
//! section reads as air) and writes RACE (a synchronously materialized base is
//! clobbered by the late gen result; a mutated base can be persisted and then
//! shadow the player's on-disk record forever). One water flow against a
//! half-streamed neighbourhood is enough to mark sections `modified` and
//! freeze the accident into the save.
//!
//! Enforced at the tick dispatch points (scheduled ticks, block updates,
//! random ticks — see `world::tick`) and at the write choke points
//! (`set_block_world`, `set_water_world`, `materialize_section`,
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
//! on-load water kick when that terrain streams in
//! (`world::stream::queue_loaded_section_water_updates`).

use crate::chunk::{SectionPos, SECTION_SIZE};
use crate::mathh::IVec3;
use crate::section::SectionSummary;

use super::store::World;

/// Widest read reach of any gated behaviour, in cells: water's sideways slope
/// search walks up to `1 + SLOPE_FIND_DIST` = 5 cells from the flowing cell;
/// every other reaction reads closer. ±5 cells stays within the adjacent
/// section on each axis, so the reach box spans at most 2 sections per axis.
pub(super) const SIM_READ_REACH: i32 = 5;

/// Ticks before retrying simulation work that was blocked on an in-flight
/// section (a gen job or saved overlay still landing).
pub(super) const SIM_RETRY_DELAY: u64 = 5;

/// Whether a cell's simulation may run now, must retry, or should be dropped.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub(super) enum SimReadiness {
    Ready,
    /// Blocked on an in-flight section (gen/overlay); resolves within ticks.
    Wait,
    /// Blocked on terrain that is not coming under the current load target.
    /// The on-load water kick re-arms the flow if it ever streams in.
    Drop,
}

/// One section's streamed-content finality.
#[derive(PartialEq, Eq, Clone, Copy)]
enum StreamState {
    Final,
    InFlight,
    Unresolved,
}

impl World {
    /// Whether `sp` may be written (or materialized) right now: no in-flight
    /// gen job and no in-flight saved overlay. A write into a pending-gen
    /// section would be clobbered by the landing result; a write into a
    /// section whose overlay is still in flight mutates content about to be
    /// replaced by the player's saved record.
    #[inline]
    pub(super) fn stream_writable(&self, sp: SectionPos) -> bool {
        !self.pending_sections.contains(&sp)
            && !self.awaited_overlays.contains(&sp)
            && !self.pending_overlays.contains_key(&sp)
    }

    /// `quiet` short-circuits the three in-flight probes when the caller
    /// already knows all in-flight sets are empty (the steady-state fast path).
    fn section_stream_state(&self, sp: SectionPos, quiet: bool) -> StreamState {
        if !SectionPos::cy_in_range(sp.cy) {
            return StreamState::Final; // outside the world: reads air forever
        }
        if !quiet && !self.stream_writable(sp) {
            return StreamState::InFlight;
        }
        if self.sections.contains_key(&sp) {
            return StreamState::Final;
        }
        let cp = sp.chunk_pos();
        if let Some(col) = self.column_gen.get(&cp) {
            if self.save.as_ref().is_some_and(|s| s.manifest_contains(sp)) {
                // A saved record will overlay this section when it streams in.
                return StreamState::InFlight;
            }
            return match col.section_summary(sp.cy) {
                SectionSummary::Empty | SectionSummary::FullOpaque | SectionSummary::FullWater => {
                    StreamState::Final
                }
                _ => StreamState::Unresolved,
            };
        }
        if self.columns.contains_key(&cp) {
            // A column without gen data (a test fixture, or one ensured by a
            // materialize-on-write): its absent sections are genuinely all-air.
            return StreamState::Final;
        }
        if self.pending.contains_key(&cp) {
            return StreamState::InFlight;
        }
        if self
            .last_load_target
            .is_some_and(|t| Self::column_wanted(t, cp))
        {
            return StreamState::InFlight; // column gen will be submitted shortly
        }
        StreamState::Unresolved
    }

    /// Classify whether simulation work at `pos` may run: every section within
    /// the behaviour read reach (±[`SIM_READ_REACH`] cells) must be
    /// stream-final. `Drop` outranks `Wait`: terrain that is not coming can
    /// only be resolved by a future load event, never by waiting.
    pub(super) fn sim_readiness_at(&self, pos: IVec3) -> SimReadiness {
        let quiet = self.pending_sections.is_empty()
            && self.awaited_overlays.is_empty()
            && self.pending_overlays.is_empty();
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
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ};
    use crate::mathh::IVec3;
    use crate::world::testutil::flat_world;

    use super::super::store::World;

    fn run_ticks(w: &mut World, n: u32) {
        let recipes = crate::crafting::Recipes::default();
        for _ in 0..n {
            w.game_tick(&recipes);
        }
    }

    #[test]
    fn water_waits_for_an_in_flight_neighbor_section_then_flows() {
        let mut w = flat_world();
        // The receiving section (chunk (1,0), y 64..79) has an in-flight gen
        // job: flow at the seam must hold.
        let in_flight = SectionPos::new(1, 4, 0);
        w.pending_sections.insert(in_flight);

        w.set_block_world(15, 65, 8, Block::Water);
        run_ticks(&mut w, 60);
        assert_eq!(
            w.chunk_block(16, 65, 8),
            Block::Air.id(),
            "water crossed a seam into an in-flight section"
        );

        // The job resolves: the parked (retrying) flow must complete on its own.
        w.pending_sections.remove(&in_flight);
        run_ticks(&mut w, 60);
        assert_eq!(
            w.chunk_block(16, 65, 8),
            Block::Water.id(),
            "flow never resumed after the in-flight section resolved"
        );
    }

    #[test]
    fn writes_into_an_in_flight_section_are_refused() {
        let mut w = flat_world();

        // Loaded section with an in-flight gen result: both write paths refuse.
        let loaded = SectionPos::new(1, 4, 0);
        w.pending_sections.insert(loaded);
        assert!(!w.set_block_world(20, 70, 8, Block::Stone));
        assert!(!w.set_water_world(IVec3::new(20, 70, 8), Block::Water, 0));
        assert_eq!(w.chunk_block(20, 70, 8), Block::Air.id());

        // Absent section with an in-flight job: the write must not materialize it.
        let absent = SectionPos::new(1, 6, 0);
        w.pending_sections.insert(absent);
        assert!(!w.set_block_world(20, 100, 8, Block::Stone));
        assert!(
            !w.sections.contains_key(&absent),
            "write materialized an in-flight section"
        );

        // An awaited saved overlay blocks writes the same way, and unblocks.
        let awaited = SectionPos::new(0, 4, 0);
        w.awaited_overlays.insert(awaited);
        assert!(!w.set_block_world(8, 70, 8, Block::Stone));
        w.awaited_overlays.remove(&awaited);
        assert!(w.set_block_world(8, 70, 8, Block::Stone));
    }

    #[test]
    fn harvest_skips_a_section_whose_overlay_is_in_flight() {
        let mut w = flat_world();
        let sp = SectionPos::new(0, 4, 0);
        assert!(w.set_block_world(1, 70, 1, Block::Stone)); // marks it modified

        w.awaited_overlays.insert(sp);
        assert!(
            w.harvest_section_snapshot(sp).is_none(),
            "persisting a base whose overlay is in flight would shadow the on-disk record"
        );
        w.awaited_overlays.remove(&sp);
        assert!(w.harvest_section_snapshot(sp).is_some());
    }

    #[test]
    fn kick_floods_across_a_seam_between_all_water_and_air_sections() {
        // Chunk (0,0): stone floor y=64, WATER filling y 65..=79 — section
        // (0,4,0) holds water and stone but ZERO air. Chunk (1,0): floor only —
        // section (1,4,0) holds air and no water. The only water-air contact is
        // exactly on the section seam, which the per-section interior scan can
        // never see from either side.
        let build = || {
            let mut w = World::new(0, 1);
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
                s.has_water() && !s.has_air(),
                "fixture: airless water section"
            );
            w2
        };

        // Air side lands second: its ingest kick must find the neighbour's water.
        let mut w = build();
        w.queue_loaded_section_water_updates(&[SectionPos::new(1, 4, 0)]);
        run_ticks(&mut w, 30);
        assert_eq!(
            w.chunk_block(16, 65, 8),
            Block::Water.id(),
            "air-side kick missed cross-seam water"
        );

        // Water side lands second: its boundary-plane kick must fire too.
        let mut w = build();
        w.queue_loaded_section_water_updates(&[SectionPos::new(0, 4, 0)]);
        run_ticks(&mut w, 30);
        assert_eq!(
            w.chunk_block(16, 65, 8),
            Block::Water.id(),
            "water-side kick missed cross-seam air"
        );
    }
}
