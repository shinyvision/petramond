use std::collections::VecDeque;
use std::sync::Arc;

use crate::chunk::{section_idx, SectionPos, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::mathh::IVec3;

use super::shape::LightCells;
use super::{nbhd_idx, NBHD, NBHD_VOLUME};

const DIRECTIONS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// Light value an emitter seeds at its own cell (x2 scale). One torch is level 14.
pub(super) const EMITTER_LIGHT: u8 = 28;

/// Reusable flood scratch: the 48³ working light cube plus the BFS queue. One per
/// light worker thread (see `queue::run_light_bake`) so streaming bakes don't
/// allocate ~110 KB per flood; the flood functions reset it on entry, and the
/// clipped centre result is allocated fresh since it outlives the bake.
pub(super) struct FloodScratch {
    light: Box<[u8]>,
    queue: VecDeque<(usize, usize, usize)>,
}

impl FloodScratch {
    pub(super) fn new() -> Self {
        Self {
            light: vec![0u8; NBHD_VOLUME].into_boxed_slice(),
            queue: VecDeque::new(),
        }
    }

    fn reset(&mut self) -> (&mut [u8], &mut VecDeque<(usize, usize, usize)>) {
        self.light.fill(0);
        self.queue.clear();
        (&mut self.light, &mut self.queue)
    }
}

/// Flood skylight across the 3x3x3 section neighbourhood, then clip to the centre.
pub(super) fn skylight(
    pos: SectionPos,
    cells: LightCells<'_>,
    surface: &[i32],
    scratch: &mut FloodScratch,
) -> Arc<[u8]> {
    let noy = pos.origin_world().1 - SECTION_SIZE as i32;

    let (light, queue) = scratch.reset();

    // Every above-surface cell reads as full sky (the flood relaxations and the
    // clipped output both read these) ...
    for y in 0..NBHD {
        let wy = noy + y as i32;
        for z in 0..NBHD {
            for x in 0..NBHD {
                if wy > surface[z * NBHD + x] {
                    light[nbhd_idx(x, y, z)] = SKY_FULL;
                }
            }
        }
    }

    // ... but only the terrain-envelope FRONTIER enters the BFS queue: sky cells
    // with at least one in-cube neighbour at-or-below that neighbour's column
    // surface. An interior sky cell's pop can never push (all its neighbours
    // already hold SKY_FULL), so skipping it is byte-identical — and a surface
    // bake used to enqueue every one of its ~50k open-sky cells just to pop them
    // for nothing. Per column the frontier is the band from the cell directly
    // above the surface up to the highest of the four horizontal neighbours'
    // surfaces (cells beside terrain), clamped to the cube.
    let cube_y_lo = noy;
    let cube_y_hi = noy + NBHD as i32 - 1;
    for z in 0..NBHD {
        for x in 0..NBHD {
            let s = surface[z * NBHD + x];
            if s >= cube_y_hi {
                continue;
            }
            let mut band_top = s + 1;
            if x > 0 {
                band_top = band_top.max(surface[z * NBHD + x - 1]);
            }
            if x + 1 < NBHD {
                band_top = band_top.max(surface[z * NBHD + x + 1]);
            }
            if z > 0 {
                band_top = band_top.max(surface[(z - 1) * NBHD + x]);
            }
            if z + 1 < NBHD {
                band_top = band_top.max(surface[(z + 1) * NBHD + x]);
            }
            let y_lo = if s < cube_y_lo {
                0
            } else {
                (s + 1 - noy) as usize
            };
            let y_hi = if band_top < cube_y_lo {
                continue;
            } else if band_top >= cube_y_hi {
                NBHD - 1
            } else {
                (band_top - noy) as usize
            };
            if y_lo > y_hi {
                continue;
            }
            for y in y_lo..=y_hi {
                queue.push_back((x, y, z));
            }
        }
    }

    propagate(cells, light, queue);
    clip_center(light)
}

/// Flood block light from every emitter in the neighbourhood, then clip to the centre.
pub(super) fn block_light(
    pos: SectionPos,
    cells: LightCells<'_>,
    emitters: &[IVec3],
    scratch: &mut FloodScratch,
) -> Arc<[u8]> {
    let (cox, coy, coz) = pos.origin_world();
    let (nox, noy, noz) = (
        cox - SECTION_SIZE as i32,
        coy - SECTION_SIZE as i32,
        coz - SECTION_SIZE as i32,
    );
    let n = NBHD as i32;

    let (light, queue) = scratch.reset();
    for e in emitters {
        let (x, y, z) = (e.x - nox, e.y - noy, e.z - noz);
        if !(0..n).contains(&x) || !(0..n).contains(&y) || !(0..n).contains(&z) {
            continue;
        }
        let (x, y, z) = (x as usize, y as usize, z as usize);
        let i = nbhd_idx(x, y, z);
        if light[i] < EMITTER_LIGHT {
            light[i] = EMITTER_LIGHT;
            queue.push_back((x, y, z));
        }
    }

    propagate(cells, light, queue);
    clip_center(light)
}

fn propagate(cells: LightCells<'_>, light: &mut [u8], queue: &mut VecDeque<(usize, usize, usize)>) {
    while let Some(from) = queue.pop_front() {
        let level = light[nbhd_idx(from.0, from.1, from.2)];
        if level <= 2 {
            continue;
        }
        let next = level - 2;
        for dir in DIRECTIONS {
            let Some(to) = step(from, dir) else {
                continue;
            };
            if !cells.can_cross(from, to, dir) {
                continue;
            }
            let ni = nbhd_idx(to.0, to.1, to.2);
            if light[ni] < next {
                light[ni] = next;
                queue.push_back(to);
            }
        }
    }
}

fn step(from: (usize, usize, usize), dir: (i32, i32, i32)) -> Option<(usize, usize, usize)> {
    let x = from.0.checked_add_signed(dir.0 as isize)?;
    let y = from.1.checked_add_signed(dir.1 as isize)?;
    let z = from.2.checked_add_signed(dir.2 as isize)?;
    (x < NBHD && y < NBHD && z < NBHD).then_some((x, y, z))
}

fn clip_center(light: &[u8]) -> Arc<[u8]> {
    let mut out = vec![0u8; SECTION_VOLUME];
    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                out[section_idx(lx, ly, lz)] =
                    light[nbhd_idx(lx + SECTION_SIZE, ly + SECTION_SIZE, lz + SECTION_SIZE)];
            }
        }
    }
    out.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::block::Block;
    use crate::block_state::{StairHalf, StairState};
    use crate::furnace::Facing;

    use super::super::shape::{ShapeStateSnapshot, SparseCellState};
    use super::super::NBHD_AREA;

    fn default_states() -> ShapeStateSnapshot {
        ShapeStateSnapshot::default()
    }

    fn cells<'a>(blocks: &'a [u8], states: &'a ShapeStateSnapshot) -> LightCells<'a> {
        LightCells::new(blocks, states)
    }

    fn full_seed_skylight(pos: SectionPos, cells: LightCells<'_>, surface: &[i32]) -> Arc<[u8]> {
        let noy = pos.origin_world().1 - SECTION_SIZE as i32;
        let mut light = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        let mut queue: VecDeque<(usize, usize, usize)> = VecDeque::new();
        for y in 0..NBHD {
            let wy = noy + y as i32;
            for z in 0..NBHD {
                for x in 0..NBHD {
                    if wy > surface[z * NBHD + x] {
                        light[nbhd_idx(x, y, z)] = SKY_FULL;
                        queue.push_back((x, y, z));
                    }
                }
            }
        }
        propagate(cells, &mut light, &mut queue);
        clip_center(&light)
    }

    fn stair_states(entries: &[(usize, Facing)]) -> ShapeStateSnapshot {
        let states = entries
            .iter()
            .map(|&(idx, facing)| SparseCellState::Stair {
                idx,
                state: StairState::new(facing, StairHalf::Bottom),
            })
            .collect::<Vec<_>>();
        ShapeStateSnapshot::from_sparse(&states)
    }

    #[test]
    fn frontier_seeding_matches_full_sky_seeding() {
        // The frontier-only seed set must reproduce the full-seed flood exactly:
        // an interior sky cell's pop can never push (its neighbours are all
        // SKY_FULL already), so the two fixpoints are identical. Randomized
        // rough terrain with cave holes exercises bands above/inside/below the
        // cube and diagonal-neighbour seams.
        let pos = SectionPos::new(3, 2, -5);
        let noy = pos.origin_world().1 - SECTION_SIZE as i32;
        let mut rng = 0x1234_5678_9abc_def0u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        for round in 0..4 {
            let mut blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
            let mut surface = vec![0i32; NBHD_AREA].into_boxed_slice();
            for z in 0..NBHD {
                for x in 0..NBHD {
                    let h = noy + (next() % 60) as i32 - 6;
                    surface[z * NBHD + x] = h;
                    for y in 0..NBHD {
                        let wy = noy + y as i32;
                        if wy <= h && next() % 8 != 0 {
                            blocks[nbhd_idx(x, y, z)] = Block::Stone.id();
                        }
                    }
                }
            }
            let states = default_states();

            let got = skylight(
                pos,
                cells(&blocks, &states),
                &surface,
                &mut FloodScratch::new(),
            );

            // Reference: the pre-optimization seeding — every above-surface cell.
            let want = full_seed_skylight(pos, cells(&blocks, &states), &surface);

            assert_eq!(&got[..], &want[..], "flood mismatch in round {round}");
        }
    }

    #[test]
    fn frontier_seeding_handles_covered_sentinel_columns() {
        let pos = SectionPos::new(0, 0, 0);
        let noy = pos.origin_world().1 - SECTION_SIZE as i32;
        let mut surface = vec![noy + 24; NBHD_AREA].into_boxed_slice();
        for z in 0..NBHD {
            for x in 0..SECTION_SIZE {
                surface[z * NBHD + x] = i32::MAX;
            }
        }
        let blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        let states = default_states();

        let got = skylight(
            pos,
            cells(&blocks, &states),
            &surface,
            &mut FloodScratch::new(),
        );

        let want = full_seed_skylight(pos, cells(&blocks, &states), &surface);

        assert_eq!(&got[..], &want[..]);
    }

    #[test]
    fn block_light_floods_across_a_section_seam() {
        let pos = SectionPos::new(0, 0, 0);
        let emitter = IVec3::new(-1, 8, 8);
        let blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        let states = default_states();

        let cube = block_light(
            pos,
            cells(&blocks, &states),
            &[emitter],
            &mut FloodScratch::new(),
        );

        assert_eq!(cube[section_idx(0, 8, 8)], EMITTER_LIGHT - 2);
        assert!(cube[section_idx(4, 8, 8)] < cube[section_idx(0, 8, 8)]);
        assert_eq!(cube[section_idx(15, 8, 8)], 0);
    }

    #[test]
    fn opaque_seam_blocks_the_cross_section_flood() {
        let pos = SectionPos::new(0, 0, 0);
        let emitter = IVec3::new(-1, 8, 8);
        let mut blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        for ly in 0..SECTION_SIZE {
            for lz in 0..SECTION_SIZE {
                blocks[nbhd_idx(SECTION_SIZE, ly + SECTION_SIZE, lz + SECTION_SIZE)] =
                    Block::Stone.id();
            }
        }
        let states = default_states();

        let cube = block_light(
            pos,
            cells(&blocks, &states),
            &[emitter],
            &mut FloodScratch::new(),
        );

        assert_eq!(cube[section_idx(0, 8, 8)], 0);
        assert_eq!(cube[section_idx(1, 8, 8)], 0);
    }

    #[test]
    fn block_light_enters_a_stair_only_through_an_open_side() {
        let pos = SectionPos::new(0, 0, 0);
        let emitter = IVec3::new(-1, 8, 8);
        let mut blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        let (x, y, z) = (SECTION_SIZE, SECTION_SIZE + 8, SECTION_SIZE + 8);
        let stair_i = nbhd_idx(x, y, z);
        blocks[stair_i] = Block::OakStairs.id();
        blocks[nbhd_idx(x + 1, y, z)] = Block::Stone.id();
        blocks[nbhd_idx(x, y + 1, z)] = Block::Stone.id();
        blocks[nbhd_idx(x, y - 1, z)] = Block::Stone.id();
        blocks[nbhd_idx(x, y, z - 1)] = Block::Stone.id();
        blocks[nbhd_idx(x, y, z + 1)] = Block::Stone.id();

        let closed_back = stair_states(&[(stair_i, Facing::East)]);
        let closed = block_light(
            pos,
            cells(&blocks, &closed_back),
            &[emitter],
            &mut FloodScratch::new(),
        );
        assert_eq!(closed[section_idx(0, 8, 8)], 0);

        let open_side = stair_states(&[(stair_i, Facing::West)]);
        let open = block_light(
            pos,
            cells(&blocks, &open_side),
            &[emitter],
            &mut FloodScratch::new(),
        );
        assert!(open[section_idx(0, 8, 8)] > 0);
    }

    #[test]
    fn skylight_seeps_under_a_single_covering_block() {
        let pos = SectionPos::new(0, 0, 0);
        let blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        let mut surface = vec![-100i32; NBHD_AREA].into_boxed_slice();
        let (gx, gz) = (8 + SECTION_SIZE, 8 + SECTION_SIZE);
        surface[gz * NBHD + gx] = 40;
        let states = default_states();

        let cube = skylight(
            pos,
            cells(&blocks, &states),
            &surface,
            &mut FloodScratch::new(),
        );

        assert!(cube[section_idx(8, 8, 8)] > 0);
        assert_eq!(cube[section_idx(7, 8, 8)], SKY_FULL);
    }

    #[test]
    fn skylight_enters_a_stair_top_gap_but_not_its_solid_bottom() {
        let pos = SectionPos::new(0, 0, 0);
        let mut blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        let (x, y, z) = (SECTION_SIZE + 8, SECTION_SIZE + 8, SECTION_SIZE + 8);
        let stair_i = nbhd_idx(x, y, z);
        blocks[stair_i] = Block::OakStairs.id();
        blocks[nbhd_idx(x - 1, y - 1, z)] = Block::Stone.id();
        blocks[nbhd_idx(x + 1, y - 1, z)] = Block::Stone.id();
        blocks[nbhd_idx(x, y - 1, z - 1)] = Block::Stone.id();
        blocks[nbhd_idx(x, y - 1, z + 1)] = Block::Stone.id();
        blocks[nbhd_idx(x, y - 2, z)] = Block::Stone.id();

        let states = stair_states(&[(stair_i, Facing::East)]);
        let mut surface = vec![40i32; NBHD_AREA].into_boxed_slice();
        surface[z * NBHD + x] = 8;

        let cube = skylight(
            pos,
            cells(&blocks, &states),
            &surface,
            &mut FloodScratch::new(),
        );

        assert!(cube[section_idx(8, 8, 8)] > 0);
        assert_eq!(cube[section_idx(8, 7, 8)], 0);
    }

    #[test]
    fn stair_walls_with_solid_backs_inside_and_stair_roof_keep_interior_dark() {
        let pos = SectionPos::new(0, 0, 0);
        let mut blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        let mut stairs = Vec::new();
        let mut surface = vec![-100i32; NBHD_AREA].into_boxed_slice();
        let (cx, cy, cz) = (SECTION_SIZE + 8, SECTION_SIZE + 8, SECTION_SIZE + 8);

        let place_stair = |blocks: &mut [u8],
                           stairs: &mut Vec<(usize, Facing)>,
                           surface: &mut [i32],
                           x: usize,
                           y: usize,
                           z: usize,
                           facing: Facing| {
            let i = nbhd_idx(x, y, z);
            blocks[i] = Block::OakStairs.id();
            stairs.push((i, facing));
            surface[z * NBHD + x] = (y as i32) - SECTION_SIZE as i32;
        };

        place_stair(
            &mut blocks,
            &mut stairs,
            &mut surface,
            cx - 1,
            cy,
            cz,
            Facing::West,
        );
        place_stair(
            &mut blocks,
            &mut stairs,
            &mut surface,
            cx + 1,
            cy,
            cz,
            Facing::East,
        );
        place_stair(
            &mut blocks,
            &mut stairs,
            &mut surface,
            cx,
            cy,
            cz - 1,
            Facing::North,
        );
        place_stair(
            &mut blocks,
            &mut stairs,
            &mut surface,
            cx,
            cy,
            cz + 1,
            Facing::South,
        );
        place_stair(
            &mut blocks,
            &mut stairs,
            &mut surface,
            cx,
            cy + 1,
            cz,
            Facing::North,
        );
        blocks[nbhd_idx(cx, cy - 1, cz)] = Block::Stone.id();
        surface[cz * NBHD + cx] = 9;

        let states = stair_states(&stairs);
        let cube = skylight(
            pos,
            cells(&blocks, &states),
            &surface,
            &mut FloodScratch::new(),
        );

        assert_eq!(cube[section_idx(8, 8, 8)], 0);
    }

    #[test]
    fn skylight_stays_dark_under_full_cover() {
        let pos = SectionPos::new(0, 0, 0);
        let blocks = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        let surface = vec![40i32; NBHD_AREA].into_boxed_slice();
        let states = default_states();

        let cube = skylight(
            pos,
            cells(&blocks, &states),
            &surface,
            &mut FloodScratch::new(),
        );

        assert!(cube.iter().all(|&l| l == 0));
    }
}
