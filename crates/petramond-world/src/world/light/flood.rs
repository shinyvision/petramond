use std::collections::VecDeque;
use std::sync::Arc;

#[cfg(test)]
use crate::chunk::section_idx;
use crate::chunk::{SectionPos, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::light::{LightRgb, DECAY};
use crate::mathh::IVec3;

use super::shape::LightCells;
use super::{NBHD, NBHD_VOLUME};

/// The standard torch-class emission level (x2 scale, level 14). Seeds come
/// from each emitter row's own `emission` value; this constant remains as the
/// reference level the engine's emitters (torch, lit furnace) all use — tests
/// build their emitters with it.
#[cfg(any(test, feature = "test-support"))]
pub const EMITTER_LIGHT: u8 = 28;

/// Reusable flood scratch: the working light cube plus the BFS queue. One per
/// light worker thread (see `queue::run_light_bake`) so streaming bakes don't
/// allocate ~110 KB per flood; the flood functions reset it on entry, and the
/// clipped per-section results are allocated fresh since they outlive the bake.
/// The cube grows on demand: per-section bakes flood 48³, batch bakes 64³.
///
/// Sky and block keep SEPARATE cubes. The sky flood runs on nearly every
/// section and must not pay the colour cell's doubled clear; the RGB cube is
/// touched only by a bake that actually found an emitter.
pub struct FloodScratch {
    sky: Vec<u8>,
    block: Vec<LightRgb>,
    queue: VecDeque<Cursor>,
}

impl Default for FloodScratch {
    fn default() -> Self {
        Self::new()
    }
}

impl FloodScratch {
    pub fn new() -> Self {
        Self {
            sky: vec![0u8; NBHD_VOLUME],
            block: Vec::new(),
            queue: VecDeque::new(),
        }
    }

    /// No clear: [`skylight_cube`]'s pre-fill writes EVERY cell of the cube,
    /// so a memset here would only be overwritten.
    fn reset_sky(&mut self, volume: usize) -> (&mut [u8], &mut VecDeque<Cursor>) {
        if self.sky.len() < volume {
            self.sky.resize(volume, 0);
        }
        self.queue.clear();
        (&mut self.sky[..volume], &mut self.queue)
    }

    fn reset_block(&mut self, volume: usize) -> (&mut [LightRgb], &mut VecDeque<Cursor>) {
        if self.block.len() < volume {
            self.block.resize(volume, LightRgb::ZERO);
        }
        let light = &mut self.block[..volume];
        light.fill(LightRgb::ZERO);
        self.queue.clear();
        (light, &mut self.queue)
    }
}

#[inline]
fn cube_idx(dim: usize, x: usize, y: usize, z: usize) -> usize {
    (y * dim + z) * dim + x
}

/// Flood skylight across the 3x3x3 section neighbourhood, then clip to the centre.
pub fn skylight(
    pos: SectionPos,
    cells: LightCells<'_>,
    surface: &[i32],
    scratch: &mut FloodScratch,
) -> Arc<[u8]> {
    let noy = pos.origin_world().1 - SECTION_SIZE as i32;
    let keep = Keep::new(SECTION_SIZE, SECTION_SIZE * 2);
    let light = skylight_cube(noy, NBHD, cells, keep, surface, scratch);
    clip_cube(light, NBHD, (SECTION_SIZE, SECTION_SIZE, SECTION_SIZE))
}

/// Flood skylight over a `dim`³ cube whose world origin Y is `cube_oy`, leaving the
/// fixpoint in the returned scratch slice for the caller to clip per section.
/// `surface` is the `dim`² sky-cover map, row-major `[z][x]`.
pub fn skylight_cube<'s>(
    cube_oy: i32,
    dim: usize,
    cells: LightCells<'_>,
    keep: Keep,
    surface: &[i32],
    scratch: &'s mut FloodScratch,
) -> &'s [u8] {
    debug_assert_eq!(surface.len(), dim * dim);
    let (light, queue) = scratch.reset_sky(dim * dim * dim);

    // Every above-surface cell reads as full sky (the flood relaxations and the
    // clipped output both read these), everything else starts dark. One
    // horizontal plane of the cube is exactly one pass over `surface` — the
    // cube's Y-major layout makes plane `y` and the cover map the same shape —
    // so this is a branchless select the compiler vectorizes, and it is what
    // lets the scratch skip its clear ...
    for y in 0..dim {
        let wy = cube_oy + y as i32;
        let plane = &mut light[y * dim * dim..(y + 1) * dim * dim];
        for (cell, &cover) in plane.iter_mut().zip(surface.iter()) {
            *cell = if wy > cover { SKY_FULL } else { 0 };
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
    let cube_y_lo = cube_oy;
    let cube_y_hi = cube_oy + dim as i32 - 1;
    for z in 0..dim {
        for x in 0..dim {
            let Some(y_floor) = keep.seed_y_floor(x as u32, z as u32) else {
                continue;
            };
            let s = surface[z * dim + x];
            if s >= cube_y_hi {
                continue;
            }
            let mut band_top = s + 1;
            if x > 0 {
                band_top = band_top.max(surface[z * dim + x - 1]);
            }
            if x + 1 < dim {
                band_top = band_top.max(surface[z * dim + x + 1]);
            }
            if z > 0 {
                band_top = band_top.max(surface[(z - 1) * dim + x]);
            }
            if z + 1 < dim {
                band_top = band_top.max(surface[(z + 1) * dim + x]);
            }
            let y_lo = if s < cube_y_lo {
                y_floor as usize
            } else {
                ((s + 1 - cube_oy) as usize).max(y_floor as usize)
            };
            let y_hi = if band_top < cube_y_lo {
                continue;
            } else if band_top >= cube_y_hi {
                dim - 1
            } else {
                (band_top - cube_oy) as usize
            };
            if y_lo > y_hi {
                continue;
            }
            for y in y_lo..=y_hi {
                queue.push_back(cursor(x, y, z));
            }
        }
    }

    propagate_sky(dim, cells, keep, light, queue);
    light
}

/// Flood block light from every emitter in the neighbourhood, then clip to the centre.
pub fn block_light(
    pos: SectionPos,
    cells: LightCells<'_>,
    emitters: &[(IVec3, LightRgb)],
    scratch: &mut FloodScratch,
) -> Arc<[LightRgb]> {
    let (cox, coy, coz) = pos.origin_world();
    let origin = (
        cox - SECTION_SIZE as i32,
        coy - SECTION_SIZE as i32,
        coz - SECTION_SIZE as i32,
    );
    let keep = Keep::new(SECTION_SIZE, SECTION_SIZE * 2);
    let light = block_light_cube(origin, NBHD, cells, keep, emitters, scratch);
    clip_cube(light, NBHD, (SECTION_SIZE, SECTION_SIZE, SECTION_SIZE))
}

/// Flood block light over a `dim`³ cube at world `origin`, leaving the fixpoint in
/// the returned scratch slice for the caller to clip per section.
pub fn block_light_cube<'s>(
    origin: (i32, i32, i32),
    dim: usize,
    cells: LightCells<'_>,
    keep: Keep,
    emitters: &[(IVec3, LightRgb)],
    scratch: &'s mut FloodScratch,
) -> &'s [LightRgb] {
    let n = dim as i32;
    let area = (dim * dim) as isize;
    let strides: [isize; 6] = [1, -1, area, -area, dim as isize, -(dim as isize)];
    let (light, queue) = scratch.reset_block(dim * dim * dim);
    for &(e, emission) in emitters {
        let (x, y, z) = (e.x - origin.0, e.y - origin.1, e.z - origin.2);
        if !(0..n).contains(&x) || !(0..n).contains(&y) || !(0..n).contains(&z) {
            continue;
        }
        let (x, y, z) = (x as usize, y as usize, z as usize);
        let i = cube_idx(dim, x, y, z);
        let merged = light[i].max_with(emission);
        if merged != light[i] {
            light[i] = merged;
            queue.push_back(cursor(x, y, z));
        }
        // The emitter's OWN emission also steps straight out, gated only on
        // whether the neighbour accepts it: a cell's matter cannot imprison
        // the light that same matter makes, so a full opaque lamp radiates.
        // Only the row's own value escapes this way — light that merely
        // ARRIVED here is not the cell's to re-emit, which also keeps the
        // result independent of the order emitters are seeded in.
        let out = emission.decayed();
        if out.is_dark() {
            continue;
        }
        let from = cursor(x, y, z);
        for k in 0..6 {
            if !in_cube(k, x as u32, y as u32, z as u32, dim as u32) {
                continue;
            }
            let ni = i.wrapping_add_signed(strides[k]);
            if face_mask(cells.word(ni), k) == 0 {
                continue;
            }
            let merged = light[ni].max_with(out);
            if merged != light[ni] {
                light[ni] = merged;
                queue.push_back(from.wrapping_add(CURSOR_STEP[k]));
            }
        }
    }

    propagate_block(dim, cells, keep, light, queue);
    light
}

