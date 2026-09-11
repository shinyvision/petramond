use super::{
    column_key, site, CaveField, Cell, CourseCell, Excavation, FieldShape, Fill, Seal, Site, Tile,
};
use crate::data::excavations::effects::MaterialFilter;
use crate::formula::Inputs;
use crate::rng::FeatureRng;
use crate::terrain_query::heights_at;
use petramond_world::block::Block;
use petramond_world::chunk::{WORLD_MAX_Y, WORLD_MIN_Y};

mod projection;

struct Plan<'a> {
    site: Site,
    row: &'a Excavation,
    shape: &'a FieldShape,
    biome: u8,
    salt: u64,
}

impl Plan<'_> {
    fn inputs(&self, [x, y, z]: [i32; 3], surface: i32) -> Inputs {
        self.site.inputs([x, y, z], surface)
    }
}

#[derive(Clone, Copy, Default)]
struct DraftCell {
    cell: Cell,
    owner: usize,
}

struct Draft {
    lo: [i32; 3],
    side: [usize; 3],
    cells: Vec<DraftCell>,
}

impl Draft {
    fn new(origin: [i32; 3], pad: [i32; 3]) -> Self {
        let lo = std::array::from_fn(|a| origin[a] - pad[a]);
        let side = pad.map(|p| (16 + 2 * p) as usize);
        Self {
            lo,
            side,
            cells: vec![DraftCell::default(); side.iter().product()],
        }
    }

    fn index(&self, pos: [i32; 3]) -> Option<usize> {
        let p: [i32; 3] = std::array::from_fn(|a| pos[a] - self.lo[a]);
        (0..3)
            .all(|a| (0..self.side[a] as i32).contains(&p[a]))
            .then(|| (p[1] as usize * self.side[2] + p[2] as usize) * self.side[0] + p[0] as usize)
    }

    fn position(&self, i: usize) -> [i32; 3] {
        [
            self.lo[0] + (i % self.side[0]) as i32,
            self.lo[1] + (i / (self.side[0] * self.side[2])) as i32,
            self.lo[2] + (i / self.side[0] % self.side[2]) as i32,
        ]
    }

    fn read(&self, pos: [i32; 3], previous: &mut impl FnMut([i32; 3]) -> u16) -> u16 {
        match self.index(pos).map(|i| self.cells[i].cell) {
            Some(Cell::Fill(Fill { block, .. })) => block,
            _ => previous(pos),
        }
    }

    fn write(&mut self, pos: [i32; 3], block: u16, biome: u8, owner: usize) {
        if !(WORLD_MIN_Y + 1..WORLD_MAX_Y).contains(&pos[1]) {
            return;
        }
        if let Some(i) = self.index(pos) {
            self.cells[i] = DraftCell {
                cell: Cell::Fill(Fill {
                    block,
                    biome,
                    replace: MaterialFilter::Any,
                }),
                owner,
            };
        }
    }
}

fn padding(shape: &FieldShape) -> [i32; 3] {
    let mut pad = [1; 3];
    if !shape.projections.is_empty() {
        pad = [2, 1, 2];
    }
    for course in &shape.courses {
        for (a, p) in pad.iter_mut().enumerate() {
            let end = course.offset[a] + course.step[a] * i32::from(course.length[1] - 1);
            *p = (*p).max(course.offset[a].abs().max(end.abs()) + 1);
        }
    }
    pad
}

