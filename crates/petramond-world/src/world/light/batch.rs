//! Batched 2×2×2 light bake: one 64³ flood shared by up to eight sections instead of eight
//! overlapping 48³ floods.
//!
//! Matches the per-section bake byte for byte because light influence is capped. `SKY_FULL`
//! cells are the same above-cover cells in both cube sizes, and everything else decays 2 per
//! step, so nothing past 15 cells reaches a section's 16³ result. That reach fits inside both
//! the 48³ per-section cube and the 64³ batch cube. This relies on sky-cover staying consistent
//! with the blocks (a cover cell never lets direct skylight through). Otherwise the straight-down
//! rule could tunnel skylight through a phantom shaft at depths where the two cube sizes
//! disagree. Covered by `batched_bake_matches_per_section_bakes`.
//!
//! Sky shortcuts still apply per member: `Full`/`Dark` members skip the flood, and a group with
//! no flooding member does nothing.
//!
//! Colour doesn't change the reach argument. Each channel decays 2 per step on its own and never
//! exceeds the row's `emission`, so it's bounded by the same 15 cells as the scalar case.
//!
//! The ≥3-member grouping threshold in `stream::settle` still holds with the wider cell. It comes
//! from 64³/48³ = 2.37, and below three members the shared cube touches more cells than separate
//! floods would. Widening the block cell scales both cube sizes by the same factor, so that ratio
//! (and the break-even point) doesn't move. The BFS visits the same cells either way.

use crate::world::section_map::SectionMap;
use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE, SKY_FULL};
use crate::column::Column;
use crate::light::LightRgb;
use crate::mathh::IVec3;

use super::bake::LightBakeOutput;
use super::shape::{with_light_cells, ApertureScratch, Ids};
use super::skylight::SkyClass;
use super::{flood, neighborhood, skylight};

pub const GROUP: i32 = 2;
pub const SPAN: usize = GROUP as usize + 2;
const BDIM: usize = SPAN * SECTION_SIZE;
const BVOL: usize = BDIM * BDIM * BDIM;

struct BatchMember {
    pos: SectionPos,
    revision: u64,
    sky: SkyClass,
}

pub struct LightBatchJob {
    base: SectionPos,
    members: Vec<BatchMember>,
    nbhd: Option<neighborhood::Snapshot>,
    surface: Option<Box<[i32]>>,
    emitters: Vec<(IVec3, LightRgb)>,
}

impl LightBatchJob {
    pub fn member_positions(&self) -> impl Iterator<Item = SectionPos> + '_ {
        self.members.iter().map(|m| m.pos)
    }

    pub fn retain_members(&mut self, keep: impl Fn(SectionPos) -> bool) {
        self.members.retain(|m| keep(m.pos));
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

pub fn group_positions(positions: &[SectionPos]) -> Vec<(SectionPos, Vec<SectionPos>)> {
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

pub fn snapshot_batch(
    base: SectionPos,
    member_positions: &[SectionPos],
    sections: &SectionMap,
    columns: &FxHashMap<ChunkPos, std::sync::Arc<Column>>,
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

    let low = SectionPos::new(base.cx - 1, base.cy - 1, base.cz - 1);
    let emitters = neighborhood::collect_emitters(low, SPAN, sections);
    let nbhd = (any_flood || !emitters.is_empty())
        .then(|| neighborhood::Snapshot::gather(low, SPAN, sections));

    let surface = any_flood.then(|| {
        skylight::gather_surface_span(ChunkPos::new(base.cx - 1, base.cz - 1), SPAN, columns)
    });

    Some(LightBatchJob {
        base,
        members,
        nbhd,
        surface,
        emitters,
    })
}

struct BatchScratch {
    blocks: Vec<u16>,
    narrow: Vec<u8>,
    apertures: ApertureScratch,
    flood: flood::FloodScratch,
}

thread_local! {
    static BATCH_SCRATCH: std::cell::RefCell<BatchScratch> =
        std::cell::RefCell::new(BatchScratch {
            blocks: vec![0u16; BVOL],
            narrow: vec![0u8; BVOL],
            apertures: ApertureScratch::default(),
            flood: flood::FloodScratch::new(),
        });
}

pub fn run_light_bake_batch(job: LightBatchJob) -> Vec<LightBakeOutput> {
    let LightBatchJob {
        base,
        members,
        nbhd,
        surface,
        emitters,
    } = job;

    BATCH_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let BatchScratch {
            blocks: block_buf,
            narrow: narrow_buf,
            apertures,
            flood: flood_scratch,
        } = &mut *scratch;

        let ids: Option<Ids<'_>> = nbhd.as_ref().map(|n| {
            n.fill_apertures(apertures);
            if n.all_narrow() {
                n.assemble_narrow(narrow_buf);
                Ids::Narrow(&narrow_buf[..])
            } else {
                n.assemble_blocks(block_buf);
                Ids::Wide(&block_buf[..])
            }
        });
        let apertures = &*apertures;
        let keep = flood::Keep::new(SECTION_SIZE, SECTION_SIZE * (1 + GROUP as usize));
        let (box_, boy, boz) = base.origin_world();
        let member_off = |m: &BatchMember| {
            (
                ((m.pos.cx - base.cx + 1) as usize) * SECTION_SIZE,
                ((m.pos.cy - base.cy + 1) as usize) * SECTION_SIZE,
                ((m.pos.cz - base.cz + 1) as usize) * SECTION_SIZE,
            )
        };

        let sky_cubes: Vec<Arc<[u8]>> = if let Some(surface) = &surface {
            let ids = ids.as_ref().expect("a flooding batch carries its blocks");
            let cube = with_light_cells!(*ids, apertures, BDIM, |cells| flood::skylight_cube(
                boy - SECTION_SIZE as i32,
                BDIM,
                cells,
                keep,
                surface,
                flood_scratch,
            ));
            members
                .iter()
                .map(|m| match m.sky {
                    SkyClass::Full => crate::section::uniform_cube(SKY_FULL),
                    SkyClass::Dark => crate::section::uniform_cube(0),
                    SkyClass::Flood => flood::clip_sky_cube(cube, BDIM, member_off(m)),
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

        let block_cubes: Vec<Arc<[LightRgb]>> = if emitters.is_empty() {
            members.iter().map(|_| crate::light::dark_cube()).collect()
        } else {
            let ids = ids
                .as_ref()
                .expect("a block-light batch carries its blocks");
            let origin = (
                box_ - SECTION_SIZE as i32,
                boy - SECTION_SIZE as i32,
                boz - SECTION_SIZE as i32,
            );
            let cube = with_light_cells!(*ids, apertures, BDIM, |cells| flood::block_light_cube(
                origin,
                BDIM,
                cells,
                keep,
                &emitters,
                flood_scratch
            ));
            members
                .iter()
                .map(|m| flood::clip_block_cube(cube, BDIM, member_off(m)))
                .collect()
        };

        members
            .iter()
            .zip(sky_cubes)
            .zip(block_cubes)
            .map(|((m, skylight), blocklight)| LightBakeOutput {
                pos: m.pos,
                revision: m.revision,
                skylight,
                blocklight,
            })
            .collect()
    })
}
