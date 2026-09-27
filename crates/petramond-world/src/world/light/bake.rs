//! The per-section full light bake: one 48³ flood over a section's 3×3×3
//! neighbourhood, clipped to its 16³. Streaming first-bakes, persisted-light
//! repair and every relight the incremental path declines run through here or
//! through its batched twin ([`super::batch`]); both gather their inputs with
//! the same [`neighborhood::Snapshot`], so they cannot disagree on what a cell
//! is.

use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::chunk::{ChunkPos, SectionPos, SKY_FULL};
use crate::column::Column;
use crate::light::LightRgb;
use crate::mathh::IVec3;
use crate::section::Section;

use super::shape::LightCells;
use super::skylight::{self, SkyPlan};
use super::{flood, neighborhood, NBHD, NBHD_VOLUME};

/// One section's freshly baked light cubes, tagged with the section revision
/// the bake read (the installer drops a result whose revision moved on).
pub struct LightBakeOutput {
    pub pos: SectionPos,
    pub revision: u64,
    pub skylight: Arc<[u8]>,
    pub blocklight: Arc<[LightRgb]>,
}

/// A self-contained per-section bake job: the sky plan plus cheap shared
/// handles to the neighbourhood, gathered on the main thread.
pub struct SectionBakeJob {
    pos: SectionPos,
    revision: u64,
    sky: SkyPlan,
    /// Present only when a flood will actually run (a `Flood` sky plan or an
    /// emitter in range).
    nbhd: Option<neighborhood::Snapshot>,
    emitters: Vec<(IVec3, LightRgb)>,
}

impl SectionBakeJob {
    /// Snapshot `pos` for a bake when its light is dirty (`None` when it is
    /// clean or absent).
    pub fn snapshot(
        pos: SectionPos,
        sections: &FxHashMap<SectionPos, Arc<Section>>,
        columns: &FxHashMap<ChunkPos, Arc<Column>>,
    ) -> Option<Self> {
        if !sections.get(&pos)?.light_dirty {
            return None;
        }
        Self::snapshot_unchecked(pos, sections, columns)
    }

    /// [`Self::snapshot`] without the dirty gate — the parity and equivalence
    /// tests rebake settled sections to compare other paths against.
    pub fn snapshot_unchecked(
        pos: SectionPos,
        sections: &FxHashMap<SectionPos, Arc<Section>>,
        columns: &FxHashMap<ChunkPos, Arc<Column>>,
    ) -> Option<Self> {
        let section = sections.get(&pos)?;
        let revision = section.light_revision;
        let sky = skylight::plan(pos, columns);
        let low = SectionPos::new(pos.cx - 1, pos.cy - 1, pos.cz - 1);
        let emitters = neighborhood::collect_emitters(low, 3, sections);
        let nbhd = (matches!(sky, SkyPlan::Flood { .. }) || !emitters.is_empty())
            .then(|| neighborhood::Snapshot::gather(low, 3, sections));
        Some(Self {
            pos,
            revision,
            sky,
            nbhd,
            emitters,
        })
    }

    pub fn pos(&self) -> SectionPos {
        self.pos
    }
}

/// Per-light-thread reusable bake scratch: the assembled 48³ neighbourhood block
/// cube plus the flood working set. Streaming bakes run thousands of times across
/// several threads; reusing these keeps ~220 KB of per-bake churn off the allocator
/// (the returned per-section light cubes are still allocated fresh — they outlive
/// the bake).
struct BakeScratch {
    blocks: Box<[u16]>,
    flood: flood::FloodScratch,
}

thread_local! {
    static BAKE_SCRATCH: std::cell::RefCell<BakeScratch> = std::cell::RefCell::new(BakeScratch {
        blocks: vec![0u16; NBHD_VOLUME].into_boxed_slice(),
        flood: flood::FloodScratch::new(),
    });
}

/// Run one per-section bake to completion (on a light worker, or inline).
pub fn bake_section(job: SectionBakeJob) -> LightBakeOutput {
    let SectionBakeJob {
        pos,
        revision,
        sky,
        nbhd,
        emitters,
    } = job;

    BAKE_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let BakeScratch { blocks, flood } = &mut *scratch;

        let blocks: Option<&[u16]> = nbhd.as_ref().map(|n| {
            n.assemble_blocks(blocks);
            &blocks[..]
        });
        let states = nbhd
            .as_ref()
            .map(neighborhood::Snapshot::shape_states)
            .unwrap_or_default();

        let skylight = match sky {
            SkyPlan::Full => crate::section::uniform_cube(SKY_FULL),
            SkyPlan::Dark => crate::section::uniform_cube(0),
            SkyPlan::Flood { surface } => {
                let blocks =
                    blocks.expect("a flooding skylight bake carries its neighbourhood blocks");
                flood::skylight(pos, LightCells::new(blocks, &states, NBHD), &surface, flood)
            }
        };

        let blocklight = if emitters.is_empty() {
            crate::light::dark_cube()
        } else {
            let blocks = blocks.expect("a block-light bake carries its neighbourhood blocks");
            flood::block_light(
                pos,
                LightCells::new(blocks, &states, NBHD),
                &emitters,
                flood,
            )
        };

        LightBakeOutput {
            pos,
            revision,
            skylight,
            blocklight,
        }
    })
}