/// Packed BFS cursor: `x | y<<10 | z<<20`. The queue holds tens of thousands of
/// entries per flood, so it rides 4 bytes instead of a 24-byte coordinate
/// triple, and a neighbour step is one wrapping add of a constant.
type Cursor = u32;

#[inline]
fn cursor(x: usize, y: usize, z: usize) -> Cursor {
    (x as u32) | ((y as u32) << 10) | ((z as u32) << 20)
}

/// Neighbour deltas in [`FACE_NEIGHBORS`] order, as packed-cursor increments.
const CURSOR_STEP: [u32; 6] = [
    1,
    1u32.wrapping_neg(),
    1 << 10,
    (1u32 << 10).wrapping_neg(),
    1 << 20,
    (1u32 << 20).wrapping_neg(),
];

/// The `+Y` entry of [`FACE_NEIGHBORS`]; its opposite (index 3) is the
/// straight-down step direct skylight rides losslessly.
const DOWN: usize = 3;

/// The sub-box of the flood cube whose values the caller will actually KEEP —
/// the centre section for a per-section bake, the whole member group for a
/// batch. Half-open per axis.
#[derive(Copy, Clone)]
pub struct Keep {
    lo: [u32; 3],
    hi: [u32; 3],
}

impl Keep {
    pub fn new(lo: usize, hi: usize) -> Self {
        Self {
            lo: [lo as u32; 3],
            hi: [hi as u32; 3],
        }
    }

