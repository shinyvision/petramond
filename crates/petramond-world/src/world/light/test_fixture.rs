use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::block::Block;
use crate::block_state::{StairHalf, StairState};
use crate::chunk::{section_idx, ChunkPos, SectionPos, SECTION_SIZE, SECTION_VOLUME};
use crate::column::Column;
use crate::facing::Facing;
use crate::light::LightRgb;
use crate::mathh::IVec3;
use crate::section::Section;
use crate::torch::TorchPlacement;

use super::bake::{bake_section, LightBakeOutput, SectionBakeJob};

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn one_in(&mut self, n: u64) -> bool {
        self.next().is_multiple_of(n)
    }
}

pub struct Fixture {
    pub sections: FxHashMap<SectionPos, Arc<Section>>,
    pub columns: FxHashMap<ChunkPos, Arc<Column>>,
    pub low: SectionPos,
    pub span: usize,
}

const FACINGS: [Facing; 4] = [Facing::North, Facing::East, Facing::South, Facing::West];

impl Fixture {
    pub fn random(rng: &mut Rng, low: SectionPos, span: usize, absent_one_in: u64) -> Self {
        let dim = span * SECTION_SIZE;
        let heights: Vec<i32> = (0..dim * dim)
            .map(|_| low.cy * SECTION_SIZE as i32 + (rng.next() % 56) as i32 + 4)
            .collect();
        let mut sections = FxHashMap::default();
        for dy in 0..span {
            for dz in 0..span {
                for dx in 0..span {
                    if absent_one_in != 0 && rng.one_in(absent_one_in) {
                        continue;
                    }
                    let pos =
                        SectionPos::new(low.cx + dx as i32, low.cy + dy as i32, low.cz + dz as i32);
                    let (_, oy, _) = pos.origin_world();
                    let mut section = Section::new(pos.cx, pos.cy, pos.cz);
                    for ly in 0..SECTION_SIZE {
                        for lz in 0..SECTION_SIZE {
                            for lx in 0..SECTION_SIZE {
                                let h = heights
                                    [(dz * SECTION_SIZE + lz) * dim + dx * SECTION_SIZE + lx];
                                if oy + ly as i32 <= h && !rng.one_in(8) {
                                    section.set_block(lx, ly, lz, Block::Stone);
                                } else {
                                    random_feature(rng, &mut section, lx, ly, lz);
                                }
                            }
                        }
                    }
                    section.mark_light_clean();
                    sections.insert(pos, Arc::new(section));
                }
            }
        }
        let mut fixture = Self {
            sections,
            columns: FxHashMap::default(),
            low,
            span,
        };
        fixture.columns = fixture.derived_columns();
        fixture
    }

