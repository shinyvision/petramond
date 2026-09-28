use std::collections::VecDeque;

use crate::{
    BlockId, ClaimsCtx, FxHashMap, GenClaims, GenCtx, GenOutput, GenRng, MemoClaim, SectionBox,
    SectionOutput,
};

use super::plan::{decode_mask, encode_mask, IndexedPlan, Plan};
use super::rng::Draw;

/// The section must be generated again later: another worker holds the lease on a plan it
/// needs and has not published it yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deferred;

/// A lattice of candidate structure sites: at most one per `spacing` × `spacing` cell, present
/// with probability `chance`, its centre kept `border` blocks inside the cell. `reach` bounds
/// how far from its centre a site's structure may write.
#[derive(Clone, Copy, Debug)]
pub struct SiteGrid {
    pub salt: u64,
    pub spacing: i32,
    pub border: i32,
    pub chance: f32,
    pub reach: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Site {
    pub cell: [i32; 2],
    pub center: [i32; 2],
}

impl SiteGrid {
    pub fn site(&self, seed: u32, cell: [i32; 2]) -> Option<Site> {
        let mut rng = GenRng::positional(seed, self.salt, cell[0], 0, cell[1]);
        if !rng.roll(self.chance) {
            return None;
        }
        let room = (self.spacing - 2 * self.border).max(1);
        Some(Site {
            cell,
            center: [
                cell[0] * self.spacing + self.border + rng.int(0, room - 1),
                cell[1] * self.spacing + self.border + rng.int(0, room - 1),
            ],
        })
    }

    /// Sites whose reach overlaps the column rectangle `lo..=hi`.
    pub fn sites_near(&self, seed: u32, lo: [i32; 2], hi: [i32; 2]) -> Vec<Site> {
        let cell = |v: i32| v.div_euclid(self.spacing);
        let mut out = Vec::new();
        for cz in cell(lo[1] - self.reach)..=cell(hi[1] + self.reach) {
            for cx in cell(lo[0] - self.reach)..=cell(hi[0] + self.reach) {
                let Some(site) = self.site(seed, [cx, cz]) else {
                    continue;
                };
                let [x, z] = site.center;
                if x + self.reach >= lo[0]
                    && x - self.reach <= hi[0]
                    && z + self.reach >= lo[1]
                    && z - self.reach <= hi[1]
                {
                    out.push(site);
                }
            }
        }
        out
    }

    /// The sections a site's structure may write in.
    pub fn reach_box(&self, site: Site) -> SectionBox {
        let [x, z] = site.center;
        let section = |v: i32| v.div_euclid(16);
        SectionBox {
            min: [section(x - self.reach), i32::MIN, section(z - self.reach)],
            max: [section(x + self.reach), i32::MAX, section(z + self.reach)],
        }
    }

    /// A box of sections around `section`, inside its column's lattice cell, that no site
    /// reaches but those `idle` says build nothing; `None` when such a site reaches `section`.
    pub fn quiet_box(
        &self,
        seed: u32,
        section: [i32; 3],
        idle: impl Fn(Site) -> bool,
    ) -> Option<SectionBox> {
        let cell = |s: i32| (s * 16).div_euclid(self.spacing);
        let first = |c: i32| (c * self.spacing).div_euclid(16);
        let last = |c: i32| ((c + 1) * self.spacing - 1).div_euclid(16);
        let [cx, cz] = [cell(section[0]), cell(section[2])];
        let mut quiet = SectionBox {
            min: [first(cx), i32::MIN, first(cz)],
            max: [last(cx), i32::MAX, last(cz)],
        };
        let lo = [quiet.min[0] * 16, quiet.min[2] * 16];
        let hi = [quiet.max[0] * 16 + 15, quiet.max[2] * 16 + 15];
        for site in self.sites_near(seed, lo, hi) {
            let reach = self.reach_box(site);
            let overlaps = [0, 2]
                .iter()
                .all(|&a| reach.min[a] <= quiet.max[a] && quiet.min[a] <= reach.max[a]);
            if !overlaps || idle(site) {
                continue;
            }
            let mut best: Option<SectionBox> = None;
            for a in [0, 2] {
                let mut cut = quiet;
                if section[a] < reach.min[a] {
                    cut.max[a] = reach.min[a] - 1;
                } else if section[a] > reach.max[a] {
                    cut.min[a] = reach.max[a] + 1;
                } else {
                    continue;
                }
                let area = |b: &SectionBox| {
                    (b.max[0] - b.min[0] + 1) as i64 * (b.max[2] - b.min[2] + 1) as i64
                };
                if best.as_ref().is_none_or(|b| area(&cut) > area(b)) {
                    best = Some(cut);
                }
            }
            quiet = best?;
        }
        Some(quiet)
    }