    /// Steps along one axis that a path from `v` into the box must take.
    #[inline]
    fn axis(v: u32, lo: u32, hi: u32) -> (u32, u32) {
        if v < lo {
            (lo - v, 0)
        } else if v >= hi {
            (0, v - hi + 1)
        } else {
            (0, 0)
        }
    }

    /// The lowest cube Y from which a FULL-STRENGTH seed in column `(x, z)`
    /// can still reach the kept box, or `None` when the column is out of reach
    /// horizontally. The horizontal half of [`Self::lossy_steps`] is constant
    /// down a column, so the seed loop clamps a RANGE instead of testing every
    /// cell — and skips whole columns outright.
    #[inline]
    fn seed_y_floor(self, x: u32, z: u32) -> Option<u32> {
        let (dx_lo, dx_hi) = Self::axis(x, self.lo[0], self.hi[0]);
        let (dz_lo, dz_hi) = Self::axis(z, self.lo[2], self.hi[2]);
        let flat = dx_lo + dx_hi + dz_lo + dz_hi;
        let reach = u32::from(SKY_FULL / DECAY);
        (flat < reach).then(|| self.lo[1].saturating_sub(reach - flat - 1))
    }

    /// The FEWEST attenuating steps any path from `(x, y, z)` into the kept box
    /// can take, given that a cell already at full strength descends without
    /// loss but pays for every horizontal or upward step.
    ///
    /// This is the whole reason a per-section bake is affordable: the flood
    /// cube is 27× the volume it keeps, and light that cannot reach the kept
    /// box is work thrown away. Below full strength every step costs `DECAY`,
    /// so the L1 distance bounds the arrival; at full strength only the
    /// non-descending part of the journey is charged (and one lossy step drops
    /// the value below full for good, so it can never re-enter the free case).
    #[inline]
    fn lossy_steps(self, x: u32, y: u32, z: u32, at_full: bool) -> u32 {
        let (dx_lo, dx_hi) = Self::axis(x, self.lo[0], self.hi[0]);
        let (dz_lo, dz_hi) = Self::axis(z, self.lo[2], self.hi[2]);
        let (up, down) = Self::axis(y, self.lo[1], self.hi[1]);
        let flat = dx_lo + dx_hi + dz_lo + dz_hi + up;
        if at_full {
            flat
        } else {
            flat + down
        }
    }
}

