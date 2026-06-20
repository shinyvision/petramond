//! Assembly of the 1.8 biome cascade as composable [`Layer`]s.
//!
//! Active worldgen uses the land-only branch for base terrain. Rivers are carved
//! later by `worldgen::river` as explicit path objects, so the classic river
//! overlay remains available only as reference/parity machinery.

use super::layers::*;

/// Continent → … → deep-ocean (the chain before biome assignment), at scale 256.
pub fn deep_ocean_256(seed: i64) -> Box<dyn Layer> {
    let l: Box<dyn Layer> = Box::new(Continent::new(seed));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2000, true, l));
    let l: Box<dyn Layer> = Box::new(Land::new(seed, 1, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2001, false, l));
    let l: Box<dyn Layer> = Box::new(Land::new(seed, 2, l));
    let l: Box<dyn Layer> = Box::new(Land::new(seed, 50, l));
    let l: Box<dyn Layer> = Box::new(Land::new(seed, 70, l));
    let l: Box<dyn Layer> = Box::new(RemoveOcean::new(seed, 2, l));
    let l: Box<dyn Layer> = Box::new(Snow::new(seed, 2, l));
    let l: Box<dyn Layer> = Box::new(Land::new(seed, 3, l));
    let l: Box<dyn Layer> = Box::new(Cool::new(l));
    let l: Box<dyn Layer> = Box::new(Heat::new(l));
    let l: Box<dyn Layer> = Box::new(Special::new(seed, 3, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2002, false, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2003, false, l));
    let l: Box<dyn Layer> = Box::new(Land::new(seed, 4, l));
    let l: Box<dyn Layer> = Box::new(Mushroom::new(seed, 5, l));
    Box::new(DeepOcean::new(l))
}

/// Biome assignment over the deep-ocean chain (scale 256).
pub fn biome_256(seed: i64) -> Box<dyn Layer> {
    Box::new(Biome::new(seed, 200, deep_ocean_256(seed)))
}

/// Biome edge at scale 64 (biome → zoom×2 → edge).
pub fn biome_edge_64(seed: i64) -> Box<dyn Layer> {
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1000, false, biome_256(seed)));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1001, false, l));
    Box::new(BiomeEdge::new(seed, 1000, l))
}

/// River-init noise (scale 256) off the deep-ocean chain.
pub fn river_init(seed: i64) -> Box<dyn Layer> {
    Box::new(RiverInit::new(seed, 100, deep_ocean_256(seed)))
}

fn hills_branch(seed: i64) -> Box<dyn Layer> {
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 0, false, river_init(seed)));
    Box::new(Zoom::new(seed, 0, false, l))
}

/// The main biome branch through smooth (scale 4), pre river-mix.
pub fn main_branch(seed: i64) -> Box<dyn Layer> {
    let l: Box<dyn Layer> = Box::new(Hills::new(
        seed,
        1000,
        biome_edge_64(seed),
        hills_branch(seed),
    ));
    let l: Box<dyn Layer> = Box::new(Sunflower::new(seed, 1001, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1000, false, l));
    let l: Box<dyn Layer> = Box::new(Land::new(seed, 3, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1001, false, l));
    let l: Box<dyn Layer> = Box::new(Shore::new(seed, 1000, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1002, false, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1003, false, l));
    Box::new(Smooth::new(seed, 1000, l))
}

fn river_branch(seed: i64) -> Box<dyn Layer> {
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1000, false, river_init(seed)));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1001, false, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1000, false, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1001, false, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1002, false, l));
    let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1003, false, l));
    let l: Box<dyn Layer> = Box::new(River::new(seed, 1, l));
    Box::new(Smooth::new(seed, 1000, l))
}

/// River-mix (scale 4) — the biome grid the terrain generator samples.
pub fn river_mix(seed: i64) -> Box<dyn Layer> {
    Box::new(RiverMix::new(
        seed,
        100,
        main_branch(seed),
        river_branch(seed),
    ))
}

/// Land-only biome grid (scale 4), before the classic river overlay.
pub fn land_mix(seed: i64) -> Box<dyn Layer> {
    main_branch(seed)
}

/// Land-only per-block biome grid (scale 1), before river carving.
pub fn land_voronoi(seed: i64) -> Box<dyn Layer> {
    Box::new(Voronoi::new(seed, 10, land_mix(seed)))
}

/// Classic river-overlay Voronoi (scale 1), kept for diagnostics/reference.
pub fn voronoi(seed: i64) -> Box<dyn Layer> {
    Box::new(Voronoi::new(seed, 10, river_mix(seed)))
}
