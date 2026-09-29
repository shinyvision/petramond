use super::*;
use crate::biome::climate::{AxisRange, BiomeClimateEntry, ClimateRect, SurfaceClimate};
use crate::density::terrain::TerrainDensitySpec;
use crate::graph::{Channel, SamplePoint, SampledScalarField};
use petramond_world::chunk::Chunk;
use petramond_world::chunk::{idx, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use petramond_world::section::Section;

#[derive(Debug)]
struct PlaneDensity {
    surface_y: f64,
}

impl SampledScalarField for PlaneDensity {
    fn sample(&self, point: SamplePoint) -> f64 {
        self.surface_y - point.y
    }
}

#[derive(Debug)]
struct CoastContinentality;

impl SampledScalarField for CoastContinentality {
    fn sample(&self, point: SamplePoint) -> f64 {
        if point.x < 0.0 {
            -0.5
        } else if point.x <= 16.0 {
            0.05
        } else {
            0.5
        }
    }

    fn depends_on_y(&self) -> bool {
        false
    }
}

fn plains_index() -> BiomeClimateIndex {
    const ANY: AxisRange = AxisRange::new(-1.0, 1.0);
    static PLAINS: &[ClimateRect] = &[ClimateRect::surface(ANY, ANY, ANY, ANY, ANY)];
    BiomeClimateIndex::new(&[BiomeClimateEntry {
        biome: Biome::PLAINS,
        rectangles: PLAINS,
    }])
}

fn coast_index() -> BiomeClimateIndex {
    const ANY: AxisRange = AxisRange::new(-1.0, 1.0);
    static OCEAN: &[ClimateRect] = &[ClimateRect::surface(
        ANY,
        ANY,
        AxisRange::new(-1.0, -0.2),
        ANY,
        ANY,
    )];
    static PLAINS: &[ClimateRect] = &[ClimateRect::surface(
        ANY,
        ANY,
        AxisRange::new(0.0, 1.0),
        ANY,
        ANY,
    )];
    BiomeClimateIndex::new(&[
        BiomeClimateEntry {
            biome: Biome::OCEAN,
            rectangles: OCEAN,
        },
        BiomeClimateEntry {
            biome: Biome::PLAINS,
            rectangles: PLAINS,
        },
    ])
}

fn test_system(field: impl SampledScalarField + 'static) -> SurfaceDensitySystem {
    let seed = 0x1234_5678;
    let mut density = TerrainDensitySpec::default_surface().build_graph(seed);
    let node = density.graph_mut().sampled_field(field);
    density
        .graph_mut()
        .set_channel(Channel::new(channels::MASTER_DENSITY), node);
    SurfaceDensitySystem {
        seed,
        columns: Columns::new(density.into(), crate::cache::installed()),
        climate: Box::leak(Box::new(plains_index())),
        surface: SurfaceSystem,
    }
}

fn coast_system() -> SurfaceDensitySystem {
    let seed = 0x1234_5678;
    let mut density = TerrainDensitySpec::default_surface().build_graph(seed);
    let density_node = density
        .graph_mut()
        .sampled_field(PlaneDensity { surface_y: 65.0 });
    density
        .graph_mut()
        .set_channel(Channel::new(channels::MASTER_DENSITY), density_node);
    let continentality = density.graph_mut().sampled_field(CoastContinentality);
    let zero = density.graph_mut().constant(0.0);
    density
        .graph_mut()
        .set_channel(Channel::new(channels::TEMPERATURE), zero);
    density
        .graph_mut()
        .set_channel(Channel::new(channels::HUMIDITY), zero);
    density
        .graph_mut()
        .set_channel(Channel::new(channels::CONTINENTALITY), continentality);
    density
        .graph_mut()
        .set_channel(Channel::new(channels::EROSION), zero);
    density
        .graph_mut()
        .set_channel(Channel::new(channels::VARIANCE), zero);
    SurfaceDensitySystem {
        seed,
        columns: Columns::new(density.into(), crate::cache::installed()),
        climate: Box::leak(Box::new(coast_index())),
        surface: SurfaceSystem,
    }
}

fn generate_surface_chunk(system: &SurfaceDensitySystem, cx: i32, cz: i32) -> Chunk {
    let region = system.region(
        cx * CHUNK_SX as i32,
        cz * CHUNK_SZ as i32,
        CHUNK_SX,
        CHUNK_SZ,
    );
    section_fill(system, cx, cz, &region)
}

fn section_fill(system: &SurfaceDensitySystem, cx: i32, cz: i32, region: &RegionCells) -> Chunk {
    let biomes: Vec<u8> = region.biomes.iter().map(|b| b.id()).collect();
    let mut chunk = Chunk::new(cx, cz);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_biome(x, z, biomes[z * CHUNK_SX + x]);
        }
    }
    for cy in 0..(CHUNK_SY / SECTION_SIZE) as i32 {
        let mut section = Section::new(cx, cy, cz);
        system.fill_section(&mut section, &biomes, &region.surf);
        for ly in 0..SECTION_SIZE {
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    chunk.blocks_slice_mut()[idx(x, cy as usize * SECTION_SIZE + ly, z)] =
                        section.block_raw(x, ly, z);
                }
            }
        }
    }
    chunk.recompute_heightmap();
    chunk
}