/// Per-edge geometry, shared by both floods: whether the step stays in the
/// cube, and the two aperture quadrant masks that must overlap for light to
/// cross. `k` indexes [`FACE_NEIGHBORS`]; the packed aperture word orders its
/// faces `-x,+x,-y,+y,-z,+z`, so the SOURCE cell's outgoing face is `k ^ 1`
/// and the DESTINATION cell's incoming face is `k`.
#[inline]
fn in_cube(k: usize, x: u32, y: u32, z: u32, dim: u32) -> bool {
    match k {
        0 => x + 1 < dim,
        1 => x > 0,
        2 => y + 1 < dim,
        3 => y > 0,
        4 => z + 1 < dim,
        _ => z > 0,
    }
}

#[inline]
fn face_mask(word: u32, face: usize) -> u32 {
    (word >> (face * 4)) & 0xF
}

/// Skylight relaxation: scalar cells, plus the one exception colour never
/// touches — an undecayed straight-down step through a cell that transmits
/// DIRECT skylight.
fn propagate_sky(
    dim: usize,
    cells: LightCells<'_>,
    keep: Keep,
    light: &mut [u8],
    queue: &mut VecDeque<Cursor>,
) {
    let d = dim as u32;
    let area = (dim * dim) as isize;
    let strides: [isize; 6] = [1, -1, area, -area, dim as isize, -(dim as isize)];
    while let Some(from) = queue.pop_front() {
        let (x, y, z) = (from & 0x3FF, (from >> 10) & 0x3FF, from >> 20);
        let fi = (y as usize * dim + z as usize) * dim + x as usize;
        let level = light[fi];
        if level <= DECAY {
            continue;
        }
        // Nothing this cell can send survives the trip into the kept box.
        if u32::from(DECAY) * keep.lossy_steps(x, y, z, level == SKY_FULL) >= u32::from(level) {
            continue;
        }
        let fw = cells.word(fi);
        if fw & crate::block::LIGHT_APERTURES_OPEN == 0 {
            continue;
        }
        for k in 0..6 {
            let out = face_mask(fw, k ^ 1);
            if out == 0 || !in_cube(k, x, y, z, d) {
                continue;
            }
            let ni = fi.wrapping_add_signed(strides[k]);
            let tw = cells.word(ni);
            if face_mask(tw, k) & out == 0 {
                continue;
            }
            let next = if level == SKY_FULL
                && k == DOWN
                && tw & crate::block::LIGHT_CELL_DIRECT_SKY != 0
            {
                SKY_FULL
            } else {
                level - DECAY
            };
            if light[ni] < next {
                light[ni] = next;
                queue.push_back(from.wrapping_add(CURSOR_STEP[k]));
            }
        }
    }
}

