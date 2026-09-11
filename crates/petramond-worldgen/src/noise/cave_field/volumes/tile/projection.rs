use super::{shift, site, CaveField, Draft, Excavation, FieldShape, Plan, Site};
use crate::data::excavations::effects::Projection;
use crate::formula::{BatchScan, Scan};
use crate::memo::SharedMemo;
use crate::terrain_query::height_tile;
use petramond_world::block::Block;
use petramond_world::chunk::{SEA_LEVEL, WORLD_MAX_Y, WORLD_MIN_Y};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

/// Per rule, the scans its stages reuse across every target.
struct RuleScans<'a> {
    admission: Scan<'a>,
    material: Scan<'a>,
    preserve: Vec<Scan<'a>>,
}

/// The terrain a projection's rules compare against where the field left a
/// cell alone: rock under the surface, water between the surface and the
/// sea, air above. A cave or an ice sheet at that exact cell is a detail
/// the mounts do without.
fn terrain_guess(y: i32, surface: i32) -> u16 {
    if y <= surface {
        Block::Stone.id()
    } else if y < SEA_LEVEL {
        Block::Water.id()
    } else {
        Block::Air.id()
    }
}

type PlaneKey = (u32, [usize; 2], [i32; 2], usize, i32);
/// What stands at one height across a 16×16 chunk once the field has cut:
/// the union of every site's cut on the margin lattice, its material where
/// it cuts and the terrain guess elsewhere. A probe reads this plane.
static PLANES: LazyLock<SharedMemo<PlaneKey, Arc<[u16; 256]>>> =
    LazyLock::new(|| SharedMemo::new(2048));

/// The sites of `row` whose bounds reach the chunk within the shape's
/// height range, in placement order.
fn reaching_sites(
    field: &CaveField,
    row: &'static Excavation,
    shape: &'static FieldShape,
    chunk: [i32; 2],
) -> Vec<Site> {
    let origin = [chunk[0] * 16, chunk[1] * 16];
    let spacing = row.placement.spacing;
    let reach = shape.bound_radius;
    let mut sites = Vec::new();
    for z in (origin[1] - reach).div_euclid(spacing)..=(origin[1] + 15 + reach).div_euclid(spacing)
    {
        for x in
            (origin[0] - reach).div_euclid(spacing)..=(origin[0] + 15 + reach).div_euclid(spacing)
        {
            let Some(site) = site(field, row, shape, [x, z]) else {
                continue;
            };
            if site.bounds.intersects(
                [origin[0], shape.y[0], origin[1]],
                [origin[0] + 15, shape.y[1], origin[1] + 15],
            ) {
                sites.push(site);
            }
        }
    }
    sites
}

/// The input lanes that differ between sites.
fn varying_of(sites: &[Site]) -> Vec<usize> {
    let first = sites[0].inputs([0; 3], 0);
    (3..=7)
        .filter(|&i| {
            sites[1..]
                .iter()
                .any(|s| s.inputs([0; 3], 0).0[i] != first.0[i])
        })
        .collect()
}

