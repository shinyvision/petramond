use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::basin::{Basins, Col};
use super::{DAM_MAX, NOTCH_PATH_MAX, SIDES};

pub(super) fn cut_notches(
    basins: &Basins,
    silt: &mut BTreeSet<[i32; 3]>,
    terrain: &impl Fn([i32; 3]) -> Option<bool>,
) -> Result<BTreeSet<[i32; 3]>, &'static str> {
    let count = basins.counts();
    let mut exempt: BTreeSet<[i32; 3]> = BTreeSet::new();
    let mut cuts: BTreeSet<[i32; 3]> = BTreeSet::new();
    for j in 1..basins.pools.len() {
        if count[j] == 0 {
            continue;
        }
        let i = basins.from[j];
        if count[i] == 0 {
            return Err("a basin lost the terrace it spills from");
        }
        let path = notch_path(&basins.cols, j, i).ok_or("no spill path over the lip")?;
        let s = basins.pools[i];
        let sj = basins.pools[j];
        for (k, &(x, z)) in path.iter().enumerate() {
            let last = k + 1 == path.len();
            let lo = if last { sj + 1 } else { s };
            for y in lo..=s {
                let p = [x, y, z];
                exempt.insert(p);
                silt.remove(&p);
                if terrain(p) == Some(true) {
                    cuts.insert(p);
                }
            }
            if last {
                wall_chute(basins, silt, terrain, &exempt, (x, z), j, lo..=s)?;
            } else {
                flank_channel(basins, silt, terrain, &exempt, (x, z), s)?;
            }
        }
    }
    Ok(cuts)
}

fn wall_chute(
    basins: &Basins,
    silt: &mut BTreeSet<[i32; 3]>,
    terrain: &impl Fn([i32; 3]) -> Option<bool>,
    exempt: &BTreeSet<[i32; 3]>,
    (x, z): Col,
    j: usize,
    rows: std::ops::RangeInclusive<i32>,
) -> Result<(), &'static str> {
    for y in rows {
        for (dx, dz) in SIDES {
            let np = [x + dx, y, z + dz];
            if exempt.contains(&np)
                || basins.pool_of((np[0], np[2])) == Some(j)
                || basins.wet_at(np[0], np[2], y)
            {
                continue;
            }
            flank(basins, silt, terrain, np)?;
        }
    }
    Ok(())
}

fn flank_channel(
    basins: &Basins,
    silt: &mut BTreeSet<[i32; 3]>,
    terrain: &impl Fn([i32; 3]) -> Option<bool>,
    exempt: &BTreeSet<[i32; 3]>,
    (x, z): Col,
    s: i32,
) -> Result<(), &'static str> {
    let below = [x, s - 1, z];
    let grounded = silt.contains(&below) || terrain(below) == Some(true);
    if !grounded {
        return Ok(());
    }
    for (dx, dz) in SIDES {
        let np = [x + dx, s, z + dz];
        if exempt.contains(&np) || basins.wet_at(np[0], np[2], s) {
            continue;
        }
        flank(basins, silt, terrain, np)?;
    }
    Ok(())
}

fn flank(
    basins: &Basins,
    silt: &mut BTreeSet<[i32; 3]>,
    terrain: &impl Fn([i32; 3]) -> Option<bool>,
    np: [i32; 3],
) -> Result<(), &'static str> {
    if terrain(np) == Some(false) && silt.insert(np) {
        let mut fy = np[1] - 1;
        let mut height = 1;
        while height <= DAM_MAX
            && terrain([np[0], fy, np[2]]) == Some(false)
            && !basins.wet_at(np[0], np[2], fy)
            && silt.insert([np[0], fy, np[2]])
        {
            height += 1;
            fy -= 1;
        }
        if height > DAM_MAX || terrain([np[0], fy, np[2]]).is_none() {
            return Err("a spill flank over a chasm");
        }
    }
    Ok(())
}

/// Path from basin `j`'s columns to basin `i`'s, unowned columns only.
/// Last entry is the plunge column in `j`.
/// BFS layers go in sorted order, so we always get the same path.
pub(super) fn notch_path(
    cols: &BTreeMap<Col, (usize, i32)>,
    j: usize,
    i: usize,
) -> Option<Vec<Col>> {
    let mut parent: BTreeMap<Col, Col> = BTreeMap::new();
    let mut frontier: VecDeque<Col> = cols
        .iter()
        .filter(|&(_, &(pi, _))| pi == j)
        .map(|(&c, _)| c)
        .collect();
    for &c in &frontier {
        parent.insert(c, c);
    }
    for _ in 0..=NOTCH_PATH_MAX {
        let mut next = VecDeque::new();
        let mut hits: Vec<(Col, Col)> = Vec::new();
        while let Some(c) = frontier.pop_front() {
            for (dx, dz) in SIDES {
                let n = (c.0 + dx, c.1 + dz);
                match cols.get(&n) {
                    Some(&(pi, _)) if pi == i => hits.push((n, c)),
                    Some(_) => {}
                    None => {
                        if let std::collections::btree_map::Entry::Vacant(e) = parent.entry(n) {
                            e.insert(c);
                            next.push_back(n);
                        }
                    }
                }
            }
        }
        if let Some(&(_, via)) = hits.iter().min() {
            let mut path = vec![via];
            let mut c = via;
            while parent[&c] != c {
                c = parent[&c];
                path.push(c);
            }
            return Some(path);
        }
        frontier = next;
    }
    None
}