    pub fn window(&self) -> impl Iterator<Item = SectionPos> + '_ {
        let (low, span) = (self.low, self.span as i32);
        (0..span).flat_map(move |dy| {
            (0..span).flat_map(move |dz| {
                (0..span).map(move |dx| SectionPos::new(low.cx + dx, low.cy + dy, low.cz + dz))
            })
        })
    }

    /// Sky cover per window column: the topmost loaded cell that stops direct
    /// skylight. Fabricating cover independently of blocks creates phantom
    /// full-skylight shafts through which the undecayed down rule tunnels
    /// arbitrarily deep — a state the engine never produces.
    pub fn derived_columns(&self) -> FxHashMap<ChunkPos, Arc<Column>> {
        let mut columns = FxHashMap::default();
        for dcz in 0..self.span as i32 {
            for dcx in 0..self.span as i32 {
                let cp = ChunkPos::new(self.low.cx + dcx, self.low.cz + dcz);
                let mut col = Column::new();
                for lz in 0..SECTION_SIZE {
                    for lx in 0..SECTION_SIZE {
                        let cover = self.cover_at(cp, lx, lz);
                        col.set_surface_y(lx, lz, cover);
                        col.set_sky_cover_y(lx, lz, cover);
                    }
                }
                columns.insert(cp, Arc::new(col));
            }
        }
        columns
    }

    fn cover_at(&self, cp: ChunkPos, lx: usize, lz: usize) -> i32 {
        for dy in (0..self.span as i32).rev() {
            let pos = SectionPos::new(cp.cx, self.low.cy + dy, cp.cz);
            let Some(section) = self.sections.get(&pos) else {
                continue;
            };
            for ly in (0..SECTION_SIZE).rev() {
                if !section.block(lx, ly, lz).transmits_direct_skylight() {
                    return pos.origin_world().1 + ly as i32;
                }
            }
        }
        crate::column::NO_SURFACE
    }

    pub fn full_bake(&self, pos: SectionPos) -> LightBakeOutput {
        let job = SectionBakeJob::snapshot_unchecked(pos, &self.sections, &self.columns)
            .expect("fixture section is present");
        bake_section(job)
    }

    pub fn bake_all(&mut self) {
        let positions: Vec<SectionPos> = self.sections.keys().copied().collect();
        let outs: Vec<LightBakeOutput> = positions.iter().map(|&p| self.full_bake(p)).collect();
        for out in outs {
            let section = Arc::make_mut(self.sections.get_mut(&out.pos).unwrap());
            section.set_skylight(out.skylight);
            section.set_blocklight(out.blocklight);
        }
    }

    pub fn install(&mut self, pos: SectionPos, skylight: Arc<[u8]>, blocklight: Arc<[LightRgb]>) {
        let section = Arc::make_mut(self.sections.get_mut(&pos).unwrap());
        section.set_skylight(skylight);
        section.set_blocklight(blocklight);
    }

    pub fn cell_mut(&mut self, cell: IVec3) -> Option<(&mut Section, usize, usize, usize)> {
        let pos = SectionPos::from_world(cell.x, cell.y, cell.z)?;
        let section = Arc::make_mut(self.sections.get_mut(&pos)?);
        let (ox, oy, oz) = pos.origin_world();
        Some((
            section,
            (cell.x - ox) as usize,
            (cell.y - oy) as usize,
            (cell.z - oz) as usize,
        ))
    }

    pub fn assert_matches_full_bakes(&self, label: &str) {
        for pos in self.window() {
            let Some(section) = self.sections.get(&pos) else {
                continue;
            };
            if !section.has_baked_light() {
                assert!(section.all_opaque(), "{label}: {pos:?} holds no light");
                continue;
            }
            let want = self.full_bake(pos);
            let sky = section.skylight_arc().expect("fixture light is baked");
            let block = section.blocklight_arc();
            let block: &[LightRgb] = block.as_deref().unwrap_or(&super::ZERO_CUBE[..]);
            report_first_diff(label, "skylight", pos, &sky, &want.skylight);
            report_first_diff(label, "block light", pos, block, &want.blocklight);
        }
    }
}

fn random_feature(rng: &mut Rng, section: &mut Section, lx: usize, ly: usize, lz: usize) {
    if rng.one_in(401) {
        place_stair(rng, section, lx, ly, lz);
    } else if rng.one_in(353) {
        section.set_block(lx, ly, lz, Block::Torch);
        section.insert_torch(lx, ly, lz, TorchPlacement::Floor);
    } else if rng.one_in(911) {
        section.set_block(lx, ly, lz, Block::Glass);
    }
}

pub fn place_stair(rng: &mut Rng, section: &mut Section, lx: usize, ly: usize, lz: usize) {
    section.set_block(lx, ly, lz, Block::OakStairs);
    let facing = FACINGS[(rng.next() % 4) as usize];
    section.set_stair_state(lx, ly, lz, StairState::new(facing, StairHalf::Bottom));
    if rng.one_in(3) {
        section.set_custom_light_aperture(section_idx(lx, ly, lz) as u16, rng.one_in(2));
    }
}

pub fn report_first_diff<T: PartialEq + std::fmt::Debug>(
    label: &str,
    what: &str,
    pos: SectionPos,
    got: &[T],
    want: &[T],
) {
    for i in 0..SECTION_VOLUME {
        if got[i] != want[i] {
            let (lx, ly, lz) = crate::chunk::section_local(i);
            panic!(
                "{label}: {what} mismatch at {pos:?} cell ({lx},{ly},{lz}): got {:?}, \
                 full bake {:?}",
                got[i], want[i]
            );
        }
    }
}