    /// The stream a site's structure is derived from; independent of the placement rolls.
    pub fn rng(&self, seed: u32, site: Site) -> GenRng {
        GenRng::positional(seed, self.salt ^ 0x5173_d3a7, site.cell[0], 1, site.cell[1])
    }
}

/// What deriving a site's structure came to.
pub enum Derived {
    Plan(Box<Plan>),
    /// The site builds nothing (water, a cliff, the wrong biome): published like a plan.
    Nothing,
    /// A host read answered short. Never published: a failed read is not a fact about the site.
    Unavailable,
}

/// Plans settled per site on this instance, in front of the process-wide memo: the first
/// worker to reach a site derives its plan and publishes it, every other worker reads it.
pub struct PlanCache {
    prefix: Vec<u8>,
    capacity: usize,
    entries: FxHashMap<[i32; 2], Entry>,
    order: VecDeque<[i32; 2]>,
}

struct Entry {
    plan: Option<IndexedPlan>,
    /// The sections the plan leaves untouched were declared to the host.
    declared: bool,
    /// This worker derived the plan and has not handed its sections over to the host yet.
    owed: bool,
}

impl PlanCache {
    /// `name` separates this structure's memo keys from the mod's others.
    pub fn new(name: &str, capacity: usize) -> PlanCache {
        let mut prefix = b"build/".to_vec();
        prefix.extend_from_slice(name.as_bytes());
        prefix.push(0);
        PlanCache {
            prefix,
            capacity: capacity.max(1),
            entries: FxHashMap::default(),
            order: VecDeque::new(),
        }
    }

    /// The site's plan (`None`: nothing to build here, or it could not be derived right now),
    /// deriving it with `derive` when no worker has published it yet.
    pub fn get(
        &mut self,
        site: Site,
        derive: impl FnOnce() -> Derived,
    ) -> Result<Option<&mut IndexedPlan>, Deferred> {
        if !self.entries.contains_key(&site.cell) {
            let Some((plan, derived)) = self.settle(site, derive)? else {
                return Ok(None);
            };
            if self.order.len() == self.capacity {
                if let Some(old) = self.order.pop_front() {
                    self.entries.remove(&old);
                }
            }
            self.order.push_back(site.cell);
            self.entries.insert(
                site.cell,
                Entry {
                    owed: derived && plan.is_some(),
                    plan,
                    declared: false,
                },
            );
        }
        Ok(self
            .entries
            .get_mut(&site.cell)
            .and_then(|e| e.plan.as_mut()))
    }

    /// The sections of `site`'s reach no other site reaches, around `section`.
    fn reach_alone(
        &self,
        grid: &SiteGrid,
        seed: u32,
        section: [i32; 3],
        site: Site,
    ) -> Option<SectionBox> {
        let quiet = grid.quiet_box(seed, section, |s| s == site || self.builds_nothing(s))?;
        let reach = grid.reach_box(site);
        Some(SectionBox {
            min: [0, 1, 2].map(|a| reach.min[a].max(quiet.min[a])),
            max: [0, 1, 2].map(|a| reach.max[a].min(quiet.max[a])),
        })
    }