fn lattice_fill(system: &SurfaceDensitySystem, cx: i32, cz: i32, region: &RegionCells) -> Chunk {
    let lattice = DensityLattice::sample_channel(
        system.columns.graph().graph(),
        channels::MASTER_DENSITY,
        DensityLatticeBounds::chunk(cx, cz),
        DensityLatticeCellSize::default(),
    )
    .expect("master density");
    let mut chunk = Chunk::new(cx, cz);
    let (ox, oz) = chunk.chunk_origin_world();
    let mut cells = system.climate_cells();
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            let (wx, wz) = (ox + x as i32, oz + z as i32);
            let (surf_y, biome) = region.at(wx, wz);
            chunk.set_biome(x, z, biome.id());
            let waterline = system.waterline_block(&mut cells, wx, wz, surf_y);
            let rule = spec(biome).surface;
            let mut run_top: Option<i32> = None;
            let mut depth_from_top = 0u32;
            for y in (0..CHUNK_SY).rev() {
                let wy = y as i32;
                let blocks = chunk.blocks_slice_mut();
                if !lattice.solid_at_local(x, y, z) {
                    run_top = None;
                    depth_from_top = 0;
                    if wy == SEA_LEVEL {
                        blocks[idx(x, y, z)] = waterline.id();
                    } else if wy < SEA_LEVEL {
                        blocks[idx(x, y, z)] = Block::Water.id();
                    }
                    continue;
                }
                let ctx = SurfaceCtx {
                    seed: system.seed,
                    wx,
                    wz,
                    y: wy,
                    surf_y: *run_top.get_or_insert(wy),
                    depth_from_top,
                };
                blocks[idx(x, y, z)] = system.surface.skin_block(&ctx, rule).id();
                depth_from_top += 1;
            }
        }
    }
    chunk.recompute_heightmap();
    chunk
}

fn top_solid_excluding_water(chunk: &Chunk, x: usize, z: usize) -> Option<i32> {
    (0..CHUNK_SY).rev().find_map(|y| {
        let block = chunk.block(x, y, z);
        (block != Block::Air && block != Block::Water).then_some(y as i32)
    })
}

#[test]
fn deep_skin_is_depth_independent_and_ignores_underwater_status() {
    let surface = SurfaceSystem;
    let deep = (MAX_SKIN_BAND_DEPTH + 1) as u32;
    let ctx = |wx: i32, wz: i32, surf_y: i32, depth: u32| SurfaceCtx {
        seed: 1,
        wx,
        wz,
        y: surf_y - depth as i32,
        surf_y,
        depth_from_top: depth,
    };

    for spec in crate::biome::specs() {
        for (wx, wz) in [(0, 0), (137, -911), (-4096, 512)] {
            for surf_y in [SEA_LEVEL - 20, SEA_LEVEL + 20, 160] {
                assert_eq!(
                    surface.skin_block(&ctx(wx, wz, surf_y, deep), spec.surface),
                    surface.skin_block(&ctx(wx, wz, surf_y, deep + 120), spec.surface),
                    "{:?} skin is depth-dependent below MAX_SKIN_BAND_DEPTH \
                     at ({wx},{wz}) surf_y={surf_y}",
                    spec.biome
                );
            }
            assert_eq!(
                surface.skin_block(&ctx(wx, wz, SEA_LEVEL - 20, deep), spec.surface),
                surface.skin_block(&ctx(wx, wz, SEA_LEVEL + 20, deep), spec.surface),
                "{:?} deep material differs between underwater and dry columns \
                 at ({wx},{wz})",
                spec.biome
            );
        }
    }
}