pub(super) fn build_tile(field: &CaveField, tile: [i32; 3]) -> Tile {
    let origin = tile.map(|v| v * 16);
    let mut plans = Vec::new();
    let mut pad = [1; 3];
    let empty = Tile {
        cells: None,
        touched: 0,
        seals: Box::default(),
        courses: Box::default(),
    };
    for row in &field.excavations.rows {
        let Some(shape) = row.field() else {
            continue;
        };
        let margin = padding(shape);
        if origin[1] - margin[1] > shape.y[1] || origin[1] + 15 + margin[1] < shape.y[0] {
            continue;
        }
        let reach = shape.bound_radius;
        let spacing = row.placement.spacing;
        for z in (origin[2] - margin[2] - reach).div_euclid(spacing)
            ..=(origin[2] + 15 + margin[2] + reach).div_euclid(spacing)
        {
            for x in (origin[0] - margin[0] - reach).div_euclid(spacing)
                ..=(origin[0] + 15 + margin[0] + reach).div_euclid(spacing)
            {
                let Some(site) = site(field, row, shape, [x, z]) else {
                    continue;
                };
                if !site.bounds.intersects(
                    std::array::from_fn(|a| origin[a] - margin[a]),
                    std::array::from_fn(|a| origin[a] + 15 + margin[a]),
                ) {
                    continue;
                }
                plans.push(Plan {
                    site,
                    row,
                    shape,
                    biome: row.placement.underground_biome.unwrap_or(0),
                    salt: row.salt,
                });
                for a in 0..3 {
                    pad[a] = pad[a].max(margin[a]);
                }
            }
        }
    }
    if plans.is_empty() {
        return empty;
    }
    let mut draft = Draft::new(origin, pad);
    let columns: Vec<_> = (0..draft.side[2])
        .flat_map(|z| {
            (0..draft.side[0])
                .map(move |x| [origin[0] + x as i32 - pad[0], origin[2] + z as i32 - pad[2]])
        })
        .collect();
    let surfaces = heights_at(field.seed, &columns);
    carve(&mut draft, &plans, &surfaces, field.seed);
    let mut seals = Vec::new();
    let mut courses = Vec::new();
    if plans.iter().any(|p| !p.shape.boundaries.is_empty()) {
        boundaries(&mut draft, &plans, &mut seals);
    }
    if plans.iter().any(|p| !p.shape.courses.is_empty()) {
        course_cells(&draft, &plans, &surfaces, field.seed, &mut courses);
    }
    if plans.iter().any(|p| !p.shape.projections.is_empty()) {
        projection::apply(&mut draft, &plans, &surfaces, field);
    }
    finish(draft, origin, seals, courses)
}

/// The lattice step the cut margins are sampled on, like the cave density;
/// a recipe with steps or bands in its height (a `step`, `trunc` or `select`
/// of the height) is sampled twice as densely along the height so its
/// risers stay crisp.
const STEP: usize = 4;
const FINE_STEP: usize = 2;
/// A corner no member reaches: far enough below zero that interpolation
/// towards it never turns positive within a step.
const NONE: f64 = -1.0e9;

