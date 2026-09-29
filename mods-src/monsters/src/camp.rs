//! Skeleton camps: round-ish, walled camps of wood or stone, tattered by design, scattered over
//! the surface. A site's camp is derived once into a build plan from positional reads only and
//! clipped into every section it reaches, so sections may generate in any order.

/// `petramond:<block>` as a material, built once per call site: builders use these in their
/// innermost loops.
macro_rules! named {
    ($block:literal) => {{
        static MATERIAL: std::sync::LazyLock<mod_sdk::build::Material> =
            std::sync::LazyLock::new(|| {
                mod_sdk::build::Material::named(concat!("petramond:", $block))
            });
        *MATERIAL
    }};
}

/// [`named!`] for a block that holds nothing up (a torch, ladder, plant or snow layer).
macro_rules! decor {
    ($block:literal) => {{
        static MATERIAL: std::sync::LazyLock<mod_sdk::build::Material> =
            std::sync::LazyLock::new(|| {
                mod_sdk::build::Material::decor(concat!("petramond:", $block))
            });
        *MATERIAL
    }};
}

mod centre;
mod gates;
mod grid;
mod ground;
mod huts;
mod layout;
mod plateaus;
mod posts;
mod style;
mod towers;
mod walls;

#[cfg(test)]
mod tests;

use mod_sdk::build::{Derived, Families, Heights, Plan, PlanCache, RingField, Site, SiteGrid};
use mod_sdk::*;

use grid::Grid;
use style::Mats;

/// Sections a camp can write: the arena pit and well shafts reach below the surface, towers on
/// plateaus above it.
pub(crate) const GEN_FILTER: GenFeatureFilter = GenFeatureFilter::surface_band(-16, 44)
    .without_blocks()
    .without_columns()
    .with_claims();

/// How far from its centre a camp may write: the outline's largest radius plus a plateau
/// straddling it and the cleared margin.
const REACH: i32 = 48;

const GRID: SiteGrid = SiteGrid {
    salt: GenRng::salt("monsters:skeleton_camp"),
    spacing: 288,
    border: REACH,
    chance: 0.65,
    reach: REACH,
};

/// The positional reads a camp is derived from.
pub(crate) trait Terrain {
    fn heights(&self, min: [i32; 2], max: [i32; 2]) -> Option<Heights>;
    fn biomes(&self, columns: Vec<[i32; 2]>) -> Option<Vec<u8>>;
    fn fluid(&self, positions: Vec<[i32; 3]>) -> Option<Vec<bool>>;
}

struct HostTerrain;

impl Terrain for HostTerrain {
    fn heights(&self, min: [i32; 2], max: [i32; 2]) -> Option<Heights> {
        Heights::load(min, max)
    }

    fn biomes(&self, columns: Vec<[i32; 2]>) -> Option<Vec<u8>> {
        mod_sdk::build::surface_biomes(columns)
    }

    fn fluid(&self, positions: Vec<[i32; 3]>) -> Option<Vec<bool>> {
        let want = positions.len();
        let spaces = paged(positions, terrain_space_at);
        (spaces.len() == want).then(|| spaces.iter().map(|s| *s == TerrainSpace::Fluid).collect())
    }
}

pub(crate) struct Camps {
    families: Families,
    cache: PlanCache,
    ids: FxHashMap<String, Option<BlockId>>,
}

impl Camps {
    pub(crate) fn new() -> Option<Camps> {
        let families = Families::load();
        if let Err(missing) = style::check(&families) {
            log(&format!("skeleton camps disabled: {missing}"));
            return None;
        }
        Some(Camps {
            families,
            cache: PlanCache::new("skeleton_camp", 16),
            ids: FxHashMap::default(),
        })
    }

    pub(crate) fn generate(&mut self, ctx: &GenCtx) -> GenOutput {
        let derive = derive_logged(ctx.seed(), ctx.sea_level(), &self.families);
        self.cache
            .generate(&GRID, ctx, derive, &mut resolver(&mut self.ids))
            .unwrap_or_else(|_| GenOutput::deferred())
    }

    pub(crate) fn claims(&mut self, ctx: &ClaimsCtx) -> GenClaims {
        let derive = derive_logged(ctx.seed, ctx.sea_level, &self.families);
        self.cache
            .claims(&GRID, ctx, derive, &mut resolver(&mut self.ids))
            .unwrap_or_else(|_| GenClaims {
                deferred: true,
                ..GenClaims::default()
            })
    }
}

/// Registry names to this session's block ids, each asked of the host once.
fn resolver(
    ids: &mut FxHashMap<String, Option<BlockId>>,
) -> impl FnMut(&str) -> Option<BlockId> + '_ {
    move |name| match ids.get(name) {
        Some(&id) => id,
        None => *ids.entry(name.to_string()).or_insert(resolve_block(name)),
    }
}

/// [`derive`] from the host's terrain, logging what each camp built.
fn derive_logged(seed: u32, sea_level: i32, families: &Families) -> impl Fn(Site) -> Derived + '_ {
    move |site| {
        let [x, z] = site.center;
        let mut report = |summary: &str| log(&format!("skeleton camp at {x} {z}: {summary}"));
        derive(seed, sea_level, site, families, &HostTerrain, &mut report)
    }
}