/// ONE FUSED relaxation over all three colour channels, never three floods.
///
/// Per edge the GEOMETRY dominates: the bounds step plus the two aperture
/// masks. That work is entirely colour-blind — colour never decides whether
/// light crosses, only what arrives — so relaxing a vector pays it ONCE and
/// adds two more compares, where three scalar floods would pay it three times.
/// The decay is the same for all six edges, so it is computed once per POP.
fn propagate_block(
    dim: usize,
    cells: LightCells<'_>,
    keep: Keep,
    light: &mut [LightRgb],
    queue: &mut VecDeque<Cursor>,
) {
    let d = dim as u32;
    let area = (dim * dim) as isize;
    let strides: [isize; 6] = [1, -1, area, -area, dim as isize, -(dim as isize)];
    while let Some(from) = queue.pop_front() {
        let (x, y, z) = (from & 0x3FF, (from >> 10) & 0x3FF, from >> 20);
        let fi = (y as usize * dim + z as usize) * dim + x as usize;
        let level = light[fi];
        let next = level.decayed();
        if next.is_dark() {
            continue;
        }
        // Block light has no lossless step, so the L1 distance to the kept box
        // bounds what the brightest channel could still deliver there.
        if u32::from(DECAY) * keep.lossy_steps(x, y, z, false) >= u32::from(level.luminance()) {
            continue;
        }
        let fw = cells.word(fi);
        if fw & crate::block::LIGHT_APERTURES_OPEN == 0 {
            continue;
        }
        for k in 0..6 {
            let out = face_mask(fw, k ^ 1);
            if out == 0 || !in_cube(k, x, y, z, d) {
                continue;
            }
            let ni = fi.wrapping_add_signed(strides[k]);
            if face_mask(cells.word(ni), k) & out == 0 {
                continue;
            }
            let merged = light[ni].max_with(next);
            if merged != light[ni] {
                light[ni] = merged;
                queue.push_back(from.wrapping_add(CURSOR_STEP[k]));
            }
        }
    }
}

