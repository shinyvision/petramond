use std::collections::{HashMap, HashSet, VecDeque};

use crate::chunk::{ChunkPos, SECTION_COUNT};
use crate::world::{SectionFace, SectionPos, World, SECTION_FACES};

const MAX_VISIBLE_SECTIONS: usize = 512;
const MAX_SECTION_VISIBILITY_BUILDS_PER_UPDATE: usize = 24;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct SectionVisibilityKey {
    camera_cell: (i32, i32, i32),
    world_revision: u64,
}

#[derive(Default)]
pub(super) struct SectionVisibilityCache {
    key: Option<SectionVisibilityKey>,
    chunk_masks: HashMap<ChunkPos, u16>,
    active: bool,
}

impl SectionVisibilityCache {
    pub(super) fn update(&mut self, world: &mut World, camera: glam::Vec3) {
        let camera_cell = (
            camera.x.floor() as i32,
            camera.y.floor() as i32,
            camera.z.floor() as i32,
        );
        let key = SectionVisibilityKey {
            camera_cell,
            world_revision: world.visibility_revision,
        };
        if self.key == Some(key) {
            return;
        }

        self.chunk_masks.clear();
        self.active = false;
        if let Some(chunk_masks) = compute_visible_sections(world, camera_cell) {
            self.chunk_masks = chunk_masks;
            self.active = true;
        }
        self.key = Some(key);
    }

    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    pub(super) fn chunk_mask(&self, pos: ChunkPos) -> Option<u16> {
        self.active
            .then(|| self.chunk_masks.get(&pos).copied())
            .flatten()
    }

    pub(super) fn visible_section_count(&self) -> u32 {
        if !self.active {
            return 0;
        }
        self.chunk_masks
            .values()
            .map(|mask| mask.count_ones())
            .sum()
    }
}

fn compute_visible_sections(
    world: &mut World,
    camera_cell: (i32, i32, i32),
) -> Option<HashMap<ChunkPos, u16>> {
    let (start, exits) = world.camera_section_exits(camera_cell.0, camera_cell.1, camera_cell.2)?;
    let mut chunk_masks = HashMap::new();
    let mut visited_entries: HashSet<(SectionPos, SectionFace)> = HashSet::new();
    let mut queue = VecDeque::new();
    let mut visible_sections = 0usize;
    let mut built_chunks = 0usize;

    mark_visible(&mut chunk_masks, start, &mut visible_sections)?;
    enqueue_exits(
        world,
        start,
        exits,
        &mut chunk_masks,
        &mut visited_entries,
        &mut queue,
        &mut visible_sections,
        &mut built_chunks,
    )?;

    while let Some((section, entry_face)) = queue.pop_front() {
        let Some(connectivity) = section_connectivity(world, section, &mut built_chunks)? else {
            continue;
        };
        enqueue_exits(
            world,
            section,
            connectivity.exits_from(entry_face),
            &mut chunk_masks,
            &mut visited_entries,
            &mut queue,
            &mut visible_sections,
            &mut built_chunks,
        )?;
    }

    Some(chunk_masks)
}

fn section_connectivity(
    world: &mut World,
    section: SectionPos,
    built_chunks: &mut usize,
) -> Option<Option<crate::world::SectionConnectivity>> {
    let chunk_pos = section.chunk_pos();
    if !world.has_section_visibility(chunk_pos) {
        if !world.chunk_loaded(chunk_pos.cx, chunk_pos.cz) {
            return Some(None);
        }
        if *built_chunks >= MAX_SECTION_VISIBILITY_BUILDS_PER_UPDATE {
            return None;
        }
        if !world.ensure_section_visibility(chunk_pos) {
            return Some(None);
        }
        *built_chunks += 1;
    }
    Some(world.section_connectivity(section))
}

fn enqueue_exits(
    world: &mut World,
    section: SectionPos,
    exits: u8,
    chunk_masks: &mut HashMap<ChunkPos, u16>,
    visited_entries: &mut HashSet<(SectionPos, SectionFace)>,
    queue: &mut VecDeque<(SectionPos, SectionFace)>,
    visible_sections: &mut usize,
    built_chunks: &mut usize,
) -> Option<()> {
    for exit in SECTION_FACES {
        if exits & exit.bit() == 0 {
            continue;
        }
        let Some(next) = section.neighbor(exit) else {
            continue;
        };
        let Some(_) = section_connectivity(world, next, built_chunks)? else {
            continue;
        };

        mark_visible(chunk_masks, next, visible_sections)?;
        let entry = exit.opposite();
        if visited_entries.insert((next, entry)) {
            queue.push_back((next, entry));
        }
    }
    Some(())
}

fn mark_visible(
    chunk_masks: &mut HashMap<ChunkPos, u16>,
    section: SectionPos,
    visible_sections: &mut usize,
) -> Option<()> {
    if section.sy < 0 || section.sy >= SECTION_COUNT as i32 {
        return Some(());
    }
    let bit = 1u16 << section.sy;
    let mask = chunk_masks.entry(section.chunk_pos()).or_default();
    if *mask & bit == 0 {
        *mask |= bit;
        *visible_sections += 1;
        if *visible_sections > MAX_VISIBLE_SECTIONS {
            return None;
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::{Chunk, CHUNK_SY};

    fn insert_visibility_chunk(world: &mut World, pos: ChunkPos, chunk: Chunk) {
        world.chunks.insert(pos, chunk);
        world.rebuild_section_visibility(pos);
    }

    #[test]
    fn solid_neighbor_section_is_visible_but_not_traversed_through() {
        let mut world = World::new(0, 0);
        let left_pos = ChunkPos::new(0, 0);
        let wall_pos = ChunkPos::new(1, 0);
        let behind_pos = ChunkPos::new(2, 0);
        let mut left = Chunk::new(0, 0);
        for z in 0..16 {
            for x in 0..16 {
                left.set_block(x, 16, z, Block::Stone);
            }
        }
        insert_visibility_chunk(&mut world, left_pos, left);

        let mut wall = Chunk::new(1, 0);
        for y in 0..CHUNK_SY {
            for z in 0..16 {
                for x in 0..16 {
                    wall.set_block(x, y, z, Block::Stone);
                }
            }
        }
        insert_visibility_chunk(&mut world, wall_pos, wall);
        insert_visibility_chunk(&mut world, behind_pos, Chunk::new(2, 0));

        let visible = compute_visible_sections(&mut world, (8, 8, 8)).unwrap();

        assert!(visible.get(&left_pos).is_some_and(|m| m & 1 != 0));
        assert!(visible.get(&wall_pos).is_some_and(|m| m & 1 != 0));
        assert!(visible.get(&behind_pos).map_or(true, |m| m & 1 == 0));
    }

    #[test]
    fn open_sky_camera_disables_section_culling() {
        let mut world = World::new(0, 0);
        world.chunks.insert(ChunkPos::new(0, 0), Chunk::new(0, 0));
        world.invalidate_section_visibility(ChunkPos::new(0, 0));

        let visible = compute_visible_sections(&mut world, (8, 80, 8));

        assert!(visible.is_none());
    }
}
