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

/// The placed silt of a sealed chain, and the columns dammed at a waterline.
pub(super) struct Seal {
    pub(super) silt: BTreeSet<[i32; 3]>,
    pub(super) rim_dam: BTreeSet<Col>,
}

/// Seal the chain, retreating the water wherever the rim cannot hold, until
/// one pass needs no retreat and leaves no gutted basin behind.
pub(super) fn seal(
    basins: &mut Basins,
    terrain: &impl Fn([i32; 3]) -> Option<bool>,
) -> Result<Seal, &'static str> {
    let sealed = loop {
        let (sealed, retreat) = seal_pass(basins, terrain);
        if retreat.is_empty() {
            // A basin the retreat gutted is gone whole; dropping one changes
            // the geometry, so the seal runs once more over what is left
            // rather than leaving its beds and dams behind.
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

/// One seal over the chain as it stands: beds first, so foundations can rest
/// on them, then a dam against every open side. Returns the seal and the wet
/// columns that must retreat for it to hold.
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

/// Carry a dam placed at `top` down to something solid — rock, earlier silt,
/// or another basin's water. `false` when that takes more than `DAM_MAX`
/// courses or runs past the probed terrain.
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

/// The anti-bathtub gate, measured on what actually stands: weir faces
/// between linked basins are legitimately tall, but they are a small share
/// of the rim, and half the rim running tall is a tank, not a terrace.
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
