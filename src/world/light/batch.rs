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

use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::chunk::{section_idx, ChunkPos, SectionPos, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::column::Column;
use crate::mathh::IVec3;
use crate::section::Section;

use super::shape::{LightCells, ShapeStateSnapshot, SparseCellState};
use super::skylight::SkyClass;
use super::{flood, neighborhood, skylight};

/// Sections per axis in one batch group.
pub(in crate::world) const GROUP: i32 = 2;
/// Sections per axis of the gathered neighbourhood (the group plus a one-section halo).
const SPAN: usize = GROUP as usize + 2;
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
pub(in crate::world) struct LightBatchJob {
    base: SectionPos,
    members: Vec<BatchMember>,
    /// `SPAN`³ field-`Arc` block buffers (`None` = absent, reads as air).
    blocks: Vec<Option<Arc<[u8]>>>,
    states: Vec<SparseCellState>,
    /// `BDIM`² sky-cover map, gathered only when a member needs the sky flood.
    surface: Option<Box<[i32]>>,
    emitters: Vec<(IVec3, u8)>,
}

pub(in crate::world) struct LightBatchOutput {
    pub pos: SectionPos,
    pub revision: u64,
    pub skylight: Arc<[u8]>,
    pub blocklight: Arc<[u8]>,
}

impl LightBatchJob {
    /// The members this job will actually bake (snapshot may have skipped
    /// requested positions whose section was absent).
    pub(in crate::world) fn member_positions(&self) -> impl Iterator<Item = SectionPos> + '_ {
        self.members.iter().map(|m| m.pos)
    }

    /// Drop members whose per-member cancellation fired while the job was
    /// queued; the shared snapshot stays valid for the rest.
    pub(in crate::world) fn retain_members(&mut self, keep: impl Fn(SectionPos) -> bool) {
        self.members.retain(|m| keep(m.pos));
    }

    pub(in crate::world) fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// Group section positions into their 2×2×2-aligned batches: `(group base, members)`.
pub(in crate::world) fn group_positions(
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

/// Snapshot one batch: the same cheap per-section handles [`super::queue::LightBakeJob`]
/// takes, gathered once for the whole group.
pub(in crate::world) fn snapshot_batch(
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
    let mut blocks: Vec<Option<Arc<[u8]>>> = vec![None; SPAN * SPAN * SPAN];
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
                blocks[span_idx(dx, dy, dz)] = Some(section.blocks_arc());
                let (bx, by, bz) = (dx * SECTION_SIZE, dy * SECTION_SIZE, dz * SECTION_SIZE);
                states.extend(section.stair_states().iter().map(|(&key, &state)| {
                    let (lx, ly, lz) = crate::chunk::section_local(key as usize);
                    SparseCellState::Stair {
                        idx: bidx(bx + lx, by + ly, bz + lz),
                        state,
                    }
                }));
                states.extend(section.slab_states().iter().map(|(&key, &state)| {
                    let (lx, ly, lz) = crate::chunk::section_local(key as usize);
                    SparseCellState::Slab {
                        idx: bidx(bx + lx, by + ly, bz + lz),
                        state,
                    }
                }));
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
fn assemble_blocks(arcs: &[Option<Arc<[u8]>>], out: &mut [u8]) {
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
                        out[d..d + SECTION_SIZE].copy_from_slice(&src[s..s + SECTION_SIZE]);
                    }
                }
            }
        }
    }
}

struct BatchScratch {
    blocks: Vec<u8>,
    flood: flood::FloodScratch,
}

thread_local! {
    static BATCH_SCRATCH: std::cell::RefCell<BatchScratch> =
        std::cell::RefCell::new(BatchScratch {
            blocks: vec![0u8; BVOL],
            flood: flood::FloodScratch::new(),
        });
}