fn probe_plane(
    field: &CaveField,
    row: &'static Excavation,
    shape: &'static FieldShape,
    chunk: [i32; 2],
    y: i32,
) -> Arc<[u16; 256]> {
    let key = (
        field.seed,
        field.table_identities(),
        chunk,
        std::ptr::from_ref(shape) as usize,
        y,
    );
    PLANES.get_or_insert(key, || {
        let origin = [chunk[0] * 16, chunk[1] * 16];
        let heights = height_tile(field.seed, chunk);
        let mut out = [0u16; 256];
        for (i, cell) in out.iter_mut().enumerate() {
            *cell = terrain_guess(y, heights[i]);
        }
        let sites = reaching_sites(field, row, shape, chunk);
        if sites.is_empty() || y < shape.y[0] || y > shape.y[1] {
            return Arc::new(out);
        }
        let batch = shape.margin.batch(&varying_of(&sites));
        let mut scan = batch.scan(field.seed);
        // Corners four apart across the chunk and the two lattice heights
        // bracketing `y`.
        let base = y.div_euclid(STEP) * STEP;
        let ys = [f64::from(base), f64::from(base + STEP)];
        let t = f64::from(y - base) / f64::from(STEP);
        let mut margin = [[NONE; 2]; 25];
        let mut owner = [[usize::MAX; 2]; 25];
        let mut members = Vec::new();
        let mut inputs = Vec::new();
        for cz in 0..5 {
            for cx in 0..5 {
                let (wx, wz) = (origin[0] + cx as i32 * STEP, origin[1] + cz as i32 * STEP);
                let surface = heights[(cz * 4).min(15) * 16 + (cx * 4).min(15)];
                members.clear();
                members.extend(sites.iter().enumerate().rev().filter(|(_, s)| {
                    let b = s.bounds;
                    (b.min[0] - STEP..=b.max[0] + STEP).contains(&wx)
                        && (b.min[2] - STEP..=b.max[2] + STEP).contains(&wz)
                }));
                if members.is_empty() {
                    continue;
                }
                inputs.clear();
                inputs.extend(members.iter().map(|(_, s)| s.inputs([wx, 0, wz], surface)));
                scan.run(inputs[0], &inputs, &ys);
                let corner = cz * 5 + cx;
                for lane in 0..2 {
                    for (m, (o, _)) in members.iter().enumerate() {
                        let [value] = scan.output::<1>(m, lane);
                        if !value.is_finite() {
                            continue;
                        }
                        if value > margin[corner][lane] {
                            margin[corner][lane] = value;
                        }
                        if value > 0.0 && owner[corner][lane] == usize::MAX {
                            owner[corner][lane] = *o;
                        }
                    }
                }
            }
        }
        for z in 0..16usize {
            for x in 0..16usize {
                let (cx, fx) = (x / 4, (x % 4) as f64 / 4.0);
                let (cz, fz) = (z / 4, (z % 4) as f64 / 4.0);
                let mut value = 0.0;
                let mut best = (NONE, usize::MAX);
                for (dx, dz, lane) in [
                    (0, 0, 0),
                    (1, 0, 0),
                    (0, 1, 0),
                    (1, 1, 0),
                    (0, 0, 1),
                    (1, 0, 1),
                    (0, 1, 1),
                    (1, 1, 1),
                ] {
                    let corner = (cz + dz) * 5 + cx + dx;
                    let w = if dx == 1 { fx } else { 1.0 - fx }
                        * if dz == 1 { fz } else { 1.0 - fz }
                        * if lane == 1 { t } else { 1.0 - t };
                    value += w * margin[corner][lane];
                    if owner[corner][lane] != usize::MAX && margin[corner][lane] > best.0 {
                        best = (margin[corner][lane], owner[corner][lane]);
                    }
                }
                let surface = heights[z * 16 + x];
                if value <= 0.0 || best.1 == usize::MAX || y > surface + shape.surface_offset {
                    continue;
                }
                let site = sites[best.1];
                let (wx, wz) = (origin[0] + x as i32, origin[1] + z as i32);
                let [index] = shape
                    .material
                    .column_seeded(field.seed, site.inputs([wx, 0, wz], surface))
                    .at::<1>(f64::from(y));
                out[z * 16 + x] = shape.palette
                    [index.max(0.0).min((shape.palette.len() - 1) as f64) as usize]
                    .block;
            }
        }
        Arc::new(out)
    })
}

