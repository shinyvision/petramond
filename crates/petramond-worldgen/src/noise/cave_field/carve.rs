use super::*;
use crate::data::underground::pattern::{ColumnCache, MaterialPattern};

pub(super) const MAX_FLOOR_DEPTH: usize = crate::data::underground::MAX_FLOOR_DEPTH as usize;

/// A course slot whose cell was not stone when the walk reached it. It still
/// OCCUPIES its depth — a course's reach is geometry, not block ids, because
/// only the batch owning a cell can see its id and a course crosses batches.
const NOT_STONE: usize = usize::MAX;

/// Per-carve scratch every column reuses: pattern evaluations rebound to the
/// column instead of rebuilt, and one scan per geology pattern met.
#[derive(Default)]
pub(super) struct Scratch {
    patterns: ColumnCache,
    geology: Geology,
}

#[derive(Default)]
struct Geology {
    scans: Vec<(usize, crate::formula::Scan<'static>)>,
    /// The column's stone heights — the only lanes a pattern is asked for —
    /// and each walked height's lane, `u16::MAX` where the cell is not stone.
    ys: Vec<f64>,
    lane: Vec<u16>,
    values: Vec<[f64; 1]>,
    /// The biomes met in the current column, and each one's materials over
    /// the column's height (pooled so a column never allocates).
    biomes: Vec<u8>,
    materials: Vec<Vec<u16>>,
}

impl Geology {
    #[allow(clippy::too_many_arguments)]
    fn materials(
        &mut self,
        pattern: &'static MaterialPattern,
        biome: u8,
        seed: u32,
        wx: i32,
        wz: i32,
        surf_y: i32,
    ) -> &[u16] {
        if let Some(k) = self.biomes.iter().position(|&b| b == biome) {
            return &self.materials[k];
        }
        let key = std::ptr::from_ref(pattern) as usize;
        let scan = match self.scans.iter().position(|(k, _)| *k == key) {
            Some(i) => &mut self.scans[i].1,
            None => {
                self.scans.push((key, pattern.scanner(seed)));
                &mut self.scans.last_mut().expect("just pushed").1
            }
        };
        match pattern.lattice() {
            Some(step) => scan.run_lattice(
                MaterialPattern::inputs(wx, wz, surf_y),
                &self.ys,
                step,
                &mut self.values,
            ),
            None => scan.run(
                MaterialPattern::inputs(wx, wz, surf_y),
                &self.ys,
                &mut self.values,
            ),
        }
        let k = self.biomes.len();
        self.biomes.push(biome);
        if self.materials.len() == k {
            self.materials.push(Vec::new());
        }
        pattern.materials(&self.values, &mut self.materials[k]);
        &self.materials[k]
    }
}

/// One batch carve's shared state. Both batch paths — whole column and cubic
/// section — drive the SAME column walk through this, because the orientation
/// lining is loop-shaped (it reads what the cell above turned out to be) and
/// two copies of that would be free to disagree below y=0, where the
/// chunk/section parity test does not look.
///
/// Sharing the walk is not enough on its own: its carry has to be seeded and
/// flushed by ASKING the carve field, never by assuming the box floor is a
/// world floor. Both ends of a column are box boundaries for exactly one of
/// the two paths, so anything remembered across a voxel is container-shaped
/// state unless the other end can re-derive it.
pub(super) struct BatchCarve<'a> {
    field: &'a CaveField,
    lat: &'a CaveLattice,
    may_cut: Vec<bool>,
    mx: usize,
    mz: usize,
    /// Hoisted: does any loaded row line per orientation? Everything the
    /// orientation rule needs — the run bookkeeping, the top-of-column probe,
    /// the biome read on a SOLID cell — hangs off this, so a table without it
    /// walks exactly the loop it always did.
    pub(super) faces: bool,
    air: u16,
    water: u16,
    stone: u16,
}

impl<'a> BatchCarve<'a> {
    pub(super) fn new(field: &'a CaveField, lat: &'a CaveLattice) -> Self {
        Self {
            field,
            lat,
            may_cut: lat.may_cut_mask(field.underground, field.lining_faces),
            mx: lat.nx - 1,
            mz: lat.nz - 1,
            faces: field.lining_faces && !lat.unlined,
            air: Block::Air.id(),
            water: Block::Water.id(),
            stone: Block::Stone.id(),
        }
    }

