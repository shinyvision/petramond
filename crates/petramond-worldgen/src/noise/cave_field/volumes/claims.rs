//! Excavations may claim a habitat column without requiring its block tiles.

use std::sync::{Arc, LazyLock};

use super::{site, CaveField, FieldShape, Site};
use crate::{data::underground::IdSet, memo::SharedMemo};

#[derive(Clone, Copy)]
struct Claim {
    biome: u8,
    y: [i32; 2],
}
type Key = (u32, [usize; 2], [i32; 2]);
type Claims = Arc<[Claim]>;
static COLUMNS: LazyLock<SharedMemo<Key, Claims>> = LazyLock::new(|| SharedMemo::new(8192));

#[derive(Default)]
pub(in crate::noise::cave_field) struct Columns {
    origin: [i32; 2],
    nx: usize,
    columns: Vec<Claims>,
}

impl Columns {
    pub(in crate::noise::cave_field) fn gather(
        field: &CaveField,
        lo: [i32; 2],
        hi: [i32; 2],
    ) -> Self {
        if !field
            .excavations
            .rows
            .iter()
            .any(|r| r.field().is_some_and(|f| f.biome_fill.is_some()))
        {
            return Self::default();
        }
        let origin = lo.map(|v| v.div_euclid(16));
        let end = hi.map(|v| v.div_euclid(16));
        let nx = (end[0] - origin[0] + 1) as usize;
        let columns = (origin[1]..=end[1])
            .flat_map(|z| (origin[0]..=end[0]).map(move |x| column(field, [x, z])))
            .collect();
        Self {
            origin,
            nx,
            columns,
        }
    }

    pub(in crate::noise::cave_field) fn at(&self, [x, y, z]: [i32; 3]) -> Option<u8> {
        if self.columns.is_empty() {
            return None;
        }
        let dx = x.div_euclid(16) - self.origin[0];
        let dz = z.div_euclid(16) - self.origin[1];
        self.columns[dz as usize * self.nx + dx as usize]
            .iter()
            .rev()
            .find(|claim| (claim.y[0]..=claim.y[1]).contains(&y))
            .map(|claim| claim.biome)
    }
}

pub(in crate::noise::cave_field) fn include(
    field: &CaveField,
    lo: [i32; 3],
    hi: [i32; 3],
    ids: &mut IdSet,
) {
    if !field.excavations.rows.iter().any(|r| {
        r.field().is_some_and(|f| {
            f.biome_fill
                .as_ref()
                .is_some_and(|f| lo[1] <= f.y[1] && hi[1] >= f.y[0])
        })
    }) {
        return;
    }
    for z in lo[2].div_euclid(16)..=hi[2].div_euclid(16) {
        for x in lo[0].div_euclid(16)..=hi[0].div_euclid(16) {
            for claim in column(field, [x, z]).iter() {
                if lo[1] <= claim.y[1] && hi[1] >= claim.y[0] {
                    ids.insert(claim.biome);
                }
            }
        }
    }
}

fn column(field: &CaveField, pos: [i32; 2]) -> Claims {
    COLUMNS.get_or_insert((field.seed, field.table_identities(), pos), || {
        let origin = pos.map(|v| v * 16);
        let mut out = Vec::new();
        for row in &field.excavations.rows {
            let Some(shape) = row.field() else {
                continue;
            };
            let Some(fill) = &shape.biome_fill else {
                continue;
            };
            let Some(biome) = row.placement.underground_biome else {
                continue;
            };
            let reach = shape.bound_radius;
            let spacing = row.placement.spacing;
            let mut sites = Vec::new();
            for z in (origin[1] - reach).div_euclid(spacing)
                ..=(origin[1] + 15 + reach).div_euclid(spacing)
            {
                for x in (origin[0] - reach).div_euclid(spacing)
                    ..=(origin[0] + 15 + reach).div_euclid(spacing)
                {
                    if let Some(site) = site(field, row, shape, [x, z]) {
                        sites.push(site);
                    }
                }
            }
            if !sites.is_empty() && reaches(field, shape, &fill.margin, &sites, origin) {
                out.push(Claim { biome, y: fill.y });
            }
        }
        out.into()
    })
}

/// Whether any of `sites` admits any cell of the 16×16 column at `origin`
/// within its bounds: the union of the members' margins, sampled on the
/// same lattice the tiles carve on, is positive somewhere in the chunk.
fn reaches(
    field: &CaveField,
    shape: &FieldShape,
    formula: &crate::formula::Formula,
    sites: &[Site],
    origin: [i32; 2],
) -> bool {
    const STEP: i32 = 4;
    let sites: Vec<&Site> = sites
        .iter()
        .filter(|site| {
            site.bounds.intersects(
                [origin[0], shape.y[0], origin[1]],
                [origin[0] + 15, shape.y[1], origin[1] + 15],
            )
        })
        .collect();
    if sites.is_empty() {
        return false;
    }
    let heights = crate::terrain_query::height_tile(field.seed, origin.map(|v| v.div_euclid(16)));
    let first = sites[0].inputs([0; 3], 0);
    let varying: Vec<usize> = (3..=7)
        .filter(|&i| {
            sites[1..]
                .iter()
                .any(|s| s.inputs([0; 3], 0).0[i] != first.0[i])
        })
        .collect();
    let batch = formula.batch(&varying);
    let mut scan = batch.scan(field.seed);
    let mut inputs = Vec::new();
    let mut bands = Vec::new();
    let mut ys = Vec::new();
    for z in (0..=16).step_by(STEP as usize) {
        for x in (0..=16).step_by(STEP as usize) {
            let wx = origin[0] + x;
            let wz = origin[1] + z;
            let surface = heights[((z.min(15)) * 16 + x.min(15)) as usize];
            inputs.clear();
            bands.clear();
            for site in &sites {
                let b = site.bounds;
                if !(b.min[0] - STEP..=b.max[0] + STEP).contains(&wx)
                    || !(b.min[2] - STEP..=b.max[2] + STEP).contains(&wz)
                {
                    continue;
                }
                let top = b.max[1].min(surface + shape.surface_offset);
                let bottom = b.min[1];
                if top < bottom {
                    continue;
                }
                inputs.push(site.inputs([wx, 0, wz], surface));
                bands.push((bottom - STEP, top + STEP));
            }
            if inputs.is_empty() {
                continue;
            }
            let lo = bands
                .iter()
                .map(|b| b.0)
                .min()
                .expect("member")
                .div_euclid(STEP)
                * STEP;
            let hi = bands.iter().map(|b| b.1).max().expect("member");
            ys.clear();
            ys.extend((lo..=hi).step_by(STEP as usize).map(f64::from));
            scan.run(inputs[0], &inputs, &ys);
            for (m, &(bottom, top)) in bands.iter().enumerate() {
                for (lane, &y) in ys.iter().enumerate() {
                    let y = y as i32;
                    if y < bottom || y > top {
                        continue;
                    }
                    let [value] = scan.output::<1>(m, lane);
                    if value.is_finite() && value > 0.0 {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests;
