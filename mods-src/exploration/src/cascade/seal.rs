//! Sealing the rim: every water cell's four sides must be water, notch, or
//! solid; where the terrain is open a silt dam — rimstone — is placed and
//! carried down to footing. A column needing more than `DAM_MAX` courses is a
//! chasm: the WATER RETREATS from that edge (the offending wet columns leave
//! their basin) and the seal runs again. Optimism lives here — rejection is
//! reserved for a chain the retreat collapses.

use std::collections::BTreeSet;

use mod_sdk::FxHashMap;

use super::basin::{Basins, Col};
use super::{DAM_MAX, DAM_TALL, DAM_TALL_SHARE_MAX, MIN_POOLS, MIN_POOL_AREA, SIDES};

pub(super) struct Seal {
    pub(super) silt: BTreeSet<[i32; 3]>,
    pub(super) rim_dam: BTreeSet<Col>,
}

pub(super) fn seal(
    basins: &mut Basins,
    terrain: &impl Fn([i32; 3]) -> Option<bool>,
) -> Result<Seal, &'static str> {
    let sealed = loop {
        let (sealed, retreat) = seal_pass(basins, terrain);
        if retreat.is_empty() {
            // Gutted basins get dropped whole, which changes the geometry - so seal again over
            // what's left, or its beds and dams stick around.
            let count = basins.counts();
            let before = basins.cols.len();
            basins
                .cols
                .retain(|_, &mut (pi, _)| count[pi] >= MIN_POOL_AREA);
            if basins.cols.len() == before {
                break sealed;
            }
            continue;
        }
        for c in retreat {
            basins.cols.remove(&c);
        }
    };
    let count = basins.counts();
    if count.iter().filter(|&&c| c > 0).count() < MIN_POOLS || count[0] == 0 {
        return Err("the seal retreat collapsed the chain");
    }
    Ok(sealed)
}

fn seal_pass(
    basins: &Basins,
    terrain: &impl Fn([i32; 3]) -> Option<bool>,
) -> (Seal, BTreeSet<Col>) {
    let mut silt: BTreeSet<[i32; 3]> = BTreeSet::new();
    let mut rim_dam: BTreeSet<Col> = BTreeSet::new();
    for (&(x, z), &(_, bed)) in &basins.cols {
        silt.insert([x, bed, z]);
    }
    let mut retreat: BTreeSet<Col> = BTreeSet::new();
    for (&(x, z), &(pi, bed)) in &basins.cols {
        let s = basins.pools[pi];
        for y in bed + 1..=s {
            for (dx, dz) in SIDES {
                let (nx, nz) = (x + dx, z + dz);
                if basins.wet_at(nx, nz, y) {
                    continue;
                }
                let np = [nx, y, nz];
                match terrain(np) {
                    Some(true) => continue,
                    None => {
                        retreat.insert((x, z));
                        continue;
                    }
                    Some(false) => {}
                }
                if y == s {
                    rim_dam.insert((nx, nz));
                }
                if silt.insert(np) && !footing(basins, terrain, &mut silt, np) {
                    retreat.insert((x, z));
                }
            }
        }
    }
    (Seal { silt, rim_dam }, retreat)
}

fn footing(
    basins: &Basins,
    terrain: &impl Fn([i32; 3]) -> Option<bool>,
    silt: &mut BTreeSet<[i32; 3]>,
    top: [i32; 3],
) -> bool {
    let [nx, y, nz] = top;
    let mut fy = y - 1;
    let mut height = 1;
    loop {
        if height > DAM_MAX {
            return false;
        }
        match terrain([nx, fy, nz]) {
            Some(true) => return true,
            None => return false,
            Some(false) => {
                if basins.wet_at(nx, nz, fy) || !silt.insert([nx, fy, nz]) {
                    return true;
                }
                height += 1;
                fy -= 1;
            }
        }
    }
}

pub(super) fn rim_is_a_wall(silt: &BTreeSet<[i32; 3]>, rim_dam: &BTreeSet<Col>) -> bool {
    let mut height: FxHashMap<Col, i32> = FxHashMap::default();
    for p in silt {
        if rim_dam.contains(&(p[0], p[2])) {
            *height.entry((p[0], p[2])).or_insert(0) += 1;
        }
    }
    let tall = height.values().filter(|&&h| h >= DAM_TALL).count();
    tall * 100 > height.len().max(1) * DAM_TALL_SHARE_MAX
}