pub(in crate::world) fn run_light_bake_batch(job: LightBatchJob) -> Vec<LightBatchOutput> {
    let t_stage = std::time::Instant::now();
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
                surface,
                flood_scratch,
            );
            members
                .iter()
                .map(|m| match m.sky {
                    SkyClass::Full => vec![SKY_FULL; SECTION_VOLUME].into(),
                    SkyClass::Dark => vec![0u8; SECTION_VOLUME].into(),
                    SkyClass::Flood => flood::clip_cube(cube, BDIM, member_off(m)),
                })
                .collect()
        } else {
            members
                .iter()
                .map(|m| match m.sky {
                    SkyClass::Full => vec![SKY_FULL; SECTION_VOLUME].into(),
                    _ => vec![0u8; SECTION_VOLUME].into(),
                })
                .collect()
        };

        // Block light: one joint flood; every member clips its own cube (emitters
        // beyond a member's reach contribute nothing to its 16³, so this matches
        // the per-section result byte for byte).
        let block_cubes: Vec<Arc<[u8]>> = if emitters.is_empty() {
            members
                .iter()
                .map(|_| vec![0u8; SECTION_VOLUME].into())
                .collect()
        } else {
            let cells = LightCells::new(&block_buf[..], &states, BDIM);
            let origin = (
                box_ - SECTION_SIZE as i32,
                boy - SECTION_SIZE as i32,
                boz - SECTION_SIZE as i32,
            );
            let cube = flood::block_light_cube(origin, BDIM, cells, &emitters, flood_scratch);
            members
                .iter()
                .map(|m| flood::clip_cube(cube, BDIM, member_off(m)))
                .collect()
        };

        super::queue::LIGHT_STAGE_NS.fetch_add(
            t_stage.elapsed().as_nanos() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        super::queue::LIGHT_STAGE_JOBS
            .fetch_add(members.len() as u64, std::sync::atomic::Ordering::Relaxed);

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

#[cfg(test)]
mod tests {
    use super::super::queue::{run_light_bake, LightBakeJob};
    use super::*;
    use crate::block::Block;
    use crate::block_state::{StairHalf, StairState};
    use crate::facing::Facing;
    use crate::torch::TorchPlacement;

    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// Randomized rough terrain with caves, stairs, and torches across the whole
    /// 4×4×4 span (some sections absent), members straddling the surface band so
    /// Full, Dark, and Flood classifications all occur across rounds.
    #[test]
    fn batched_bake_matches_per_section_bakes() {
        let base = SectionPos::new(0, 0, 0);
        let mut rng = 0xdead_beef_cafe_1234u64;

        for round in 0..3 {
            let mut sections: FxHashMap<SectionPos, Arc<Section>> = FxHashMap::default();
            let mut columns: FxHashMap<ChunkPos, Column> = FxHashMap::default();

            // Per world column (4×4 chunk columns × 16² cells): a rough height.
            let mut heights = vec![0i32; (SPAN * SECTION_SIZE) * (SPAN * SECTION_SIZE)];
            for h in heights.iter_mut() {
                *h = (xorshift(&mut rng) % 56) as i32 - 12;
            }

            for dy in 0..SPAN {
                for dz in 0..SPAN {
                    for dx in 0..SPAN {
                        // Leave a few sections absent: absent neighbours read as air.
                        if xorshift(&mut rng) % 9 == 0 {
                            continue;
                        }
                        let pos = SectionPos::new(dx as i32 - 1, dy as i32 - 1, dz as i32 - 1);
                        let (_, oy, _) = pos.origin_world();
                        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
                        for ly in 0..SECTION_SIZE {
                            for lz in 0..SECTION_SIZE {
                                for lx in 0..SECTION_SIZE {
                                    let gx = dx * SECTION_SIZE + lx;
                                    let gz = dz * SECTION_SIZE + lz;
                                    let h = heights[gz * SPAN * SECTION_SIZE + gx];
                                    let wy = oy + ly as i32;
                                    // Solid below the surface with random cave holes.
                                    if wy <= h && xorshift(&mut rng) % 8 != 0 {
                                        section.set_block(lx, ly, lz, Block::Stone);
                                    } else if xorshift(&mut rng) % 401 == 0 {
                                        section.set_block(lx, ly, lz, Block::OakStairs);
                                        section.set_stair_state(
                                            lx,
                                            ly,
                                            lz,
                                            StairState::new(Facing::East, StairHalf::Bottom),
                                        );
                                    } else if xorshift(&mut rng) % 353 == 0 {
                                        section.set_block(lx, ly, lz, Block::Torch);
                                        section.insert_torch(lx, ly, lz, TorchPlacement::Floor);
                                    }
                                }
                            }
                        }
                        sections.insert(pos, Arc::new(section));
                    }
                }
            }

            // Sky cover derived from the ACTUAL blocks (topmost cell that blocks
            // direct skylight), the invariant the engine maintains. Fabricating
            // cover independently of blocks creates phantom full-skylight shafts
            // through which the undecayed down rule tunnels arbitrarily deep —
            // which is exactly where 48³ and 64³ cubes may legitimately disagree.
            for dcz in 0..SPAN {
                for dcx in 0..SPAN {
                    let mut col = Column::new();
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            let mut cover = crate::column::NO_SURFACE;
                            'scan: for dy in (0..SPAN).rev() {
                                let pos =
                                    SectionPos::new(dcx as i32 - 1, dy as i32 - 1, dcz as i32 - 1);
                                let Some(section) = sections.get(&pos) else {
                                    continue;
                                };
                                let blocks = section.blocks_arc();
                                for ly in (0..SECTION_SIZE).rev() {
                                    let b = Block::from_id(blocks[section_idx(lx, ly, lz)]);
                                    if !b.transmits_direct_skylight() {
                                        cover = pos.origin_world().1 + ly as i32;
                                        break 'scan;
                                    }
                                }
                            }
                            col.set_surface_y(lx, lz, cover);
                            col.set_sky_cover_y(lx, lz, cover);
                        }
                    }
                    columns.insert(ChunkPos::new(dcx as i32 - 1, dcz as i32 - 1), col);
                }
            }

            let member_positions: Vec<SectionPos> = (0..GROUP)
                .flat_map(|my| {
                    (0..GROUP).flat_map(move |mz| {
                        (0..GROUP).map(move |mx| {
                            SectionPos::new(base.cx + mx, base.cy + my, base.cz + mz)
                        })
                    })
                })
                .filter(|p| sections.contains_key(p))
                .collect();
            assert!(
                !member_positions.is_empty(),
                "fixture produced an empty group in round {round}"
            );

            let job = snapshot_batch(base, &member_positions, &sections, &columns)
                .expect("batch snapshot");
            let batched = run_light_bake_batch(job);
            assert_eq!(batched.len(), member_positions.len());

            let report_first_diff = |label: &str, pos: SectionPos, got: &[u8], want: &[u8]| {
                for i in 0..SECTION_VOLUME {
                    if got[i] != want[i] {
                        let (lx, ly, lz) = crate::chunk::section_local(i);
                        panic!(
                            "{label} mismatch at {pos:?} cell ({lx},{ly},{lz}) in round \
                             {round}: batched {} vs per-section {}",
                            got[i], want[i]
                        );
                    }
                }
            };
            for out in batched {
                let job = LightBakeJob::snapshot_unchecked(1, out.pos, &sections, &columns)
                    .expect("per-section snapshot");
                let want = run_light_bake(job);
                report_first_diff("skylight", out.pos, &out.skylight, &want.skylight);
                report_first_diff("block light", out.pos, &out.blocklight, &want.blocklight);
            }
        }
    }
}
