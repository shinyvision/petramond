use std::collections::{HashMap, HashSet, VecDeque};

use crate::camera::Frustum;
use crate::chunk::{ChunkPos, CHUNK_SY, SECTION_COUNT, SECTION_SIZE};
use crate::mesh::MeshIndexSection;
use crate::world::{SectionFace, SectionPos, World, SECTION_FACES};

const MAX_VISIBLE_SECTIONS: usize = 512;
const MAX_SECTION_VISIBILITY_BUILDS_PER_UPDATE: usize = 24;

/// Below this many indices saved, a fragmented per-section draw list isn't worth
/// the extra draw calls; the chunk falls back to one full-mesh draw.
const MIN_SECTION_CULL_INDEX_SAVINGS: u32 = 2_048;

/// The contiguous index ranges to submit for one chunk after section culling,
/// plus the bookkeeping the render loop reports as stats. Built once per visible
/// chunk per frame by [`section_draw_ranges`].
pub(super) struct SectionDrawRanges {
    ranges: [(u32, u32); SECTION_COUNT],
    len: usize,
    /// Total index count across `ranges` (≤ the chunk's full index count).
    pub(super) submitted: u32,
}

impl SectionDrawRanges {
    fn new() -> Self {
        Self {
            ranges: [(0, 0); SECTION_COUNT],
            len: 0,
            submitted: 0,
        }
    }

    fn full(index_count: u32) -> Self {
        let mut out = Self::new();
        if index_count > 0 {
            out.ranges[0] = (0, index_count);
            out.len = 1;
            out.submitted = index_count;
        }
        out
    }

    fn push(&mut self, start: u32, end: u32) {
        if start >= end {
            return;
        }
        if self.len > 0 && self.ranges[self.len - 1].1 == start {
            self.ranges[self.len - 1].1 = end;
        } else {
            self.ranges[self.len] = (start, end);
            self.len += 1;
        }
        self.submitted += end - start;
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.ranges[..self.len].iter().copied()
    }
}

/// Compute the index ranges to draw for one chunk: the visible sections (per the
/// connectivity `visible_mask`) that are also inside the view `frustum`, coalesced
/// into contiguous runs. Falls back to one full-mesh draw when the per-section
/// fragmentation wouldn't save enough indices to be worth the extra draw calls.
pub(super) fn section_draw_ranges(
    frustum: Frustum,
    origin: (i32, i32),
    full_idx_count: u32,
    sections: &[MeshIndexSection; SECTION_COUNT],
    visible_mask: u16,
) -> SectionDrawRanges {
    let mut out = SectionDrawRanges::new();
    for (section_idx, section) in sections.iter().enumerate() {
        if visible_mask & (1u16 << section_idx) == 0 || section.index_count == 0 {
            continue;
        }
        if !section_visible(frustum, origin, section_idx) {
            continue;
        }
        out.push(
            section.first_index,
            section.first_index + section.index_count,
        );
    }

    if out.is_empty() || out.submitted >= full_idx_count {
        return out;
    }
    if out.len == 1 {
        return out;
    }

    let saved = full_idx_count - out.submitted;
    let saves_enough_indices = saved >= MIN_SECTION_CULL_INDEX_SAVINGS;
    let saves_enough_ratio = (out.submitted as u64) * 4 <= (full_idx_count as u64) * 3;
    if saves_enough_indices && saves_enough_ratio {
        out
    } else {
        SectionDrawRanges::full(full_idx_count)
    }
}

/// Is one section's world-space AABB inside the view frustum?
fn section_visible(frustum: Frustum, origin: (i32, i32), section_idx: usize) -> bool {
    let (ox, oz) = origin;
    let y0 = (section_idx * SECTION_SIZE) as f32;
    let y1 = ((section_idx + 1) * SECTION_SIZE).min(CHUNK_SY) as f32;
    let min = glam::Vec3::new(ox as f32, y0, oz as f32);
    let max = glam::Vec3::new((ox + 16) as f32, y1, (oz + 16) as f32);
    frustum.aabb_visible(min, max)
}

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
            world_revision: world.visibility_revision(),
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
        world.insert_chunk_for_test(pos, chunk);
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
        world.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        world.invalidate_section_visibility(ChunkPos::new(0, 0));

        let visible = compute_visible_sections(&mut world, (8, 80, 8));

        assert!(visible.is_none());
    }

    #[test]
    fn section_draw_ranges_keep_single_visible_section() {
        let frustum = Frustum::permissive();
        let mut sections = [MeshIndexSection::default(); SECTION_COUNT];
        sections[2] = MeshIndexSection {
            first_index: 120,
            index_count: 60,
        };

        let ranges = section_draw_ranges(frustum, (0, 0), 480, &sections, 1u16 << 2);

        assert_eq!(ranges.iter().collect::<Vec<_>>(), vec![(120, 180)]);
        assert_eq!(ranges.submitted, 60);
    }

    #[test]
    fn section_draw_ranges_fall_back_when_fragmented_savings_are_small() {
        let frustum = Frustum::permissive();
        let mut sections = [MeshIndexSection::default(); SECTION_COUNT];
        sections[0] = MeshIndexSection {
            first_index: 0,
            index_count: 100,
        };
        sections[2] = MeshIndexSection {
            first_index: 200,
            index_count: 100,
        };

        let ranges = section_draw_ranges(frustum, (0, 0), 360, &sections, 0b0101);

        assert_eq!(ranges.iter().collect::<Vec<_>>(), vec![(0, 360)]);
        assert_eq!(ranges.submitted, 360);
    }

    #[test]
    fn section_draw_ranges_keep_fragmented_ranges_when_savings_are_large() {
        let frustum = Frustum::permissive();
        let mut sections = [MeshIndexSection::default(); SECTION_COUNT];
        sections[0] = MeshIndexSection {
            first_index: 0,
            index_count: 600,
        };
        sections[8] = MeshIndexSection {
            first_index: 8_000,
            index_count: 600,
        };

        let ranges = section_draw_ranges(
            frustum,
            (0, 0),
            12_000,
            &sections,
            (1u16 << 0) | (1u16 << 8),
        );

        assert_eq!(
            ranges.iter().collect::<Vec<_>>(),
            vec![(0, 600), (8_000, 8_600)]
        );
        assert_eq!(ranges.submitted, 1_200);
    }
}