/// Sample every member's cut margin at the draft's lattice corners, keep
/// the union and its first owner per corner, and fill the cells where the
/// interpolated margin is positive. The cut's boundary is smooth at the
/// lattice scale by construction, the way the cave density's is.
fn carve(draft: &mut Draft, plans: &[Plan<'static>], surfaces: &[i32], seed: u32) {
    let step_y = if plans
        .iter()
        .any(|p| p.shape.margin.discontinuous_in_height())
    {
        FINE_STEP
    } else {
        STEP
    };
    let steps = [STEP, step_y, STEP];
    let corners: [usize; 3] = std::array::from_fn(|a| (draft.side[a] - 1).div_ceil(steps[a]) + 1);
    let total: usize = corners.iter().product();
    let index = |cx: usize, cy: usize, cz: usize| (cy * corners[2] + cz) * corners[0] + cx;
    let mut margin = vec![NONE; total];
    let mut owner = vec![u16::MAX; total];
    let ys: Vec<f64> = (0..corners[1])
        .map(|cy| f64::from(draft.lo[1] + (cy * step_y) as i32))
        .collect();
    let slack = STEP as i32;
    let mut first_owner = 0;
    for group in plans.chunk_by(|a, b| std::ptr::eq(a.shape, b.shape)) {
        let shape = group[0].shape;
        let first = group[0].inputs([0; 3], 0);
        let varying: Vec<_> = (3..=7)
            .filter(|&i| {
                group[1..]
                    .iter()
                    .any(|plan| plan.inputs([0; 3], 0).0[i] != first.0[i])
            })
            .collect();
        let batch = shape.margin.batch(&varying);
        let mut scan = batch.scan(seed);
        let mut members = Vec::new();
        let mut inputs = Vec::new();
        for cz in 0..corners[2] {
            for cx in 0..corners[0] {
                let wx = draft.lo[0] + (cx * STEP) as i32;
                let wz = draft.lo[2] + (cz * STEP) as i32;
                let sx = (cx * STEP).min(draft.side[0] - 1);
                let sz = (cz * STEP).min(draft.side[2] - 1);
                let surface = surfaces[sz * draft.side[0] + sx];
                members.clear();
                members.extend(group.iter().enumerate().rev().filter(|(_, plan)| {
                    let b = plan.site.bounds;
                    (b.min[0] - slack..=b.max[0] + slack).contains(&wx)
                        && (b.min[2] - slack..=b.max[2] + slack).contains(&wz)
                }));
                if members.is_empty() {
                    continue;
                }
                inputs.clear();
                inputs.extend(
                    members
                        .iter()
                        .map(|(_, plan)| plan.inputs([wx, 0, wz], surface)),
                );
                scan.run(inputs[0], &inputs, &ys);
                for (lane, &wy) in ys.iter().enumerate() {
                    let wy = wy as i32;
                    let i = index(cx, lane, cz);
                    for (m, (o, plan)) in members.iter().enumerate() {
                        let b = plan.site.bounds;
                        if wy < b.min[1] - slack || wy > b.max[1] + slack {
                            continue;
                        }
                        let [value] = scan.output::<1>(m, lane);
                        if !value.is_finite() {
                            continue;
                        }
                        if value > margin[i] {
                            margin[i] = value;
                        }
                        if value > 0.0 && owner[i] == u16::MAX {
                            owner[i] = (first_owner + o) as u16;
                        }
                    }
                }
            }
        }
        first_owner += group.len();
    }

    let constant: Vec<Option<f64>> = plans
        .iter()
        .map(|plan| {
            (!plan.shape.material_varies).then(|| {
                plan.shape
                    .material
                    .column_seeded(seed, plan.inputs([0; 3], 0))
                    .at::<1>(0.0)[0]
            })
        })
        .collect();
    let mut evaluations: Vec<Option<(usize, crate::formula::Evaluation<'static>)>> =
        (0..plans.len()).map(|_| None).collect();
    let weight = |k: usize, step: usize| (k % step) as f64 / step as f64;
    for z in 0..draft.side[2] {
        for x in 0..draft.side[0] {
            let column = z * draft.side[0] + x;
            let surface = surfaces[column];
            let (wx, wz) = (draft.lo[0] + x as i32, draft.lo[2] + z as i32);
            let (cx, fx) = (x / STEP, weight(x, STEP));
            let (cz, fz) = (z / STEP, weight(z, STEP));
            for y in 0..draft.side[1] {
                let (cy, fy) = (y / step_y, weight(y, step_y));
                let mut value = 0.0;
                let mut best = (NONE, u16::MAX);
                for (dx, dy, dz) in [
                    (0, 0, 0),
                    (1, 0, 0),
                    (0, 1, 0),
                    (1, 1, 0),
                    (0, 0, 1),
                    (1, 0, 1),
                    (0, 1, 1),
                    (1, 1, 1),
                ] {
                    let i = index(cx + dx, cy + dy, cz + dz);
                    let w = if dx == 1 { fx } else { 1.0 - fx }
                        * if dy == 1 { fy } else { 1.0 - fy }
                        * if dz == 1 { fz } else { 1.0 - fz };
                    value += w * margin[i];
                    if owner[i] != u16::MAX && margin[i] > best.0 {
                        best = (margin[i], owner[i]);
                    }
                }
                if value <= 0.0 || best.1 == u16::MAX {
                    continue;
                }
                let o = usize::from(best.1);
                let plan = &plans[o];
                let shape = plan.shape;
                let wy = draft.lo[1] + y as i32;
                if wy < shape.y[0]
                    || wy > shape.y[1]
                    || wy > surface + shape.surface_offset
                    || !plan.site.bounds.contains_column(wx, wz)
                    || wy < plan.site.bounds.min[1]
                    || wy > plan.site.bounds.max[1]
                {
                    continue;
                }
                let material = match constant[o] {
                    Some(value) => value,
                    None => {
                        let (bound, evaluation) = evaluations[o].get_or_insert_with(|| {
                            (
                                usize::MAX,
                                shape
                                    .material
                                    .column_seeded(seed, plan.inputs([wx, 0, wz], surface)),
                            )
                        });
                        if *bound != column {
                            evaluation.rebind(plan.inputs([wx, 0, wz], surface));
                            *bound = column;
                        }
                        evaluation.at::<1>(f64::from(wy))[0]
                    }
                };
                let material =
                    shape.palette[material.max(0.0).min((shape.palette.len() - 1) as f64) as usize];
                draft.cells[(y * draft.side[2] + z) * draft.side[0] + x] = DraftCell {
                    cell: Cell::Fill(Fill {
                        block: material.block,
                        biome: plan.biome,
                        replace: material.replace,
                    }),
                    owner: o,
                };
            }
        }
    }
}

fn shift(pos: [i32; 3], delta: [i32; 3]) -> [i32; 3] {
    std::array::from_fn(|a| pos[a] + delta[a])
}

/// Boundary rules: a filled neighbour that qualifies is replaced now; a
/// terrain neighbour gets a seal the walk applies once it knows the block.
fn boundaries(draft: &mut Draft, plans: &[Plan<'static>], seals: &mut Vec<(usize, Seal)>) {
    let mut fills = Vec::new();
    for (i, source) in draft.cells.iter().enumerate() {
        let Cell::Fill(fill) = source.cell else {
            continue;
        };
        let pos = draft.position(i);
        for rule in &plans[source.owner].shape.boundaries {
            if !rule.from.contains(&fill.block) {
                continue;
            }
            for &offset in &rule.offsets {
                let target = shift(pos, offset);
                let Some(t) = draft.index(target) else {
                    continue;
                };
                match draft.cells[t].cell {
                    Cell::Fill(other) => {
                        if !rule.from.contains(&other.block)
                            && !rule.avoid.contains(&other.block)
                            && rule.replace.accepts(other.block)
                        {
                            fills.push((t, rule.block, fill.biome, source.owner));
                        }
                    }
                    _ => seals.push((
                        t,
                        Seal {
                            key: 0,
                            block: rule.block,
                            replace: rule.replace,
                        },
                    )),
                }
            }
        }
    }
    for (t, block, biome, owner) in fills {
        draft.cells[t] = DraftCell {
            cell: Cell::Fill(Fill {
                block,
                biome,
                replace: MaterialFilter::Any,
            }),
            owner,
        };
    }
}

/// Course rules: every cell a course would lay, remembered with its anchor;
/// the walk lays it once it knows the anchor is rock the course can sit on.
fn course_cells(
    draft: &Draft,
    plans: &[Plan<'static>],
    surfaces: &[i32],
    seed: u32,
    out: &mut Vec<(usize, CourseCell)>,
) {
    for (i, source) in draft.cells.iter().enumerate() {
        let Cell::Fill(fill) = source.cell else {
            continue;
        };
        let plan = &plans[source.owner];
        let pos = draft.position(i);
        for (rule_index, rule) in plan.shape.courses.iter().enumerate() {
            if !rule.from.contains(&fill.block) {
                continue;
            }
            let anchor = shift(pos, rule.offset);
            let Some(a) = draft.index(anchor) else {
                continue;
            };
            // An anchor the field filled itself is settled here; terrain
            // anchors are the walk's to judge.
            if let Cell::Fill(filled) = draft.cells[a].cell {
                if !Block::from_id(filled.block).is_solid() || rule.avoid.contains(&filled.block) {
                    continue;
                }
            }
            let surface = surfaces[((anchor[2] - draft.lo[2]) as usize * draft.side[0])
                + (anchor[0] - draft.lo[0]) as usize];
            let [extent, when] = rule
                .formula
                .column_seeded(seed, plan.inputs(anchor, surface))
                .at::<2>(anchor[1] as f64);
            if when <= 0.0 || !when.is_finite() {
                continue;
            }
            let mut rng = FeatureRng::positional(
                seed,
                plan.salt.wrapping_add(rule_index as u64),
                anchor[0],
                anchor[1],
                anchor[2],
            );
            let count = if rule.has_extent {
                extent.round() as i32
            } else {
                rng.next_i32(rule.length[0].into(), rule.length[1].into())
            }
            .clamp(rule.length[0].into(), rule.length[1].into());
            for step in 0..count {
                let target = shift(anchor, rule.step.map(|v| v * step));
                let Some(t) = draft.index(target) else {
                    break;
                };
                let Ok(anchor_dy) = i8::try_from(anchor[1] - target[1]) else {
                    break;
                };
                let mut block = rule.palette[(step as usize).min(rule.palette.len() - 1)];
                if let Cell::Fill(filled) = draft.cells[t].cell {
                    if rule.avoid.contains(&filled.block) {
                        break;
                    }
                    if let Some(pair) = rule.remap.iter().find(|p| p[0] == filled.block) {
                        block = pair[1];
                    }
                }
                out.push((
                    t,
                    CourseCell {
                        key: 0,
                        block,
                        anchor_dy,
                        rule,
                    },
                ));
            }
        }
    }
}

fn finish(
    draft: Draft,
    origin: [i32; 3],
    seals: Vec<(usize, Seal)>,
    courses: Vec<(usize, CourseCell)>,
) -> Tile {
    let mut out = vec![Cell::Untouched; 4096];
    let mut any = false;
    let mut touched = 0u64;
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                let pos = shift(origin, [x, y, z]);
                let mut cell = draft.cells[draft.index(pos).expect("tile interior")].cell;
                if matches!(cell, Cell::Untouched) {
                    for d in [
                        [-1, 0, 0],
                        [1, 0, 0],
                        [0, -1, 0],
                        [0, 1, 0],
                        [0, 0, -1],
                        [0, 0, 1],
                    ] {
                        if let Cell::Fill(Fill { block, .. }) =
                            draft.cells[draft.index(shift(pos, d)).expect("padded tile")].cell
                        {
                            if !Block::from_id(block).is_solid() {
                                cell = Cell::Surface;
                                break;
                            }
                        }
                    }
                }
                if !matches!(cell, Cell::Untouched) {
                    any = true;
                    touched |= 1 << ((y / 4 * 4 + z / 4) * 4 + x / 4);
                }
                out[((y * 16 + z) * 16 + x) as usize] = cell;
            }
        }
    }
    let inside = |i: usize| {
        let p = draft.position(i);
        (0..3)
            .all(|a| (origin[a]..origin[a] + 16).contains(&p[a]))
            .then(|| column_key(p[0], p[1], p[2]))
    };
    // Later writes win, as they did when every rule wrote the draft directly.
    let mut seals: Vec<Seal> = seals
        .into_iter()
        .rev()
        .filter_map(|(i, mut seal)| {
            seal.key = inside(i)?;
            Some(seal)
        })
        .collect();
    seals.sort_by_key(|s| s.key);
    seals.dedup_by_key(|s| s.key);
    let mut courses: Vec<CourseCell> = courses
        .into_iter()
        .rev()
        .filter_map(|(i, mut course)| {
            course.key = inside(i)?;
            Some(course)
        })
        .collect();
    courses.sort_by_key(|c| c.key);
    courses.dedup_by_key(|c| c.key);
    any |= !courses.is_empty();
    Tile {
        cells: any.then(|| out.into_boxed_slice()),
        touched,
        seals: seals.into(),
        courses: courses.into(),
    }
}

#[cfg(test)]
mod tests;