#[test]
fn density_sign_fill_produces_solid_air_and_sea_water() {
    let system = test_system(PlaneDensity { surface_y: 60.0 });
    let chunk = generate_surface_chunk(&system, 0, 0);

    assert_ne!(chunk.block(0, 59, 0), Block::Air);
    assert_ne!(chunk.block(0, 59, 0), Block::Water);
    assert_eq!(chunk.block(0, 60, 0), Block::Water);
    assert_eq!(chunk.block(0, SEA_LEVEL as usize, 0), Block::Water);
    assert_eq!(chunk.block(0, SEA_LEVEL as usize + 1, 0), Block::Air);
}

#[test]
fn region_top_solid_matches_filled_chunk_excluding_water() {
    let system = SurfaceDensitySystem::new(0xCAFE_BABE);
    let region = system.region(0, 0, CHUNK_SX, CHUNK_SZ);
    let chunk = section_fill(&system, 0, 0, &region);

    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            assert_eq!(
                top_solid_excluding_water(&chunk, x, z),
                Some(region.surf[z * CHUNK_SX + x]),
                "column ({x},{z})"
            );
        }
    }
}

#[test]
fn section_fill_matches_the_lattice_reference() {
    for (seed, cx, cz) in [
        (7, 0, 0),
        (7, -2, 1),
        (7, 4, -3),
        (34, 6, -1),
        (31337, 0, 0),
    ] {
        let system = SurfaceDensitySystem::new(seed);
        let region = system.region(
            cx * CHUNK_SX as i32,
            cz * CHUNK_SZ as i32,
            CHUNK_SX,
            CHUNK_SZ,
        );
        let reference = lattice_fill(&system, cx, cz, &region);
        let sections = section_fill(&system, cx, cz, &region);
        assert_eq!(
            reference.blocks_slice(),
            sections.blocks_slice(),
            "blocks differ at seed {seed} ({cx},{cz})"
        );
        assert_eq!(
            reference.biomes_slice(),
            sections.biomes_slice(),
            "biomes differ at seed {seed} ({cx},{cz})"
        );
    }
}

#[test]
fn biome_assignment_is_stable_across_overlapping_regions() {
    let system = SurfaceDensitySystem::new(7);
    let small = system.region(-8, 3, 16, 16);
    let large = system.region(-16, -5, 40, 32);

    for wz in 3..19 {
        for wx in -8..8 {
            assert_eq!(
                small.at(wx, wz).1,
                large.at(wx, wz).1,
                "biome mismatch at ({wx},{wz})"
            );
        }
    }
}

#[test]
fn fallback_biome_lookup_matches_region_biomes() {
    let system = SurfaceDensitySystem::new(99);
    let region = system.region(-4, -4, 12, 12);

    for wz in -4..8 {
        for wx in -4..8 {
            assert_eq!(
                system.biome_at(wx, wz),
                region.at(wx, wz).1,
                "biome mismatch at ({wx},{wz})"
            );
        }
    }
}

#[test]
fn beach_is_derived_only_on_low_land_near_ocean_climate() {
    let system = coast_system();

    assert_eq!(system.biome_at(-8, 0), Biome::OCEAN);
    assert_eq!(system.biome_at(8, 0), Biome::BEACH);
    assert_eq!(system.biome_at(40, 0), Biome::PLAINS);
}

#[test]
fn climate_classification_uses_variance_derived_ridge() {
    let index = plains_index();
    assert_eq!(
        index.classify_surface(SurfaceClimate::new(0.0, 0.0, 0.0, 0.0, 0.25)),
        Some(Biome::PLAINS)
    );
}

#[test]
fn surfaces_are_searched_from_the_density_floor_to_the_world_top() {
    assert_eq!(SURFACE_SEARCH_Y.end, WORLD_MAX_Y);
    assert_eq!(
        SURFACE_SEARCH_Y.start,
        FloorDensitySpec::default_surface().floor_y as i32
    );
    assert_eq!(SURFACE_FLOOR_Y, SURFACE_SEARCH_Y.start - 1);
    let system = SurfaceDensitySystem::new(0x5EA_0013);
    let heights = surface_heights(&system.columns, -40, 25, 24, 24);
    assert_eq!(heights.len(), 24 * 24);
    assert!(heights.iter().all(|h| SURFACE_SEARCH_Y.contains(h)));
}
