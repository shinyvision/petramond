use super::*;
use crate::chunk::Chunk;
use crate::worldgen::biome::climate::{AxisRange, BiomeClimateEntry, ClimateRect, SurfaceClimate};
use crate::worldgen::graph::{Channel, SamplePoint, SampledScalarField};

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
struct AlternatingRuns;

impl SampledScalarField for AlternatingRuns {
    fn sample(&self, point: SamplePoint) -> f64 {
        let y = point.y as i32;
        match y {
            72 | 88 => 1.0,
            80 | 96 => -1.0,
            _ => -1.0,
        }
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
        biome: Biome::Plains,
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
            biome: Biome::Ocean,
            rectangles: OCEAN,
        },
        BiomeClimateEntry {
            biome: Biome::Plains,
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
        density,
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
        density,
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
    let mut proto = ProtoChunk::new(cx, cz);
    system.fill_chunk(&mut proto, &region);
    proto.into_chunk()
}

fn top_solid_excluding_water(chunk: &Chunk, x: usize, z: usize) -> Option<i32> {
    (0..CHUNK_SY).rev().find_map(|y| {
        let block = chunk.block(x, y, z);
        (block != Block::Air && block != Block::Water).then_some(y as i32)
    })
}

fn exposed_solid_run_tops(chunk: &Chunk, x: usize, z: usize) -> Vec<i32> {
    (0..CHUNK_SY)
        .rev()
        .filter_map(|y| {
            let block = chunk.block(x, y, z);
            if block == Block::Air || block == Block::Water {
                return None;
            }
            let above_open =
                y + 1 >= CHUNK_SY || matches!(chunk.block(x, y + 1, z), Block::Air | Block::Water);
            above_open.then_some(y as i32)
        })
        .collect()
}

/// The deep fast path in `fill_section` requires every biome rule to be
/// depth-independent below `MAX_SKIN_BAND_DEPTH`, and `SurfaceCond::Underwater`
/// is a whole-column predicate — a depth-ungated underwater branch skins the
/// column to bedrock, so cave carving exposes all-sand/all-dirt caves under
/// water bodies.
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

    for spec in crate::worldgen::biome::SPECS.iter() {
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
fn surface_dressing_resets_across_multiple_solid_runs() {
    let system = test_system(AlternatingRuns);
    let chunk = generate_surface_chunk(&system, 0, 0);
    let run_tops = exposed_solid_run_tops(&chunk, 0, 0);

    assert!(
        run_tops.len() >= 2,
        "test density should produce multiple exposed solid runs"
    );
    for y in run_tops.into_iter().take(2) {
        assert_eq!(chunk.block(0, y as usize, 0), Block::Grass);
    }
}

#[test]
fn region_top_solid_matches_filled_chunk_excluding_water() {
    let system = SurfaceDensitySystem::new(0xCAFE_BABE);
    let region = system.region(0, 0, CHUNK_SX, CHUNK_SZ);
    let mut proto = ProtoChunk::new(0, 0);
    system.fill_chunk(&mut proto, &region);
    let chunk = proto.into_chunk();

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
fn direct_fill_matches_region_fill() {
    let system = SurfaceDensitySystem::new(7);

    for (cx, cz) in [(0, 0), (-2, 1), (4, -3)] {
        let ox = cx * CHUNK_SX as i32;
        let oz = cz * CHUNK_SZ as i32;
        let region = system.region(ox, oz, CHUNK_SX, CHUNK_SZ);

        let mut region_proto = ProtoChunk::new(cx, cz);
        system.fill_chunk(&mut region_proto, &region);
        let region_chunk = region_proto.into_chunk();

        let mut direct_proto = ProtoChunk::new(cx, cz);
        system.fill_chunk_direct(&mut direct_proto);
        let direct_chunk = direct_proto.into_chunk();

        let mut from_proto = ProtoChunk::new(cx, cz);
        system.fill_chunk_from(&mut from_proto, &region.biomes, &region.surf);
        let from_chunk = from_proto.into_chunk();

        assert_eq!(
            region_chunk.blocks_slice(),
            direct_chunk.blocks_slice(),
            "blocks differ at ({cx},{cz})"
        );
        assert_eq!(
            region_chunk.biomes_slice(),
            direct_chunk.biomes_slice(),
            "biomes differ at ({cx},{cz})"
        );
        assert_eq!(
            region_chunk.blocks_slice(),
            from_chunk.blocks_slice(),
            "surf-driven fill blocks differ at ({cx},{cz})"
        );
        assert_eq!(
            region_chunk.biomes_slice(),
            from_chunk.biomes_slice(),
            "surf-driven fill biomes differ at ({cx},{cz})"
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

    assert_eq!(system.biome_at(-8, 0), Biome::Ocean);
    assert_eq!(system.biome_at(8, 0), Biome::Beach);
    assert_eq!(system.biome_at(40, 0), Biome::Plains);
}

#[test]
fn climate_classification_uses_variance_derived_ridge() {
    let index = plains_index();
    assert_eq!(
        index.classify_surface(SurfaceClimate::new(0.0, 0.0, 0.0, 0.0, 0.25)),
        Some(Biome::Plains)
    );
}
