//! Skeleton camps: round-ish, walled camps of wood or stone, tattered by design, scattered over
//! the surface. A site's camp is derived once into a build plan from positional reads only and
//! clipped into every section it reaches, so sections may generate in any order.
//!
//! A camp is derived in three stages, each handing the next what it settled: [`survey()`] decides
//! whether the site takes a camp and fixes its [`Outline`](survey::Outline), the [`Planner`] lays out where
//! everything stands, and the [`Builder`] builds that [`Layout`] into the plan without changing
//! it.

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

mod build;
mod centre;
mod flag;
mod gates;
mod grid;
mod ground;
mod huts;
mod layout;
mod plateaus;
mod posts;
mod style;
mod survey;
mod towers;
mod walls;

#[cfg(test)]
mod tests;

use mod_sdk::build::{Derived, Families, Heights, Name, Plan, PlanCache, Site, SiteGrid};
use mod_sdk::*;

use build::Builder;
use layout::{Layout, Planner};
use survey::{survey, Survey, Surveyed};

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
    /// Whether camps fly a skull flag: the flags pack is loaded alongside.
    flagged: bool,
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
        let flagged = [crate::keys::SKULL_FLAG, crate::keys::FLAGPOLE]
            .iter()
            .all(|name| resolve_block(name).is_some());
        // Plans are memoized per seed across worlds, and a flagged camp is a different plan.
        let name = if flagged {
            "skeleton_camp_flagged"
        } else {
            "skeleton_camp"
        };
        Some(Camps {
            families,
            flagged,
            cache: PlanCache::new(name, 16),
            ids: FxHashMap::default(),
        })
    }

    pub(crate) fn generate(&mut self, ctx: &GenCtx) -> GenOutput {
        let derive = derive_logged(ctx.seed(), ctx.sea_level(), &self.families, self.flagged);
        self.cache
            .generate(&GRID, ctx, derive, &mut resolver(&mut self.ids))
            .unwrap_or_else(|_| GenOutput::deferred())
    }

    pub(crate) fn claims(&mut self, ctx: &ClaimsCtx) -> GenClaims {
        let derive = derive_logged(ctx.seed, ctx.sea_level, &self.families, self.flagged);
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

/// [`derive()`] from the host's terrain, logging what each camp built.
fn derive_logged(
    seed: u32,
    sea_level: i32,
    families: &Families,
    flagged: bool,
) -> impl Fn(Site) -> Derived + '_ {
    move |site| {
        let [x, z] = site.center;
        let mut report = |summary: &str| log(&format!("skeleton camp at {x} {z}: {summary}"));
        derive(
            seed,
            sea_level,
            site,
            families,
            flagged,
            &HostTerrain,
            &mut report,
        )
    }
}

/// Derives the camp at `site`, telling `report` what it built. A `flagged` camp flies a skull
/// flag.
pub(crate) fn derive(
    seed: u32,
    sea_level: i32,
    site: Site,
    families: &Families,
    flagged: bool,
    terrain: &dyn Terrain,
    report: &mut dyn FnMut(&str),
) -> Derived {
    let rng = GRID.rng(seed, site);
    match survey(site.center, sea_level, families, terrain, rng) {
        Survey::Camp(surveyed) => {
            let camp = Camp::raise(*surveyed, flagged);
            report(&camp.summary());
            Derived::Plan(Box::new(camp.plan))
        }
        Survey::Nothing => Derived::Nothing,
        Survey::Unavailable => Derived::Unavailable,
    }
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
    /// Taken by a ladder or a stair run during the build.
    Access,
}

/// A camp: surveyed, laid out, then built.
struct Camp {
    #[cfg(test)]
    outline: survey::Outline,
    layout: Layout,
    /// The floor as built.
    #[cfg(test)]
    ground: grid::Grid<i32>,
    posts: Vec<posts::Post>,
    plan: Plan,
    stone: bool,
    wood: Name,
    flag: bool,
}

impl Camp {
    fn raise(surveyed: Surveyed<'_>, flagged: bool) -> Camp {
        let Surveyed {
            mut rng,
            mats,
            outline,
            ground,
        } = surveyed;
        let (layout, ground) = Planner::lay_out(&outline, &mut rng, ground, flagged);
        let built = Builder::new(rng, mats, &outline, &layout, ground).build();
        Camp {
            #[cfg(test)]
            outline,
            layout,
            #[cfg(test)]
            ground: built.ground,
            posts: built.posts,
            plan: built.plan,
            stone: built.stone,
            wood: built.wood,
            flag: built.flag,
        }
    }

    fn summary(&self) -> String {
        use layout::CentreKind;
        let walls = if self.stone {
            "stone".to_string()
        } else {
            format!("{} wood", self.wood)
        };
        let layout = &self.layout;
        let centre = match layout.centre.as_ref().map(|c| c.kind) {
            None => "no centrepiece",
            Some(CentreKind::Well) => "a well",
            Some(CentreKind::Statue) => "a statue",
            Some(CentreKind::Arena) => "an arena",
        };
        format!(
            "{walls} walls, {} gates, {} towers, {} huts, {centre}, {} fortress sections, {} plateaus, {} bridges, {} posts{}",
            layout.gates.len(),
            layout.towers.len(),
            layout.huts.len(),
            layout.fort_arcs.len(),
            layout.plateaus.len(),
            layout.bridges.len(),
            self.posts.len(),
            if self.flag { ", a flag" } else { "" },
        )
    }
}
