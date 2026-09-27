use crate::block::{Aabb, Block};
use crate::chunk::{self, ChunkPos, SectionPos, SKY_FULL, WORLD_MIN_Y};
use crate::mathh::IVec3;
use crate::section::SectionSummary;

use super::data::WorldData;

pub fn door_support(floor: Block) -> bool {
    floor.is_opaque()
}

const UP: IVec3 = IVec3::new(0, 1, 0);

impl WorldData {
    pub fn loaded_section_count(&self) -> usize {
        self.sections.len()
    }

    pub fn loaded_column_count(&self) -> usize {
        self.columns.len()
    }

    pub fn chunk_block(&self, wx: i32, wy: i32, wz: i32) -> u16 {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.block_raw(lx, ly, lz),
            None => 0,
        }
    }

    pub fn block_if_loaded(&self, wx: i32, wy: i32, wz: i32) -> Option<Block> {
        let (c, lx, ly, lz) = self.chunk_at_world(wx, wy, wz)?;
        Some(c.block(lx, ly, lz))
    }

    pub fn fluid_meta_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.fluid_meta(lx, ly, lz),
            None => 0,
        }
    }

    pub fn skylight_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        if wy < WORLD_MIN_Y {
            return 0;
        }
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.skylight_at(lx, ly, lz),
            None => SKY_FULL,
        }
    }

    pub fn skylight6_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        let l = self.skylight_at_world(wx, wy, wz) as u32;
        ((l * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63) as u8
    }

    pub fn blocklight_rgb_at_world(&self, wx: i32, wy: i32, wz: i32) -> crate::light::LightRgb {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.blocklight_at(lx, ly, lz),
            None => crate::light::LightRgb::ZERO,
        }
    }

    pub fn blocklight_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        self.blocklight_rgb_at_world(wx, wy, wz).luminance()
    }

    pub fn blocklight6_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        let l = self.blocklight_at_world(wx, wy, wz) as u32;
        ((l * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63) as u8
    }

    pub fn blocklight6_rgb_at_world(&self, wx: i32, wy: i32, wz: i32) -> [u8; 3] {
        crate::light::BlockLight6::from_x2(self.blocklight_rgb_at_world(wx, wy, wz))
            .channels()
            .map(|c| c as u8)
    }

    pub fn combined_light6_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        self.skylight6_at_world(wx, wy, wz)
            .max(self.blocklight6_at_world(wx, wy, wz))
    }

    pub fn dynamic_light_at_world(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
    ) -> (u8, crate::light::BlockLight6) {
        (
            self.skylight6_at_world(wx, wy, wz),
            crate::light::BlockLight6::from_x2(self.blocklight_rgb_at_world(wx, wy, wz)),
        )
    }

    pub fn column_biome(&self, wx: i32, wz: i32) -> Option<u8> {
        self.columns
            .get(&ChunkPos::new(wx >> 4, wz >> 4))
            .map(|c| c.biome_at(chunk::lx(wx), chunk::lz(wz)))
    }

    pub fn chunk_loaded(&self, cx: i32, cz: i32) -> bool {
        self.section_column_cys
            .contains_key(&crate::chunk::ChunkPos::new(cx, cz))
    }

    pub fn placement_cell_open(&self, c: IVec3) -> bool {
        if !self.chunk_loaded(c.x >> 4, c.z >> 4) {
            return false;
        }
        let Some(pos) = SectionPos::from_world(c.x, c.y, c.z) else {
            return false;
        };
        if self.sections.contains_key(&pos) {
            return Block::from_id(self.chunk_block(c.x, c.y, c.z)).is_replaceable();
        }
        match self.section_summary(pos) {
            SectionSummary::Empty => true,
            full @ SectionSummary::FullWater => full.virtual_block().is_replaceable(),
            SectionSummary::Unknown => self.absent_cell_above_known_surface(c, pos),
            SectionSummary::FullOpaque | SectionSummary::Mixed => false,
        }
    }

    #[inline]
    fn absent_cell_above_known_surface(&self, c: IVec3, pos: SectionPos) -> bool {
        if self.saved_section_contains(pos) {
            return false;
        }
        self.columns
            .get(&pos.chunk_pos())
            .is_some_and(|col| c.y > col.surface_y(chunk::lx(c.x), chunk::lz(c.z)))
    }

    pub fn loaded_area(&self) -> Option<(i32, i32, i32)> {
        self.last_load_target
            .map(|t| (t.center.cx, t.center.cz, t.render_dist))
    }

    pub fn surface_collision_y(&self, wx: i32, wz: i32) -> Option<i32> {
        let col = self.columns.get(&ChunkPos::new(wx >> 4, wz >> 4))?;
        let top = col.surface_y(chunk::lx(wx), chunk::lz(wz));
        let cur = self.cursor();
        (WORLD_MIN_Y..=top)
            .rev()
            .find(|&y| cur.blocks_movement(IVec3::new(wx, y, wz)))
    }

    pub fn biome_at_world(&self, wx: i32, wz: i32) -> Option<u8> {
        let col = self.columns.get(&ChunkPos::new(wx >> 4, wz >> 4))?;
        Some(col.biome_at(chunk::lx(wx), chunk::lz(wz)))
    }

    pub fn precipitation_ceiling_y(&self, wx: i32, wz: i32) -> Option<i32> {
        let col = self.columns.get(&ChunkPos::new(wx >> 4, wz >> 4))?;
        let top = col.surface_y(chunk::lx(wx), chunk::lz(wz));
        let cur = self.cursor();
        (WORLD_MIN_Y..=top).rev().find(|&y| {
            let c = IVec3::new(wx, y, wz);
            cur.blocks_movement(c) || cur.block_if_loaded(c).is_some_and(|b| b.is_fluid())
        })
    }

    pub fn block_is_full_spawn_support(&self, wx: i32, wy: i32, wz: i32) -> bool {
        let Some(block) = self.block_if_loaded(wx, wy, wz) else {
            return false;
        };
        if block.is_fluid() || block.is_leaves() {
            return false;
        }
        full_unit_cube(self.collision_boxes_at(wx, wy, wz))
    }

    pub fn collision_shape_class(&self, wx: i32, wy: i32, wz: i32) -> CollisionShapeClass {
        let boxes = self.collision_boxes_at(wx, wy, wz);
        if boxes.is_empty() {
            CollisionShapeClass::Empty
        } else if full_unit_cube(boxes) {
            CollisionShapeClass::Full
        } else {
            CollisionShapeClass::Partial
        }
    }
}

pub fn full_unit_cube(boxes: &[Aabb]) -> bool {
    if boxes.len() != 1 {
        return false;
    }
    let b = boxes[0];
    b.min == [0.0, 0.0, 0.0] && b.max == [1.0, 1.0, 1.0]
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CollisionShapeClass {
    Empty,
    Partial,
    Full,
}

impl WorldData {
    pub fn door_footprint_clear(&self, base: IVec3) -> bool {
        let upper = base + UP;
        let floor = self.physics_block(base.x, base.y - 1, base.z);
        self.placement_cell_open(base) && self.placement_cell_open(upper) && door_support(floor)
    }
}