    /// Whether `site` is settled as building nothing.
    pub fn builds_nothing(&self, site: Site) -> bool {
        self.entries
            .get(&site.cell)
            .is_some_and(|e| e.plan.is_none())
    }

    /// Whether no site but `site` may write in `section`.
    fn alone_in(&self, grid: &SiteGrid, seed: u32, section: [i32; 3], site: Site) -> bool {
        let lo = [section[0] * 16, section[2] * 16];
        grid.sites_near(seed, lo, [lo[0] + 15, lo[1] + 15])
            .into_iter()
            .all(|s| s == site || self.builds_nothing(s))
    }

    /// The site's sections, once, from the worker that derived its plan, as outputs the host
    /// applies without dispatching there: every section but `current` no other site may write
    /// in, the first declaring the sections of the reach the plan leaves untouched.
    fn hand_over(
        &mut self,
        grid: &SiteGrid,
        seed: u32,
        site: Site,
        current: Option<[i32; 3]>,
        resolve: &mut impl FnMut(&str) -> Option<BlockId>,
    ) -> Vec<SectionOutput> {
        let Some(entry) = self.entries.get(&site.cell).filter(|e| e.owed) else {
            return Vec::new();
        };
        let alone: Vec<[i32; 3]> = entry.plan.as_ref().map_or_else(Vec::new, |plan| {
            plan.sections()
                .filter(|&s| Some(s) != current && self.alone_in(grid, seed, s, site))
                .collect()
        });
        let centre = [site.center[0] >> 4, 0, site.center[1] >> 4];
        let within = (!entry.declared)
            .then(|| self.reach_alone(grid, seed, centre, site))
            .flatten();
        let Some(entry) = self.entries.get_mut(&site.cell) else {
            return Vec::new();
        };
        entry.owed = false;
        let Some(plan) = entry.plan.as_mut() else {
            return Vec::new();
        };
        let mut outputs: Vec<SectionOutput> = alone
            .into_iter()
            .map(|section| {
                let mut output = GenOutput::default();
                plan.emit(section, resolve, &mut output);
                SectionOutput { section, output }
            })
            .collect();
        if let (Some(within), Some(first)) = (within, outputs.first_mut()) {
            first.output.nothing_in = plan.untouched(within);
            entry.declared = true;
        }
        outputs
    }

    /// Everything the sites on `grid` build into the section `ctx` generates, deriving their
    /// plans with `derive` as needed, and the sections around it they build nothing in, so the
    /// host stops asking there. `resolve` maps registry names to this session's block ids.
    pub fn generate(
        &mut self,
        grid: &SiteGrid,
        ctx: &GenCtx,
        mut derive: impl FnMut(Site) -> Derived,
        resolve: &mut impl FnMut(&str) -> Option<BlockId>,
    ) -> Result<GenOutput, Deferred> {
        let (seed, section) = (ctx.seed(), ctx.section_pos());
        let o = ctx.origin_world();
        let mut out = GenOutput::default();
        let mut building = Vec::new();
        for site in grid.sites_near(seed, [o[0], o[2]], [o[0] + 15, o[2] + 15]) {
            if self.get(site, || derive(site))?.is_some() {
                building.push(site);
            }
        }
        for site in &building {
            if let Some(plan) = self
                .entries
                .get_mut(&site.cell)
                .and_then(|e| e.plan.as_mut())
            {
                plan.emit(section, resolve, &mut out);
            }
        }
        for &site in &building {
            let ahead = self.hand_over(grid, seed, site, Some(section), resolve);
            out.ahead.extend(ahead);
        }
        match building[..] {
            [] => out
                .nothing_in
                .extend(grid.quiet_box(seed, section, |s| self.builds_nothing(s))),
            [site] => {
                let Some(entry) = self.entries.get(&site.cell) else {
                    return Ok(out);
                };
                if entry.declared {
                    return Ok(out);
                }
                let within = self.reach_alone(grid, seed, section, site);
                let (Some(within), Some(plan)) = (within, &self.entries[&site.cell].plan) else {
                    return Ok(out);
                };
                out.nothing_in = plan.untouched(within);
                if let Some(entry) = self.entries.get_mut(&site.cell) {
                    entry.declared = true;
                }
            }
            _ => {}
        }
        Ok(out)
    }