    /// Carve and line the inclusive column `y0..=y1`. `slot` maps a world Y to
    /// the caller's buffer index. Returns whether anything was carved.
    ///
    /// The walk ASCENDS, which is what makes the floor rule free: a cave floor
    /// is known one step after it is written, so it is repainted by index
    /// rather than discovered by probing `y+1` at every solid voxel — which
    /// would double the carve decisions in the hottest loop in worldgen. Only
    /// the column's two ENDS need real probes, because that is exactly where
    /// the neighbouring voxel belongs to another batch.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn column<const FACES: bool, F: Fn(i32) -> usize>(
        &self,
        blocks: &mut [u16],
        slot: F,
        wx: i32,
        wz: i32,
        y0: i32,
        y1: i32,
        surf_y: i32,
        scratch: &mut Scratch,
    ) -> bool {
        let (air, water, stone) = (self.air, self.water, self.stone);
        debug_assert_eq!(FACES, self.faces);
        let cxc = (wx.div_euclid(LATTICE_STEP) - self.lat.lx0) as usize;
        let czc = (wz.div_euclid(LATTICE_STEP) - self.lat.lz0) as usize;
        let col = czc * self.mx + cxc;
        // One cursor for the whole walk: x and z are fixed, so every lane's
        // xz interpolation is computed once per lattice cell instead of once
        // per lane per voxel.
        let mut cur = Col::new(self.lat, wx, wz);
        let patterns = &mut scratch.patterns;
        let stride = self.mz * self.mx;
        let ly0 = self.lat.ly0;
        let mut carved = false;
        // The contiguous run of solid cells immediately below the cursor, in
        // ascending Y — what a cave floor opening at the cursor paints.
        let mut run = [(0i32, NOT_STONE); MAX_FLOOR_DEPTH];
        let mut run_len = 0usize;
        // Seeded, not assumed: the voxel under the box floor is the other
        // batch's, so whether the box's lowest rock is a CEILING is a question
        // for the carve field. Guessing `false` here is one whole voxel plane
        // per section taking the WALL rule.
        let mut below_open = FACES
            && self.field.batch_y_pad_low(y0) != 0
            && self.field.cut_col(&mut cur, y0 - 1, surf_y).is_open();
        let mut y = y0;
        while y <= y1 {
            let cyc = (y.div_euclid(LATTICE_STEP) - ly0) as usize;
            if !self.may_cut[cyc * stride + col] {
                // Provably solid cell: jump to the next cell floor.
                y = (y.div_euclid(LATTICE_STEP) + 1) * LATTICE_STEP;
                if FACES {
                    run_len = 0;
                    below_open = false;
                }
                continue;
            }
            let i = slot(y);
            let id = blocks[i];
            let treatment = self.lat.volumes.at([wx, y, wz]);
            if (id == air || id == water) && !matches!(treatment, super::volumes::Cell::Fill(_)) {
                y += 1;
                if FACES {
                    run_len = 0;
                    below_open = false;
                }
                continue;
            }
            let cut = self.field.cut_col_treated(&mut cur, y, surf_y, treatment);
            match cut {
                // Open air, or the block the cell generates as instead: a
                // positioned field's content or a pool's fluid.
                CaveCut::Air | CaveCut::Fill(_) => {
                    let block = match cut {
                        CaveCut::Fill(block) => block,
                        _ => air,
                    };
                    blocks[i] = block;
                    carved = true;
                    if FACES {
                        if Block::from_id(block).is_solid() {
                            below_open = false;
                        } else {
                            self.paint_floor(blocks, &run[..run_len], &mut cur, patterns, block);
                            below_open = true;
                        }
                        run_len = 0;
                    }
                }
                cut => {
                    if let CaveCut::Barrier(block) = cut {
                        blocks[i] = block;
                    }
                    if cut == CaveCut::Shell && id == stone && !self.lat.unlined {
                        self.paint_side::<FACES>(blocks, i, below_open, &mut cur, patterns, y);
                    }
                    if FACES {
                        if run_len == MAX_FLOOR_DEPTH {
                            run.copy_within(1.., 0);
                            run_len -= 1;
                        }
                        let paintable = id == stone && !matches!(cut, CaveCut::Barrier(_));
                        run[run_len] = (y, if paintable { i } else { NOT_STONE });
                        run_len += 1;
                        below_open = false;
                    }
                }
            }
            y += 1;
        }
        // The column ended on rock. Whether that rock is a cave FLOOR, and how
        // far below the floor it sits, is the next batch's business — so this
        // is the other place the rule has to ask rather than remember, and the
        // lattice was padded by the deepest declared course for it.
        if FACES && run_len > 0 {
            self.paint_floor_below_top(blocks, &run[..run_len], &mut cur, patterns, y1, surf_y);
        }
        // A column with no regional candidate can only resolve to a
        // nearest-climate row, so when none of those has a pattern the pass
        // would visit every stone cell to write nothing.
        if self.lat.geology
            && (cur.region.has_candidates() || self.field.underground.geology_via_climate)
        {
            let geology = &mut scratch.geology;
            geology.biomes.clear();
            geology.ys.clear();
            geology.lane.clear();
            for yy in y0..=y1 {
                let lane = if blocks[slot(yy)] == stone {
                    geology.ys.push(f64::from(yy));
                    geology.ys.len() as u16 - 1
                } else {
                    u16::MAX
                };
                geology.lane.push(lane);
            }
            // The regional answer holds over a span of cells; only the
            // nearest-climate fallback has to be asked per cell.
            let mut y = y0;
            while y <= y1 && !geology.ys.is_empty() {
                let (span, end) = cur.region.id_span(self.field, y, y1);
                for yy in y..=end {
                    let lane = geology.lane[(yy - y0) as usize];
                    if lane == u16::MAX {
                        continue;
                    }
                    let biome =
                        span.unwrap_or_else(|| self.field.underground.id_at(cur.climate(yy), yy));
                    let Some(pattern) = self.field.underground.geology(biome) else {
                        continue;
                    };
                    let materials =
                        geology.materials(pattern, biome, self.field.seed, wx, wz, surf_y);
                    let i = slot(yy);
                    blocks[i] = materials[usize::from(lane)];
                    carved |= blocks[i] != stone;
                }
                y = end + 1;
            }
        }
        // A field's courses lie on the rock the walk just settled: laid now
        // that the anchor's block is known, over geology and lining alike.
        for course in self.lat.volumes.column_courses(wx, y0, wz) {
            let ty = course.y(y0.div_euclid(16));
            if ty < y0 || ty > y1 {
                continue;
            }
            let anchor_y = ty + i32::from(course.anchor_dy);
            let anchor_solid = if (y0..=y1).contains(&anchor_y) {
                Block::from_id(blocks[slot(anchor_y)]).is_solid()
            } else {
                !self.field.cut_col(&mut cur, anchor_y, surf_y).is_open()
            };
            if !anchor_solid {
                continue;
            }
            let i = slot(ty);
            let prior = blocks[i];
            if course.rule.avoid.contains(&prior) {
                continue;
            }
            let block = course
                .rule
                .remap
                .iter()
                .find(|p| p[0] == prior)
                .map_or(course.block, |p| p[1]);
            blocks[i] = course.rule.patched(block, self.field.seed, [wx, ty, wz]);
            carved = true;
        }
        carved
    }

    /// The floor rule owning a course whose TOP cell is `top_y`. The whole
    /// course resolves against that one cell: it is the only cell BOTH batches
    /// sharing a course can name, so resolving per cell — or against whichever
    /// end of the course happened to fall inside this box — would hand the two
    /// paths different rows wherever a course crosses a band edge.
    #[inline]
    fn floor_rule(&self, cur: &mut Col, top_y: i32) -> Option<&'static LiningFaces> {
        let id = self.field.biome_id_col(cur, top_y);
        let f = self.field.underground.faces(id)?;
        (f.floor.block != self.air).then_some(f)
    }

    /// Paint the top `take` cells of a course. Slots the walk saw as non-stone
    /// are skipped but still spent, so how deep the course reaches does not
    /// depend on what the rock happened to be made of above this box.
    ///
    /// `depth0` is the COURSE depth of the run's top cell — `0` when this run
    /// starts at the course top, and the number of cells already spent above
    /// this box otherwise. It is what tells a `floor_under` layering which
    /// cell is the surface, and it must come from the caller: a run that
    /// begins mid-course cannot see its own top, and guessing `0` would paint
    /// a second surface on every box boundary.
    #[inline]
    #[allow(clippy::too_many_arguments)]
    fn paint_run(
        &self,
        blocks: &mut [u16],
        run: &[(i32, usize)],
        f: &LiningFaces,
        cur: &Col,
        patterns: &mut ColumnCache,
        take: usize,
        depth0: i32,
        over: u16,
    ) {
        let (wx, wz) = (cur.x, cur.z);
        for (k, &(y, i)) in run.iter().rev().take(take).enumerate() {
            let lining = match f.floor_under {
                Some(under) if depth0 + k as i32 > 0 => under,
                _ => f.floor_surface(over),
            };
            if i != NOT_STONE && face_roll(self.field.seed, f.salt, lining.weight, wx, y, wz) {
                blocks[i] = self.field.underground.face_material_cached(
                    self.field.seed,
                    f.biome,
                    lining,
                    [wx, y, wz],
                    patterns,
                );
            }
        }
    }

    /// Paint the rock under a cave floor the walk just cut, whose open cell
    /// holds `over`: the run's top cell IS the course top.
    #[inline]
    fn paint_floor(
        &self,
        blocks: &mut [u16],
        run: &[(i32, usize)],
        cur: &mut Col,
        patterns: &mut ColumnCache,
        over: u16,
    ) {
        let Some(&(top_y, _)) = run.last() else {
            return;
        };
        let Some(f) = self.floor_rule(cur, top_y) else {
            return;
        };
        let take =
            f.floor_depth
                .at_cached(self.field.seed, [cur.x, top_y, cur.z], patterns) as usize;
        self.paint_run(blocks, run, f, cur, patterns, take, 0, over);
    }

    /// Paint a run left at the box's top voxel. The cave floor that owns it can
    /// be up to a full course above the box, so both the floor's existence and
    /// the run's DEPTH under it are probed — and the rule is resolved at the
    /// course's real top cell, which is what the batch containing that cell
    /// resolves against too.
    fn paint_floor_below_top(
        &self,
        blocks: &mut [u16],
        run: &[(i32, usize)],
        cur: &mut Col,
        patterns: &mut ColumnCache,
        y1: i32,
        surf_y: i32,
    ) {
        for above in 0..self.field.batch_y_pad() {
            let cut = self.field.cut_col(cur, y1 + 1 + above, surf_y);
            if !cut.is_open() {
                continue;
            }
            let Some(f) = self.floor_rule(cur, y1 + above) else {
                return;
            };
            let reach =
                f.floor_depth
                    .at_cached(self.field.seed, [cur.x, y1 + above, cur.z], patterns)
                    - above;
            if reach > 0 {
                // The block the in-box floor would have read: an open cell's
                // fill is its cut.
                let over = match cut {
                    CaveCut::Fill(block) => block,
                    _ => self.air,
                };
                self.paint_run(blocks, run, f, cur, patterns, reach as usize, above, over);
            }
            return;
        }
    }

    /// Paint a cell that hugs a cave wall: the CEILING rule when the voxel
    /// below it was carved, the WALL rule otherwise. A floor repaints over this
    /// on the next step, which is the precedence a floor GUARANTEE needs.
    #[inline]
    fn paint_side<const FACES: bool>(
        &self,
        blocks: &mut [u16],
        i: usize,
        below_open: bool,
        cur: &mut Col,
        patterns: &mut ColumnCache,
        y: i32,
    ) {
        let (wx, wz) = (cur.x, cur.z);
        let id = self.field.biome_id_col(cur, y);
        let bare = |blocks: &mut [u16], patterns: &mut ColumnCache| {
            let lining = self.field.underground.lining(id);
            if lining != self.air {
                blocks[i] = self.field.underground.surface_material_cached(
                    self.field.seed,
                    id,
                    lining,
                    [wx, y, wz],
                    patterns,
                );
            }
        };
        // A table with no orientation rule never reads the (dense, and two
        // orders of magnitude larger) per-orientation array at all.
        if !FACES {
            return bare(blocks, patterns);
        }
        let Some(f) = self.field.underground.faces(id) else {
            return bare(blocks, patterns);
        };
        let face = if below_open { f.ceiling } else { f.wall };
        if face.block != self.air && face_roll(self.field.seed, f.salt, face.weight, wx, y, wz) {
            blocks[i] = self.field.underground.face_material_cached(
                self.field.seed,
                id,
                face,
                [wx, y, wz],
                patterns,
            );
        }
    }
}

/// Positional dither for a partial-coverage face rule. A weight of 1 takes NO
/// draw at all — that is what separates a guarantee from a 0.99.
#[inline]
fn face_roll(seed: u32, salt: u64, weight: f32, x: i32, y: i32, z: i32) -> bool {
    if weight >= 1.0 {
        return true;
    }
    weight > 0.0 && crate::rng::FeatureRng::positional(seed, salt, x, y, z).chance(weight)
}
