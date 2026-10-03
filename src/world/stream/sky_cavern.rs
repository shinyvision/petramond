//! Sky caverns: below-shell sections the open sky can see.
//!
//! Streaming wants a column's surface shell plus a vertical window around the player, so a
//! sinkhole or open cavern deeper than both shows sky through its floor. Sections a
//! sightline from a vertical sky opening below the shell can reach count as surface
//! terrain: wanted, kept and shipped wherever the player is.
//!
//! Same walk as the replica's deep visibility: any open cell on an entered section's face
//! makes the neighbour SEEN (its blocks may be what the sightline hits), and the walk ENTERS
//! the neighbour only if its facing plane is open too. Grows one unloaded layer at a time
//! (a face is only known once its section lands) and never enters the shell itself, which
//! streams anyway.

use crate::world::store::{column_cy_bit, LoadTarget};
use crate::world::{ServerWorld, World, WorldSide};
use petramond_math::math::FACE_NEIGHBORS;
use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_MIN_CY, SECTION_SIZE};
use rustc_hash::FxHashSet;
use std::collections::VecDeque;
use std::sync::Arc;

impl<S: WorldSide> World<S> {
    #[inline]
    pub(in crate::world) fn sky_cavern_bits(&self, cp: ChunkPos) -> u32 {
        self.side
            .server()
            .and_then(|s| s.gen.sky_caverns.get(&cp))
            .map_or(0, |c| c.seen)
    }

    #[inline]
    pub(in crate::world) fn sky_cavern_contains(&self, sp: SectionPos) -> bool {
        self.sky_cavern_bits(sp.chunk_pos()) & column_cy_bit(sp.cy) != 0
    }
}

impl ServerWorld {
    fn shell_lo(&self, cp: ChunkPos) -> Option<i32> {
        let col = self.side.gen.column_gen.get(&cp)?;
        Some(*Self::surface_window_for_column(col, 0).start())
    }

    fn section_loaded(&self, sp: SectionPos) -> bool {
        self.data
            .section_column_cys
            .get(&sp.chunk_pos())
            .is_some_and(|bits| bits & column_cy_bit(sp.cy) != 0)
    }

    fn sky_cavern_entered(&self, sp: SectionPos) -> bool {
        self.side
            .gen
            .sky_caverns
            .get(&sp.chunk_pos())
            .is_some_and(|c| c.entered & column_cy_bit(sp.cy) != 0)
    }

    /// The section under the shell when the column's sky cover drops through the shell
    /// floor. Waits for the whole shell: absent sections read as open sky in the cover.
    fn sky_opening_below_shell(&self, cp: ChunkPos) -> Option<SectionPos> {
        let col = self.side.gen.column_gen.get(&cp)?;
        let shell = Self::surface_window_for_column(col, 0);
        let lo = *shell.start();
        if lo <= SECTION_MIN_CY {
            return None;
        }
        let content_top_cy = col.content_top().div_euclid(SECTION_SIZE as i32);
        if !shell
            .filter(|&cy| cy <= content_top_cy)
            .all(|cy| self.section_loaded(SectionPos::new(cp.cx, cy, cp.cz)))
        {
            return None;
        }
        let (sky_lo, _) = self.data.columns.get(&cp)?.sky_cover_range();
        (sky_lo < lo * SECTION_SIZE as i32).then(|| SectionPos::new(cp.cx, lo - 1, cp.cz))
    }

    /// Marks `sp` seen; a loaded section's sendability changed, an unloaded one is frontier.
    fn see_sky_cavern(&mut self, sp: SectionPos, frontier: &mut Vec<SectionPos>) {
        let bits = self.side.gen.sky_caverns.entry(sp.chunk_pos()).or_default();
        if bits.seen & column_cy_bit(sp.cy) != 0 {
            return;
        }
        bits.seen |= column_cy_bit(sp.cy);
        if self.section_loaded(sp) {
            self.side.replication.send_events.push(sp);
        } else {
            frontier.push(sp);
        }
    }

