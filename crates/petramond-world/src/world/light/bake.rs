use crate::world::section_map::SectionMap;
use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::chunk::{ChunkPos, SectionPos, SKY_FULL};
use crate::column::Column;
use crate::light::LightRgb;
use crate::mathh::IVec3;

use super::shape::{with_light_cells, ApertureScratch, Ids};
use super::skylight::{self, SkyPlan};
use super::{flood, neighborhood, NBHD, NBHD_VOLUME};

pub struct LightBakeOutput {
    pub pos: SectionPos,
    pub revision: u64,
    pub skylight: Arc<[u8]>,
    pub blocklight: Arc<[LightRgb]>,
}

pub struct SectionBakeJob {
    pos: SectionPos,
    revision: u64,
    sky: SkyPlan,
    nbhd: Option<neighborhood::Snapshot>,
    emitters: Vec<(IVec3, LightRgb)>,
}

impl SectionBakeJob {
    pub fn snapshot(
        pos: SectionPos,
        sections: &SectionMap,
        columns: &FxHashMap<ChunkPos, Arc<Column>>,
    ) -> Option<Self> {
        if !sections.get(&pos)?.light_dirty {
            return None;
        }
        Self::snapshot_unchecked(pos, sections, columns)
    }

    pub fn snapshot_unchecked(
        pos: SectionPos,
        sections: &SectionMap,
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

struct BakeScratch {
    blocks: Box<[u16]>,
    narrow: Box<[u8]>,
    apertures: ApertureScratch,
    flood: flood::FloodScratch,
}

thread_local! {
    static BAKE_SCRATCH: std::cell::RefCell<BakeScratch> = std::cell::RefCell::new(BakeScratch {
        blocks: vec![0u16; NBHD_VOLUME].into_boxed_slice(),
        narrow: vec![0u8; NBHD_VOLUME].into_boxed_slice(),
        apertures: ApertureScratch::default(),
        flood: flood::FloodScratch::new(),
    });
}

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
        let BakeScratch {
            blocks,
            narrow,
            apertures,
            flood,
        } = &mut *scratch;

        // Byte ids stay bytes: the sections' own storage is narrow for every ordinary world, and
        // a byte cube halves the flood's working set.
        let ids: Option<Ids<'_>> = nbhd.as_ref().map(|n| {
            n.fill_apertures(apertures);
            if n.all_narrow() {
                n.assemble_narrow(narrow);
                Ids::Narrow(&narrow[..])
            } else {
                n.assemble_blocks(blocks);
                Ids::Wide(&blocks[..])
            }
        });
        let apertures = &*apertures;

        let skylight = match sky {
            SkyPlan::Full => crate::section::uniform_cube(SKY_FULL),
            SkyPlan::Dark => crate::section::uniform_cube(0),
            SkyPlan::Flood { surface } => {
                let ids = ids
                    .as_ref()
                    .expect("a flooding skylight bake carries its neighbourhood blocks");
                with_light_cells!(*ids, apertures, NBHD, |cells| flood::skylight(
                    pos, cells, &surface, flood
                ))
            }
        };

        let blocklight = if emitters.is_empty() {
            crate::light::dark_cube()
        } else {
            let ids = ids
                .as_ref()
                .expect("a block-light bake carries its neighbourhood blocks");
            with_light_cells!(*ids, apertures, NBHD, |cells| flood::block_light(
                pos, cells, &emitters, flood
            ))
        };

        LightBakeOutput {
            pos,
            revision,
            skylight,
            blocklight,
        }
    })
}
