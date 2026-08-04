//! Batched 2×2×2 light bake: one 64³ flood shared by up to eight sections instead
//! of eight overlapping 48³ floods.
//!
//! Byte-parity with the per-section bake holds because light influence is bounded:
//! full-strength `SKY_FULL` cells are exactly the above-cover cells the pre-fill
//! paints (identical in both cube sizes), and every other value decays 2 per step,
//! so nothing more than 15 cells away can touch a section's 16³ result — and every
//! cell within that reach of a member lies inside both its own 48³ cube and the
//! batch's 64³ cube. This relies on the engine invariant that the sky-cover map is
//! consistent with the blocks (a cover cell never transmits direct skylight);
//! otherwise the undecayed straight-down rule could tunnel full skylight through a
//! phantom shaft at depths where the two cube sizes disagree. Pinned by
//! `batched_bake_matches_per_section_bakes`.
//!
//! Sky shortcuts are preserved per member: a `Full`/`Dark` classified member never
//! pays for the flood, and a group with no flooding member floods nothing.
//!
//! Colour does not weaken the reach argument: every CHANNEL decays 2 per step
//! independently and no channel exceeds the row's `emission`, so each channel's
//! influence is bounded by the same 15 cells the scalar cell was. The bound
//! that matters is per channel, and it holds per channel.
//!
//! The ≥3-member grouping threshold (`stream::settle`) is unchanged by the
//! wider cell. It comes from 64³ / 48³ = 2.37: below three members the shared
//! cube touches more cells than separate floods would. Widening the block cell
//! scales the batch cube and the per-section cubes by the SAME factor, so the
//! ratio — and therefore the break-even count — is invariant; and the BFS
//! itself visits the same cells in both, which is where the measured 2× came
//! from in the first place.

use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::chunk::{section_idx, ChunkPos, SectionPos, SECTION_SIZE, SKY_FULL};
use crate::column::Column;
use crate::light::LightRgb;
use crate::mathh::IVec3;
use crate::section::Section;

use super::shape::{LightCells, ShapeStateSnapshot, SparseCellState};
use super::skylight::SkyClass;
use super::{flood, neighborhood, skylight};

/// Sections per axis in one batch group.
pub const GROUP: i32 = 2;
/// Sections per axis of the gathered neighbourhood (the group plus a one-section halo).
pub const SPAN: usize = GROUP as usize + 2;
/// Cells per axis / total cells of the batch flood cube.
const BDIM: usize = SPAN * SECTION_SIZE;
const BVOL: usize = BDIM * BDIM * BDIM;

#[inline]
fn bidx(x: usize, y: usize, z: usize) -> usize {
    (y * BDIM + z) * BDIM + x
}

#[inline]
fn span_idx(dx: usize, dy: usize, dz: usize) -> usize {
    (dy * SPAN + dz) * SPAN + dx
}

struct BatchMember {
    pos: SectionPos,
    revision: u64,
    sky: SkyClass,
}

/// A self-contained batch bake job: per-member classifications plus ONE shared
/// snapshot of the group's 4×4×4 section neighbourhood.
pub struct LightBatchJob {
    base: SectionPos,
    members: Vec<BatchMember>,
    /// `SPAN`³ field-`Arc` block buffers (`None` = absent, reads as air).
    blocks: Vec<Option<crate::section::BlockCube>>,
    states: Vec<SparseCellState>,
    /// `BDIM`² sky-cover map, gathered only when a member needs the sky flood.
    surface: Option<Box<[i32]>>,
    emitters: Vec<(IVec3, LightRgb)>,
}

pub struct LightBatchOutput {
    pub pos: SectionPos,
    pub revision: u64,
    pub skylight: Arc<[u8]>,
    pub blocklight: Arc<[LightRgb]>,
}