/// Copy one 16³ section out of a flooded `dim`³ cube; `off` is the section's cell
/// offset inside the cube.
pub fn clip_cube<T: Copy>(light: &[T], dim: usize, off: (usize, usize, usize)) -> Arc<[T]> {
    // Row-appended rather than zero-filled and overwritten: the clip writes
    // every cell, so the default init would be pure waste.
    let mut out = Vec::with_capacity(SECTION_VOLUME);
    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            let src = cube_idx(dim, off.0, ly + off.1, lz + off.2);
            out.extend_from_slice(&light[src..src + SECTION_SIZE]);
        }
    }
    debug_assert_eq!(out.len(), SECTION_VOLUME);
    out.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::block::Block;
    use crate::block_state::{StairHalf, StairState};
    use crate::facing::Facing;

    use super::super::shape::{ShapeStateSnapshot, SparseCellState};
    use super::super::{nbhd_idx, NBHD_AREA};

    fn default_states() -> ShapeStateSnapshot {
        ShapeStateSnapshot::default()
    }

    fn cells<'a>(blocks: &'a [u16], states: &'a ShapeStateSnapshot) -> LightCells<'a> {
        LightCells::new(blocks, states, NBHD)
    }

    /// A colourless emitter at the test's reference level.
    fn white(level: u8) -> LightRgb {
        LightRgb::grey(level)
    }

    fn full_seed_skylight(pos: SectionPos, cells: LightCells<'_>, surface: &[i32]) -> Arc<[u8]> {
        let noy = pos.origin_world().1 - SECTION_SIZE as i32;
        let mut light = vec![0u8; NBHD_VOLUME].into_boxed_slice();
        let mut queue: VecDeque<Cursor> = VecDeque::new();
        for y in 0..NBHD {
            let wy = noy + y as i32;
            for z in 0..NBHD {
                for x in 0..NBHD {
                    if wy > surface[z * NBHD + x] {
                        light[nbhd_idx(x, y, z)] = SKY_FULL;
                        queue.push_back(cursor(x, y, z));
                    }
                }
            }
        }
        let keep = Keep::new(SECTION_SIZE, SECTION_SIZE * 2);
        propagate_sky(NBHD, cells, keep, &mut light, &mut queue);
        clip_cube(&light, NBHD, (SECTION_SIZE, SECTION_SIZE, SECTION_SIZE))
    }

    /// One stair cell, as the shape seam sees it — the fixture's stand-in for
    /// a section, so the apertures come from the real family facet.
    struct OneStair(crate::block::ShapeState);

    impl crate::block::ShapeNeighborhood for OneStair {
        fn block(&self, _pos: crate::mathh::IVec3) -> Block {
            Block::OakStairs
        }
        fn shape_state(&self, _pos: crate::mathh::IVec3) -> crate::block::ShapeState {
            self.0
        }
    }

    fn stair_states(entries: &[(usize, Facing)]) -> ShapeStateSnapshot {
        // The stair family's aperture answer, exactly as the gather asks for it.
        let k = Block::OakStairs.shape_kind().def();
        let states = entries
            .iter()
            .map(|&(idx, facing)| {
                let nb = OneStair(crate::block::CellCodec::to_cell(&StairState::new(
                    facing,
                    StairHalf::Bottom,
                )));
                SparseCellState {
                    idx,
                    masks: k.sim.light_apertures(
                        &k.params,
                        &nb,
                        crate::mathh::IVec3::ZERO,
                        Block::OakStairs,
                    ),
                }
            })
            .collect::<Vec<_>>();
        ShapeStateSnapshot::from_sparse(&states, NBHD_VOLUME)
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
            let mut blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
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
        let blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
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
        let blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
        let states = default_states();

        let cube = block_light(
            pos,
            cells(&blocks, &states),
            &[(emitter, white(EMITTER_LIGHT))],
            &mut FloodScratch::new(),
        );

        assert_eq!(cube[section_idx(0, 8, 8)], white(EMITTER_LIGHT - 2));
        assert!(cube[section_idx(4, 8, 8)].luminance() < cube[section_idx(0, 8, 8)].luminance());
        assert!(cube[section_idx(15, 8, 8)].is_dark());
    }

    #[test]
    fn opaque_seam_blocks_the_cross_section_flood() {
        let pos = SectionPos::new(0, 0, 0);
        let emitter = IVec3::new(-1, 8, 8);
        let mut blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
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
            &[(emitter, white(EMITTER_LIGHT))],
            &mut FloodScratch::new(),
        );

        assert!(cube[section_idx(0, 8, 8)].is_dark());
        assert!(cube[section_idx(1, 8, 8)].is_dark());
    }

    /// A lamp's own matter must not imprison the light that same matter makes,
    /// so an emitter in an OPAQUE cell still lights what is around it. Transit
    /// is untouched: nothing reaches the far pocket except what the lamp
    /// itself radiates, which is what keeps "full solid cube" and
    /// "see-through" from collapsing into one property.
    #[test]
    fn an_opaque_emitter_lights_out_without_becoming_a_conduit() {
        let pos = SectionPos::new(0, 0, 0);
        let mut blocks = vec![Block::Stone.id(); NBHD_VOLUME].into_boxed_slice();
        let (y, z) = (SECTION_SIZE + 8, SECTION_SIZE + 8);
        // One opaque lamp cell at x=8 walling a near pocket (x=7) off from a
        // far one (x=9,10); everything else is solid rock.
        for x in [7usize, 9, 10] {
            blocks[nbhd_idx(SECTION_SIZE + x, y, z)] = Block::Air.id();
        }
        let states = default_states();

        let lamp = LightRgb::new(0, EMITTER_LIGHT, 0);
        let neighbour = LightRgb::new(EMITTER_LIGHT, 0, 0);
        let cube = block_light(
            pos,
            cells(&blocks, &states),
            &[
                (IVec3::new(8, 8, 8), lamp),
                (IVec3::new(7, 8, 8), neighbour),
            ],
            &mut FloodScratch::new(),
        );

        assert_eq!(
            cube[section_idx(9, 8, 8)],
            LightRgb::new(0, EMITTER_LIGHT - 2, 0)
        );
        assert_eq!(
            cube[section_idx(10, 8, 8)],
            LightRgb::new(0, EMITTER_LIGHT - 4, 0)
        );
        // Nothing enters the lamp cell, so no red crosses it to the far side.
        assert_eq!(cube[section_idx(8, 8, 8)], lamp);
    }

    #[test]
    fn block_light_enters_a_stair_only_through_an_open_side() {
        let pos = SectionPos::new(0, 0, 0);
        let emitter = IVec3::new(-1, 8, 8);
        let mut blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
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
            &[(emitter, white(EMITTER_LIGHT))],
            &mut FloodScratch::new(),
        );
        assert!(closed[section_idx(0, 8, 8)].is_dark());

        let open_side = stair_states(&[(stair_i, Facing::West)]);
        let open = block_light(
            pos,
            cells(&blocks, &open_side),
            &[(emitter, white(EMITTER_LIGHT))],
            &mut FloodScratch::new(),
        );
        assert!(!open[section_idx(0, 8, 8)].is_dark());
    }

    /// The fused vector relaxation must be EXACTLY three independent scalar
    /// floods, channel for channel — the whole justification for paying the
    /// geometry once instead of three times. Cluttered geometry so the two
    /// colours reach cells by different routes and the mixing is not a trivial
    /// straight line.
    #[test]
    fn the_fused_flood_equals_three_independent_scalar_floods() {
        let pos = SectionPos::new(0, 0, 0);
        let mut rng = 0x0bad_c0de_1234_5678u64;
        let mut next = move || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let mut blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
        for b in blocks.iter_mut() {
            if next() % 5 == 0 {
                *b = Block::Stone.id();
            }
        }
        let states = default_states();

        let purple = LightRgb::new(24, 6, 30);
        let blue = LightRgb::new(4, 18, 28);
        let emitters = [
            (IVec3::new(2, 8, 8), purple),
            (IVec3::new(13, 8, 9), blue),
            (IVec3::new(-4, 3, 12), purple),
        ];
        for &(e, _) in &emitters {
            blocks[nbhd_idx(
                (e.x + SECTION_SIZE as i32) as usize,
                (e.y + SECTION_SIZE as i32) as usize,
                (e.z + SECTION_SIZE as i32) as usize,
            )] = 0;
        }

        let fused = block_light(
            pos,
            cells(&blocks, &states),
            &emitters,
            &mut FloodScratch::new(),
        );

        for ch in 0..3 {
            let scalar: Vec<(IVec3, LightRgb)> = emitters
                .iter()
                .map(|&(e, c)| (e, LightRgb::grey(c.channels()[ch])))
                .collect();
            let want = block_light(
                pos,
                cells(&blocks, &states),
                &scalar,
                &mut FloodScratch::new(),
            );
            for i in 0..SECTION_VOLUME {
                assert_eq!(
                    fused[i].channels()[ch],
                    want[i].luminance(),
                    "channel {ch} diverged at cell {i}"
                );
            }
        }

        // ... and the two hues really do MIX somewhere in between. A single
        // saturated emitter already paints cells whose channels merely DIFFER,
        // so the fixture is only honest if some cell takes different channels
        // from different lamps: brighter in green than the purple lamp alone
        // could reach it, AND brighter in blue than the blue lamp alone could.
        let only_purple = block_light(
            pos,
            cells(&blocks, &states),
            &[(emitters[0].0, purple), (emitters[2].0, purple)],
            &mut FloodScratch::new(),
        );
        let only_blue = block_light(
            pos,
            cells(&blocks, &states),
            &[(emitters[1].0, blue)],
            &mut FloodScratch::new(),
        );
        assert!(
            (0..SECTION_VOLUME)
                .any(|i| { fused[i].g() > only_purple[i].g() && fused[i].r() > only_blue[i].r() }),
            "the fixture must have a cell whose channels come from BOTH lamps"
        );
    }

    /// THE feature: two differently-coloured lamps whose ranges overlap compose
    /// by per-channel MAXIMUM, so the overlap reads as the mixed colour — never
    /// "the nearer lamp wins". A pure red and a pure blue lamp facing each other
    /// down a corridor must leave magenta between them, with red still falling
    /// off toward the blue lamp and blue still falling off toward the red one.
    #[test]
    fn two_lamps_of_different_colour_mix_where_they_overlap() {
        let pos = SectionPos::new(0, 0, 0);
        let blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
        let states = default_states();

        let red = LightRgb::new(28, 0, 0);
        let blue = LightRgb::new(0, 0, 28);
        // Both inside the centre section, 12 cells apart along X at (y=8, z=8):
        // each reaches 14 cells, so the whole span between them is doubly lit.
        let (rx, bx) = (2i32, 14i32);
        let at = |x: usize| section_idx(x, 8, 8);

        let mixed = block_light(
            pos,
            cells(&blocks, &states),
            &[(IVec3::new(rx, 8, 8), red), (IVec3::new(bx, 8, 8), blue)],
            &mut FloodScratch::new(),
        );
        let red_only = block_light(
            pos,
            cells(&blocks, &states),
            &[(IVec3::new(rx, 8, 8), red)],
            &mut FloodScratch::new(),
        );
        let blue_only = block_light(
            pos,
            cells(&blocks, &states),
            &[(IVec3::new(bx, 8, 8), blue)],
            &mut FloodScratch::new(),
        );

        for x in 0..SECTION_SIZE {
            let (m, r, b) = (mixed[at(x)], red_only[at(x)], blue_only[at(x)]);
            // Per-channel maxima, exactly — the composition rule itself.
            assert_eq!(
                m.channels(),
                [r.r().max(b.r()), r.g().max(b.g()), r.b().max(b.b())],
                "cell x={x} is not the per-channel max of the two lamps"
            );
            // Neither lamp's channel is dragged down by the other's presence,
            // and green is never invented out of thin air.
            assert_eq!(m.g(), 0, "cell x={x} grew a green channel from nowhere");
        }

        // The span BETWEEN the lamps is genuinely magenta: both channels lit,
        // and each still gradient-falls off toward the far lamp (a "nearest
        // emitter wins" scheme would give one channel a hard cliff at the
        // midpoint and zero on the far side).
        for x in (rx as usize)..=(bx as usize) {
            let c = mixed[at(x)];
            assert!(c.r() > 0 && c.b() > 0, "cell x={x} is not magenta: {c:?}");
        }
        assert!(mixed[at(rx as usize)].r() > mixed[at(bx as usize)].r());
        assert!(mixed[at(bx as usize)].b() > mixed[at(rx as usize)].b());
        // Right AT the blue lamp, red must still be present (it travelled the
        // whole 12 cells: 28 - 24 = 4) — the overlap is not clipped.
        assert_eq!(mixed[at(bx as usize)].channels(), [4, 0, 28]);
        assert_eq!(mixed[at(rx as usize)].channels(), [28, 0, 4]);
    }

    #[test]
    fn skylight_seeps_under_a_single_covering_block() {
        let pos = SectionPos::new(0, 0, 0);
        let blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
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
    fn direct_skylight_stays_full_through_glass_roofs() {
        let pos = SectionPos::new(0, 0, 0);
        let (x, y, z) = (SECTION_SIZE + 8, SECTION_SIZE + 10, SECTION_SIZE + 8);
        let states = default_states();

        for glass in [Block::Glass, Block::GlassPane] {
            let mut blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
            blocks[nbhd_idx(x, y, z)] = glass.id();
            let mut surface = vec![-100i32; NBHD_AREA].into_boxed_slice();
            surface[z * NBHD + x] = 10;

            let cube = skylight(
                pos,
                cells(&blocks, &states),
                &surface,
                &mut FloodScratch::new(),
            );

            assert_eq!(
                cube[section_idx(8, 9, 8)],
                SKY_FULL,
                "{glass:?} must not dim direct skylight"
            );
        }
    }

    #[test]
    fn skylight_enters_a_stair_top_gap_but_not_its_solid_bottom() {
        let pos = SectionPos::new(0, 0, 0);
        let mut blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
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
        let mut blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
        let mut stairs = Vec::new();
        let mut surface = vec![-100i32; NBHD_AREA].into_boxed_slice();
        let (cx, cy, cz) = (SECTION_SIZE + 8, SECTION_SIZE + 8, SECTION_SIZE + 8);

        let place_stair = |blocks: &mut [u16],
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
        let blocks = vec![0u16; NBHD_VOLUME].into_boxed_slice();
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