/// Derives the camp at `site`, telling `report` what it built.
pub(crate) fn derive(
    seed: u32,
    sea_level: i32,
    site: Site,
    families: &Families,
    terrain: &dyn Terrain,
    report: &mut dyn FnMut(&str),
) -> Derived {
    let rng = GRID.rng(seed, site);
    let mut camp = match Camp::survey(site.center, sea_level, families, terrain, rng) {
        Survey::Camp(camp) => camp,
        Survey::Nothing => return Derived::Nothing,
        Survey::Unavailable => return Derived::Unavailable,
    };
    let (plan, summary) = camp.build();
    report(&summary);
    Derived::Plan(Box::new(plan))
}

enum Survey<'a> {
    Camp(Box<Camp<'a>>),
    Nothing,
    Unavailable,
}

/// Reservation of a column during layout, so later features steer around earlier ones.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Res {
    Free,
    Wall,
    Gate,
    Tower,
    Path,
    Centre,
    Hut,
    Bridge,
    Support,
    Access,
}

/// One camp being laid out and built.
struct Camp<'a> {
    rng: GenRng,
    mats: Mats<'a>,
    plan: Plan,
    center: [i32; 2],
    radius: f32,
    harmonics: [layout::Harmonic; 4],
    wobble: mod_sdk::build::Noise2,
    natural: Heights,
    ground: Grid<i32>,
    ring: Vec<[i32; 2]>,
    field: RingField,
    /// The columns strictly inside the ring, in [`RingField::interior`] order.
    interior: Vec<[i32; 2]>,
    ring_at: Grid<i32>,
    plateau_at: Grid<u8>,
    resv: Grid<Res>,
    plateaus: Vec<layout::Plateau>,
    bridges: Vec<layout::Bridge>,
    centre: Option<layout::Centre>,
    gates: Vec<layout::Gate>,
    in_gate: Vec<bool>,
    towers: Vec<layout::Tower>,
    fort: Vec<i32>,
    fort_arcs: Vec<layout::FortArc>,
    fort_cut: Vec<i32>,
    inner_by_src: FxHashMap<usize, Vec<[i32; 2]>>,
    paths: Grid<bool>,
    /// The box every path column lies in, once there is one.
    path_bounds: Option<([i32; 2], [i32; 2])>,
    /// [`Camp::zone_box`], settled once the layout is known.
    zone: ([i32; 2], [i32; 2]),
    /// What each column of the zone box is to the camp (`ground::IN_ZONE`, ...), row by row
    /// along x.
    zone_class: Vec<u8>,
    huts: Vec<layout::Hut>,
    wall_h: Vec<i32>,
    rubble: Vec<ground::Rubble>,
    posts: Vec<posts::Post>,
}

impl Camp<'_> {
    fn ring_len(&self) -> usize {
        self.ring.len()
    }

    fn wrap(&self, i: i64) -> usize {
        i.rem_euclid(self.ring.len() as i64) as usize
    }

    fn ring_dist(&self, a: usize, b: usize) -> usize {
        let d = a.abs_diff(b) % self.ring.len();
        d.min(self.ring.len() - d)
    }

    fn inside(&self, c: [i32; 2]) -> bool {
        self.field.inside(c)
    }

    fn depth(&self, c: [i32; 2]) -> i32 {
        self.field.depth(c).unwrap_or(-1)
    }

    fn g(&self, c: [i32; 2]) -> i32 {
        self.ground.get(c)
    }

    fn put_at(&mut self, [x, z]: [i32; 2], y: i32, m: &Material) {
        self.plan.set([x, y, z], m);
    }

    fn air(&mut self, [x, z]: [i32; 2], y: i32) {
        self.plan.set([x, y, z], &Material::air());
    }

    fn reserve(&mut self, c: [i32; 2], r: Res) {
        if self.resv.contains(c) && self.resv.get(c) == Res::Free {
            self.resv.set(c, r);
        }
    }
}

use mod_sdk::build::Material;

impl Camp<'_> {
    fn build(&mut self) -> (Plan, String) {
        self.lay_out();
        self.build_ground();
        self.build_walls();
        let fort_h = self.build_fortress();
        self.build_gates(fort_h);
        self.build_towers();
        self.build_centre();
        self.build_huts();
        self.build_access();
        self.build_bridges();
        self.scatter_rubble();
        self.wear_and_decorate();
        let ground = self.ground.clone();
        let families = self.mats.families();
        self.plan.settle_slabs(families, |x, z| ground.get([x, z]));
        self.snow();
        self.build_posts(fort_h);
        self.clear_space();
        let summary = self.summary();
        (std::mem::take(&mut self.plan), summary)
    }

    fn summary(&self) -> String {
        use layout::CentreKind;
        let walls = if self.mats.stone {
            "stone".to_string()
        } else {
            format!("{} wood", self.mats.wood)
        };
        let centre = match self.centre.as_ref().map(|c| c.kind) {
            None => "no centrepiece",
            Some(CentreKind::Well) => "a well",
            Some(CentreKind::Statue) => "a statue",
            Some(CentreKind::Arena) => "an arena",
        };
        format!(
            "{walls} walls, {} gates, {} towers, {} huts, {centre}, {} fortress sections, {} plateaus, {} bridges, {} posts",
            self.gates.len(),
            self.towers.len(),
            self.huts.len(),
            self.fort_arcs.len(),
            self.plateaus.len(),
            self.bridges.len(),
            self.posts.len(),
        )
    }
}