impl LightBatchJob {
    /// The members this job will actually bake (snapshot may have skipped
    /// requested positions whose section was absent).
    pub fn member_positions(&self) -> impl Iterator<Item = SectionPos> + '_ {
        self.members.iter().map(|m| m.pos)
    }

    /// Drop members whose per-member cancellation fired while the job was
    /// queued; the shared snapshot stays valid for the rest.
    pub fn retain_members(&mut self, keep: impl Fn(SectionPos) -> bool) {
        self.members.retain(|m| keep(m.pos));
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// Group section positions into their 2×2×2-aligned batches: `(group base, members)`.
pub fn group_positions(
    positions: &[SectionPos],
) -> Vec<(SectionPos, Vec<SectionPos>)> {
    let mut groups: std::collections::BTreeMap<(i32, i32, i32), Vec<SectionPos>> =
        std::collections::BTreeMap::new();
    for &p in positions {
        let key = (
            p.cx.div_euclid(GROUP),
            p.cy.div_euclid(GROUP),
            p.cz.div_euclid(GROUP),
        );
        groups.entry(key).or_default().push(p);
    }
    groups
        .into_iter()
        .map(|(k, v)| (SectionPos::new(k.0 * GROUP, k.1 * GROUP, k.2 * GROUP), v))
        .collect()
}

/// Snapshot one batch: the same cheap per-section handles `super::queue::LightBakeJob`
/// takes, gathered once for the whole group.
pub fn snapshot_batch(
    base: SectionPos,
    member_positions: &[SectionPos],
    sections: &FxHashMap<SectionPos, Arc<Section>>,
    columns: &FxHashMap<ChunkPos, Column>,
) -> Option<LightBatchJob> {
    let mut members = Vec::with_capacity(member_positions.len());
    for &pos in member_positions {
        debug_assert!(
            (0..GROUP).contains(&(pos.cx - base.cx))
                && (0..GROUP).contains(&(pos.cy - base.cy))
                && (0..GROUP).contains(&(pos.cz - base.cz)),
            "member outside its batch group"
        );
        let Some(section) = sections.get(&pos) else {
            continue;
        };
        members.push(BatchMember {
            pos,
            revision: section.light_revision,
            sky: skylight::classify(pos, columns),
        });
    }
    if members.is_empty() {
        return None;
    }
    let any_flood = members.iter().any(|m| m.sky == SkyClass::Flood);

    let mut emitters = Vec::new();
    let mut blocks: Vec<Option<crate::section::BlockCube>> = vec![None; SPAN * SPAN * SPAN];
    let mut states = Vec::new();
    for dy in 0..SPAN {
        for dz in 0..SPAN {
            for dx in 0..SPAN {
                let npos = SectionPos::new(
                    base.cx + dx as i32 - 1,
                    base.cy + dy as i32 - 1,
                    base.cz + dz as i32 - 1,
                );
                let Some(section) = sections.get(&npos) else {
                    continue;
                };
                neighborhood::collect_section_emitters(npos, section, &mut emitters);
                blocks[span_idx(dx, dy, dz)] = Some(section.block_cube());
                let (bx, by, bz) = (dx * SECTION_SIZE, dy * SECTION_SIZE, dz * SECTION_SIZE);
                super::shape::collect_shape_states(
                    section,
                    |lx, ly, lz| bidx(bx + lx, by + ly, bz + lz),
                    &mut states,
                );
            }
        }
    }
    if !any_flood && emitters.is_empty() {
        // No flood will run: match the per-section jobs, which skip the gather.
        blocks.iter_mut().for_each(|b| *b = None);
        states.clear();
    }

    let surface = any_flood.then(|| {
        skylight::gather_surface_span(ChunkPos::new(base.cx - 1, base.cz - 1), SPAN, columns)
    });

    Some(LightBatchJob {
        base,
        members,
        blocks,
        states,
        surface,
        emitters,
    })
}

/// Assemble the batch block cube from the gathered `Arc`s, one 16-wide row copy at
/// a time (absent sections stay air).
fn assemble_blocks(arcs: &[Option<crate::section::BlockCube>], out: &mut [u16]) {
    debug_assert_eq!(out.len(), BVOL);
    out.fill(0);
    for dy in 0..SPAN {
        for dz in 0..SPAN {
            for dx in 0..SPAN {
                let Some(src) = &arcs[span_idx(dx, dy, dz)] else {
                    continue;
                };
                let (bx, by, bz) = (dx * SECTION_SIZE, dy * SECTION_SIZE, dz * SECTION_SIZE);
                for ly in 0..SECTION_SIZE {
                    for lz in 0..SECTION_SIZE {
                        let d = bidx(bx, by + ly, bz + lz);
                        let s = section_idx(0, ly, lz);
                        src.expand_row_into(s, &mut out[d..d + SECTION_SIZE]);
                    }
                }
            }
        }
    }
}

struct BatchScratch {
    blocks: Vec<u16>,
    flood: flood::FloodScratch,
}

thread_local! {
    static BATCH_SCRATCH: std::cell::RefCell<BatchScratch> =
        std::cell::RefCell::new(BatchScratch {
            blocks: vec![0u16; BVOL],
            flood: flood::FloodScratch::new(),
        });
}

pub fn run_light_bake_batch(job: LightBatchJob) -> Vec<LightBatchOutput> {
    let LightBatchJob {
        base,
        members,
        blocks,
        states,
        surface,
        emitters,
    } = job;

    BATCH_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let BatchScratch {
            blocks: block_buf,
            flood: flood_scratch,
        } = &mut *scratch;

        if surface.is_some() || !emitters.is_empty() {
            assemble_blocks(&blocks, block_buf);
        }
        let states = ShapeStateSnapshot::from_sparse(&states, BVOL);
        // Every member sits in the group box; light that cannot reach it is
        // work the flood need not do.
        let keep = flood::Keep::new(SECTION_SIZE, SECTION_SIZE * (1 + GROUP as usize));
        let (box_, boy, boz) = base.origin_world();
        let member_off = |m: &BatchMember| {
            (
                ((m.pos.cx - base.cx + 1) as usize) * SECTION_SIZE,
                ((m.pos.cy - base.cy + 1) as usize) * SECTION_SIZE,
                ((m.pos.cz - base.cz + 1) as usize) * SECTION_SIZE,
            )
        };

        // Skylight: one joint flood when any member straddles the surface band.
        // Full/Dark members keep their shortcut (identical bytes, cheaper).
        let sky_cubes: Vec<Arc<[u8]>> = if let Some(surface) = &surface {
            let cells = LightCells::new(&block_buf[..], &states, BDIM);
            let cube = flood::skylight_cube(
                boy - SECTION_SIZE as i32,
                BDIM,
                cells,
                keep,
                surface,
                flood_scratch,
            );
            members
                .iter()
                .map(|m| match m.sky {
                    SkyClass::Full => crate::section::uniform_cube(SKY_FULL),
                    SkyClass::Dark => crate::section::uniform_cube(0),
                    SkyClass::Flood => flood::clip_cube(cube, BDIM, member_off(m)),
                })
                .collect()
        } else {
            members
                .iter()
                .map(|m| match m.sky {
                    SkyClass::Full => crate::section::uniform_cube(SKY_FULL),
                    _ => crate::section::uniform_cube(0),
                })
                .collect()
        };

        // Block light: one joint flood; every member clips its own cube (emitters
        // beyond a member's reach contribute nothing to its 16³, so this matches
        // the per-section result byte for byte).
        let block_cubes: Vec<Arc<[LightRgb]>> = if emitters.is_empty() {
            members.iter().map(|_| crate::light::dark_cube()).collect()
        } else {
            let cells = LightCells::new(&block_buf[..], &states, BDIM);
            let origin = (
                box_ - SECTION_SIZE as i32,
                boy - SECTION_SIZE as i32,
                boz - SECTION_SIZE as i32,
            );
            let cube = flood::block_light_cube(origin, BDIM, cells, keep, &emitters, flood_scratch);
            members
                .iter()
                .map(|m| flood::clip_cube(cube, BDIM, member_off(m)))
                .collect()
        };

        members
            .iter()
            .zip(sky_cubes)
            .zip(block_cubes)
            .map(|((m, skylight), blocklight)| LightBatchOutput {
                pos: m.pos,
                revision: m.revision,
                skylight,
                blocklight,
            })
            .collect()
    })
}

/// Everything this module's relocated tests (in the engine crate) exercise.
/// Test-support builds only; never a public api surface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_exports {
    pub use std::sync::Arc;
    pub use crate::chunk::ChunkPos;
    pub use crate::column::Column;
    pub use rustc_hash::FxHashMap;
    pub use crate::chunk::SECTION_SIZE;
    pub use super::SPAN;
    pub use crate::section::Section;
    pub use crate::chunk::SectionPos;
    pub use crate::chunk::section_idx;
    #[allow(unused_imports)]
    pub use super::*;
}