    fn enter_sky_cavern(&mut self, sp: SectionPos, queue: &mut VecDeque<SectionPos>) {
        let bits = self.side.gen.sky_caverns.entry(sp.chunk_pos()).or_default();
        if bits.entered & column_cy_bit(sp.cy) == 0 {
            bits.entered |= column_cy_bit(sp.cy);
            queue.push_back(sp);
        }
    }

    /// Whether a seen section that just landed is entered: it is a sky opening, or an
    /// entered neighbour looks into it through planes open on both sides.
    fn landed_sky_cavern_entered(&self, sp: SectionPos) -> bool {
        if self.sky_opening_below_shell(sp.chunk_pos()) == Some(sp) {
            return true;
        }
        let Some(s) = self.data.sections.get(&sp) else {
            return false;
        };
        FACE_NEIGHBORS.iter().any(|d| {
            let m = SectionPos::new(sp.cx + d.x, sp.cy + d.y, sp.cz + d.z);
            s.face_plane_open(d.x, d.y, d.z)
                && self.sky_cavern_entered(m)
                && self
                    .data
                    .sections
                    .get(&m)
                    .is_some_and(|ms| ms.face_plane_open(-d.x, -d.y, -d.z))
        })
    }

    /// Seeds from columns whose sky opening changed, walks on from every landed cavern
    /// section, and requests the unloaded frontier.
    pub(super) fn grow_sky_caverns(
        &mut self,
        ingested: &[SectionPos],
        columns: &FxHashSet<ChunkPos>,
        target: LoadTarget,
    ) {
        let mut queue: VecDeque<SectionPos> = VecDeque::new();
        let mut frontier: Vec<SectionPos> = Vec::new();
        for &cp in columns {
            if let Some(sp) = self.sky_opening_below_shell(cp) {
                self.see_sky_cavern(sp, &mut frontier);
                if self.section_loaded(sp) {
                    self.enter_sky_cavern(sp, &mut queue);
                }
            }
        }
        for &sp in ingested {
            if self.sky_cavern_contains(sp)
                && !self.sky_cavern_entered(sp)
                && self.landed_sky_cavern_entered(sp)
            {
                self.enter_sky_cavern(sp, &mut queue);
            }
        }

        while let Some(pos) = queue.pop_front() {
            let Some(s) = self.data.sections.get(&pos).cloned() else {
                continue;
            };
            for d in FACE_NEIGHBORS {
                let (dx, dy, dz) = (d.x, d.y, d.z);
                if !s.face_plane_open(dx, dy, dz) {
                    continue;
                }
                let n = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                if !SectionPos::cy_in_range(n.cy)
                    || self.shell_lo(n.chunk_pos()).is_none_or(|lo| n.cy >= lo)
                {
                    continue;
                }
                self.see_sky_cavern(n, &mut frontier);
                if self
                    .data
                    .sections
                    .get(&n)
                    .is_some_and(|ns| ns.face_plane_open(-dx, -dy, -dz))
                {
                    self.enter_sky_cavern(n, &mut queue);
                }
            }
        }
        self.request_sky_cavern_frontier(frontier, target);
    }

    fn request_sky_cavern_frontier(&mut self, frontier: Vec<SectionPos>, target: LoadTarget) {
        let underground = self.anchor_underground(target);
        let mut wanted = Vec::new();
        for sp in frontier {
            if self.side.gen.pending_sections.contains(&sp) {
                continue;
            }
            let Some(col) = self
                .side
                .gen
                .column_gen
                .get(&sp.chunk_pos())
                .map(Arc::clone)
            else {
                continue;
            };
            let band_lo = *Self::surface_window_for_column(&col, 0).start();
            wanted.push((
                target.surface_biased_section_key(sp, band_lo, underground),
                sp,
                col,
            ));
        }
        self.admit_section_candidates(wanted);
    }
}