    /// The [claims](super::Plan::claim) of every site on `grid` whose reach touches the columns
    /// `ctx` asks about, deriving their plans with `derive` as needed, with the sections of the
    /// plans derived here handed over.
    pub fn claims(
        &mut self,
        grid: &SiteGrid,
        ctx: &ClaimsCtx,
        mut derive: impl FnMut(Site) -> Derived,
        resolve: &mut impl FnMut(&str) -> Option<BlockId>,
    ) -> Result<GenClaims, Deferred> {
        let mut out = GenClaims::default();
        let mut building = Vec::new();
        for site in grid.sites_near(ctx.seed, ctx.min, ctx.max) {
            // Another worker's plan is only needed for its claim, published on its own.
            if !self.entries.contains_key(&site.cell) {
                if let Some(mask) =
                    crate::memo_get(&self.key(site, CLAIM)).and_then(|b| decode_mask(&b))
                {
                    out.claims.push(mask);
                    continue;
                }
            }
            if let Some(plan) = self.get(site, || derive(site))? {
                out.claims.extend(plan.claim().cloned());
                building.push(site);
            }
        }
        for site in building {
            let ahead = self.hand_over(grid, ctx.seed, site, None, resolve);
            out.ahead.extend(ahead);
        }
        Ok(out)
    }

    /// The memo key of what a site publishes under `kind`.
    fn key(&self, site: Site, kind: u8) -> Vec<u8> {
        let mut key = self.prefix.clone();
        key.extend_from_slice(&site.cell[0].to_le_bytes());
        key.extend_from_slice(&site.cell[1].to_le_bytes());
        if kind != PLAN {
            key.push(kind);
        }
        key
    }

    /// `Some(entry)` to cache (a plan, or `None` for a site that builds nothing); `None` when the
    /// site could not be derived right now.
    fn settle(
        &self,
        site: Site,
        derive: impl FnOnce() -> Derived,
    ) -> Result<Option<(Option<IndexedPlan>, bool)>, Deferred> {
        let key = self.key(site, PLAN);
        let published = match crate::memo_blob_claim(&key) {
            MemoClaim::Pending => return Err(Deferred),
            MemoClaim::Value(bytes) => Some(bytes),
            MemoClaim::Lease => None,
        };
        if let Some(bytes) = published {
            match bytes.first() {
                Some(&NOTHING) => return Ok(Some((None, false))),
                Some(&PLAN) => {
                    if let Some(plan) = IndexedPlan::parse(bytes, 1) {
                        return Ok(Some((Some(plan), false)));
                    }
                }
                _ => {}
            }
        }
        match derive() {
            Derived::Plan(plan) => {
                if let Some(mask) = plan.claim_mask() {
                    crate::memo_put(&self.key(site, CLAIM), encode_mask(mask));
                }
                let mut bytes = vec![PLAN];
                bytes.extend(plan.encode());
                drop(plan);
                crate::memo_blob_put(&key, bytes.clone());
                Ok(Some((IndexedPlan::parse(bytes, 1), true)))
            }
            Derived::Nothing => {
                crate::memo_blob_put(&key, vec![NOTHING]);
                Ok(Some((None, false)))
            }
            Derived::Unavailable => Ok(None),
        }
    }
}

const NOTHING: u8 = 0;
const PLAN: u8 = 1;
/// The memo key suffix of a plan's claim mask.
const CLAIM: u8 = 2;