/// Whether a rule's probe passes at a target column: what the field left at
/// the probe's height there is none of the blocks the rule refuses.
fn probe_passes(
    field: &CaveField,
    plan: &Plan<'static>,
    rule: &Projection,
    target: [i32; 3],
    probe_y: f64,
) -> bool {
    if rule.probe.is_empty() || !probe_y.is_finite() {
        return true;
    }
    let y = probe_y.floor() as i32;
    let chunk = [target[0].div_euclid(16), target[2].div_euclid(16)];
    let plane = probe_plane(field, plan.row, plan.shape, chunk, y);
    let block = plane[(target[2].rem_euclid(16) * 16 + target[0].rem_euclid(16)) as usize];
    !rule.probe.contains(&block)
}

type AnchorKey = (u32, [usize; 2], [i32; 2], usize);
/// Per column of a chunk, the owning site and its anchor height.
type ChunkAnchors = Arc<[Option<(Site, i32)>; 256]>;
/// Per column of a 16×16 chunk, the site owning the rule's anchor there and
/// its height: the lowest cell the site cuts with one of the rule's source
/// materials, found on the cut margin's lattice and refined cell by cell.
/// Every tile stacked over the chunk reads the same answer, so a mount is
/// whole across tile edges.
static ANCHORS: LazyLock<SharedMemo<AnchorKey, ChunkAnchors>> =
    LazyLock::new(|| SharedMemo::new(4096));

const STEP: i32 = 4;
/// A corner no site reaches.
const NONE: f64 = -1.0e9;

fn chunk_anchors(
    field: &CaveField,
    row: &'static Excavation,
    shape: &'static FieldShape,
    rule_index: usize,
    chunk: [i32; 2],
) -> ChunkAnchors {
    let rule = &shape.projections[rule_index];
    let key = (
        field.seed,
        field.table_identities(),
        chunk,
        rule as *const _ as usize,
    );
    ANCHORS.get_or_insert(key, || {
        let origin = [chunk[0] * 16, chunk[1] * 16];
        let heights = height_tile(field.seed, chunk);
        let sites = reaching_sites(field, row, shape, chunk);
        let mut out = [None; 256];
        if sites.is_empty() {
            return Arc::new(out);
        }
        let varying = varying_of(&sites);
        let batch = shape.margin.batch(&varying);
        let mut scan = batch.scan(field.seed);
        let range = rule.anchor.as_ref().map(|a| a.range.batch(&varying));
        let mut range_scan = range.as_ref().map(|b| b.scan(field.seed));
        let mut members: Vec<&Site> = Vec::new();
        let mut inputs = Vec::new();
        let mut bands = Vec::new();
        let mut ys = Vec::new();
        for z in 0..16 {
            for x in 0..16 {
                let (wx, wz) = (origin[0] + x, origin[1] + z);
                let surface = heights[(z * 16 + x) as usize];
                members.clear();
                members.extend(
                    sites
                        .iter()
                        .rev()
                        .filter(|site| site.bounds.contains_column(wx, wz)),
                );
                if members.is_empty() {
                    continue;
                }
                inputs.clear();
                inputs.extend(members.iter().map(|s| s.inputs([wx, 0, wz], surface)));
                if let Some(range_scan) = range_scan.as_mut() {
                    range_scan.run(inputs[0], &inputs, &[0.0]);
                }
                bands.clear();
                for (k, site) in members.iter().enumerate() {
                    let mut lo = site.bounds.min[1].max(shape.y[0]);
                    let mut hi = site.bounds.max[1]
                        .min(shape.y[1])
                        .min(surface + shape.surface_offset);
                    if let Some(range_scan) = range_scan.as_ref() {
                        let [bottom, top] = range_scan.output::<2>(k, 0);
                        if !bottom.is_finite() || !top.is_finite() {
                            bands.push((1, 0));
                            continue;
                        }
                        lo = lo.max(bottom.ceil() as i32);
                        hi = hi.min(top.floor() as i32);
                    }
                    bands.push((lo, hi));
                }
                let Some(lo_all) = bands.iter().filter(|b| b.0 <= b.1).map(|b| b.0).min() else {
                    continue;
                };
                let hi_all = bands
                    .iter()
                    .filter(|b| b.0 <= b.1)
                    .map(|b| b.1)
                    .max()
                    .expect("band");
                let base = lo_all.div_euclid(STEP) * STEP;
                ys.clear();
                ys.extend((base..=hi_all + STEP).step_by(STEP as usize).map(f64::from));
                scan.run(inputs[0], &inputs, &ys);
                let column = (z * 16 + x) as usize;
                'members: for (k, site) in members.iter().enumerate() {
                    let (lo, hi) = bands[k];
                    if lo > hi {
                        continue;
                    }
                    let margin_at = |y: i32, scan: &BatchScan<'_, '_>| {
                        let lane = ((y - base).div_euclid(STEP)) as usize;
                        let t = f64::from((y - base).rem_euclid(STEP)) / f64::from(STEP);
                        let [a] = scan.output::<1>(k, lane);
                        let [b] = scan.output::<1>(k, lane + 1);
                        a + (b - a) * t
                    };
                    // The lowest lattice bracket where the margin turns
                    // positive, then the exact cell within it.
                    let mut y = lo;
                    while y <= hi && margin_at(y, &scan) <= 0.0 {
                        let next = (y.div_euclid(STEP) + 1) * STEP;
                        let [above] = scan.output::<1>(k, ((next - base) / STEP) as usize);
                        y = if above > 0.0 { y + 1 } else { next };
                    }
                    let mut material = None;
                    for candidate in y..=hi.min(y + 31) {
                        if margin_at(candidate, &scan) <= 0.0 {
                            break;
                        }
                        let evaluation = material.get_or_insert_with(|| {
                            shape
                                .material
                                .column_seeded(field.seed, site.inputs([wx, 0, wz], surface))
                        });
                        let [index] = evaluation.at::<1>(f64::from(candidate));
                        let block = shape.palette
                            [index.max(0.0).min((shape.palette.len() - 1) as f64) as usize]
                            .block;
                        if rule.from.contains(&block) {
                            out[column] = Some((**site, candidate));
                            break 'members;
                        }
                    }
                }
            }
        }
        Arc::new(out)
    })
}

pub(super) fn apply(
    draft: &mut Draft,
    plans: &[Plan<'static>],
    surfaces: &[i32],
    field: &CaveField,
) {
    let mut writes = Vec::new();
    // The anchor of every (owner, rule, column) the chunk memo names an
    // owner for that this tile knows as a plan.
    let mut anchors: HashMap<(usize, usize, usize), i32> = HashMap::new();
    let mut chunks: HashMap<([i32; 2], usize), ChunkAnchors> = HashMap::new();
    let mut probe_scan: HashMap<usize, Scan<'_>> = HashMap::new();
    let mut probe_out: Vec<[f64; 1]> = Vec::new();
    for column in 0..surfaces.len() {
        let wx = draft.lo[0] + (column % draft.side[0]) as i32;
        let wz = draft.lo[2] + (column / draft.side[0]) as i32;
        let chunk = [wx.div_euclid(16), wz.div_euclid(16)];
        let cell = (wz.rem_euclid(16) * 16 + wx.rem_euclid(16)) as usize;
        for group in plans.chunk_by(|a, b| std::ptr::eq(a.shape, b.shape)) {
            let shape = group[0].shape;
            for rule_index in 0..shape.projections.len() {
                let rule = &shape.projections[rule_index];
                let anchors_of_chunk = chunks
                    .entry((chunk, rule as *const _ as usize))
                    .or_insert_with(|| {
                        chunk_anchors(field, group[0].row, shape, rule_index, chunk)
                    });
                let Some((site, anchor)) = anchors_of_chunk[cell] else {
                    continue;
                };
                let Some(owner) = plans
                    .iter()
                    .position(|p| std::ptr::eq(p.shape, shape) && p.site.center == site.center)
                else {
                    continue;
                };
                anchors.insert((owner, rule_index, column), anchor);
            }
        }
    }

    let mut scans: HashMap<usize, RuleScans<'_>> = HashMap::new();
    let mut material_out = Vec::new();
    let mut preserve_out: Vec<Vec<[f64; 1]>> = Vec::new();
    let mut admission_out = Vec::new();
    let mut ys = Vec::new();
    for (&(owner, rule_index, column), &anchor) in &anchors {
        let plan = &plans[owner];
        let rule = &plan.shape.projections[rule_index];
        let surface = surfaces[column];
        let x = draft.lo[0] + (column % draft.side[0]) as i32;
        let z = draft.lo[2] + (column / draft.side[0]) as i32;
        let key = rule as *const _ as usize;
        let scans = scans.entry(key).or_insert_with(|| RuleScans {
            admission: rule.admission.scan(field.seed),
            material: rule.material.scan(field.seed),
            preserve: rule
                .preserve
                .iter()
                .map(|p| p.when.scan(field.seed))
                .collect(),
        });
        for &offset in &rule.offsets {
            let target = shift([x, anchor, z], offset);
            let Some(column_of_target) = draft.index([target[0], draft.lo[1], target[2]]) else {
                continue;
            };
            let target_surface = surfaces[column_of_target];
            let probe = probe_scan
                .entry(key)
                .or_insert_with(|| rule.probe_y.scan(field.seed));
            probe.run(plan.inputs(target, surface), &[0.0], &mut probe_out);
            if !probe_passes(field, plan, rule, target, probe_out[0][0]) {
                continue;
            }
            // The anchor column's surface, as the rule's author measured
            // depth from; the target's own only tells the terrain guess.
            let inputs = plan.inputs(target, surface);
            scans
                .admission
                .run(inputs, &[f64::from(target[1])], &mut admission_out);
            let [when, lo, hi] = admission_out[0];
            if !when.is_finite() || when <= 0.0 || !lo.is_finite() || !hi.is_finite() {
                continue;
            }
            let lo = (lo.ceil() as i32).max(draft.lo[1]).max(WORLD_MIN_Y + 1);
            let hi = (hi.floor() as i32)
                .min(draft.lo[1] + draft.side[1] as i32 - 1)
                .min(WORLD_MAX_Y - 1);
            if lo > hi {
                continue;
            }
            ys.clear();
            ys.extend((lo..=hi).map(f64::from));
            scans.material.run(inputs, &ys, &mut material_out);
            preserve_out.resize_with(rule.preserve.len(), Vec::new);
            for (scan, out) in scans.preserve.iter_mut().zip(preserve_out.iter_mut()) {
                scan.run(inputs, &ys, out);
            }
            for (lane, y) in (lo..=hi).enumerate() {
                let pos = [target[0], y, target[2]];
                let [index] = material_out[lane];
                if !index.is_finite() || index < 0.0 {
                    continue;
                }
                let prior = draft.read(pos, &mut |p: [i32; 3]| terrain_guess(p[1], target_surface));
                if rule.avoid.contains(&prior)
                    || rule
                        .preserve
                        .iter()
                        .zip(&preserve_out)
                        .any(|(p, out)| p.from.contains(&prior) && out[lane][0] > 0.0)
                {
                    continue;
                }
                let fill = rule.palette[(index as usize).min(rule.palette.len() - 1)];
                if !fill.replace.accepts(prior) {
                    continue;
                }
                let block = rule
                    .remap
                    .iter()
                    .find(|p| p[0] == prior)
                    .map_or(fill.block, |p| p[1]);
                writes.push((owner, rule_index, pos, block, plan.biome));
            }
        }
    }
    // Writes in a fixed order — later placements and later rules win — so
    // overlapping mounts settle the same way whichever tile asks.
    writes.sort_by_key(|&(owner, rule, pos, _, _)| (owner, rule, pos));
    for (owner, _, pos, block, biome) in writes {
        draft.write(pos, block, biome, owner);
    }
}
