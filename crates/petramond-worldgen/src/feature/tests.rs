use super::super::proto::MARGIN;
use super::tree_select::{tree_candidate_at, tree_spacing_allows};
use super::{feature_region_bounds, place_features_with_field, RuntimeFeatureField};
use petramond_world::biome::Biome;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use crate::density::surface::SurfaceDensitySystem;
use crate::generate_chunk;
use crate::region::RegionCells;

fn is_tree(id: u16) -> bool {
    let block = Block::from_id(id);
    block.is_log() || block.is_leaves()
}

fn synthetic_tree_region(x0: i32, z0: i32, w: usize, h: usize) -> RegionCells {
    let mut region = RegionCells::new(x0, z0, w, h);
    region.surf.fill(70);
    region.biomes.fill(Biome::RedwoodForest);
    region
}

/// Unclipped write collector: trees now overhang a single chunk (footprint
/// up to MARGIN), so the shape invariants below inspect the feature's FULL
/// write set instead of one chunk's clipped slice.
struct MapSink(std::collections::HashMap<petramond_world::mathh::IVec3, Block>);

impl super::VoxelSink for MapSink {
    fn get(&self, p: petramond_world::mathh::IVec3) -> Block {
        self.0.get(&p).copied().unwrap_or(Block::Air)
    }
    fn set(&mut self, p: petramond_world::mathh::IVec3, b: Block) {
        self.0.insert(p, b);
    }
}

fn generate_into_map(
    feat: &'static super::ConfiguredFeature,
    seed: u32,
) -> std::collections::HashMap<petramond_world::mathh::IVec3, Block> {
    generate_into_map_with_open(feat, seed, &mut |_| true)
}

fn generate_into_map_with_open(
    feat: &'static super::ConfiguredFeature,
    seed: u32,
    open: &mut dyn FnMut(petramond_world::mathh::IVec3) -> bool,
) -> std::collections::HashMap<petramond_world::mathh::IVec3, Block> {
    use petramond_world::mathh::IVec3;
    use crate::rng::FeatureRng;
    let mut sink = MapSink(std::collections::HashMap::new());
    let mut rng = FeatureRng::positional(seed, 0xACAC, 0, 0, 0);
    {
        let mut ctx = super::FeatureCtx::new(&mut sink);
        feat.feature
            .generate(&mut ctx, open, IVec3::new(0, 64, 0), &mut rng);
    }
    sink.0
}

/// Every leaf a configured tree places must reach one of its logs within
/// `MAX_LOG_DISTANCE` FACE-steps travelling only through leaves — the exact
/// rule `block::behavior::leaves` decays against. Diagonal-only attachment (the
/// acacia umbrella bug) does not count, so this guards against canopies that
/// silently rot after generation.
#[test]
fn configured_trees_place_only_orthogonally_supported_leaves() {
    use crate::data::features;

    for (name, feat) in [
        ("acacia", features::acacia()),
        ("oak_young", features::oak_young()),
        ("oak_small", features::oak_small()),
        ("oak_swamp", features::oak_swamp()),
        ("oak_big", features::oak_big()),
        ("spruce", features::spruce()),
        ("redwood", features::redwood()),
    ] {
        for seed in [1u32, 7, 42, 99, 1000, 31337] {
            let map = generate_into_map(feat, seed);
            assert_all_leaves_supported(&map, &format!("{name} seed {seed}"));
        }
    }
}

/// Assert every leaf in a feature write set reaches one of its logs within
/// `MAX_LOG_DISTANCE` FACE-steps travelling only through leaves — the exact
/// rule `block::behavior::leaves` decays against. Diagonal-only attachment (the
/// acacia umbrella bug) does not count, so this guards against canopies that
/// silently rot after generation.
fn assert_all_leaves_supported(
    map: &std::collections::HashMap<petramond_world::mathh::IVec3, Block>,
    what: &str,
) {
    use petramond_world::block::behavior::MAX_LOG_DISTANCE;
    use std::collections::{HashSet, VecDeque};

    const FACES: [(i32, i32, i32); 6] = [
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ];

    let mut leaves = HashSet::new();
    let mut logs = HashSet::new();
    for (p, b) in map {
        if b.is_leaves() {
            leaves.insert((p.x, p.y, p.z));
        } else if b.is_log() {
            logs.insert((p.x, p.y, p.z));
        }
    }
    assert!(!leaves.is_empty(), "{what}: placed no leaves");

    for &start in &leaves {
        let mut visited = HashSet::from([start]);
        let mut frontier = VecDeque::from([(start, 0)]);
        let mut supported = false;
        'bfs: while let Some(((sx, sy, sz), dist)) = frontier.pop_front() {
            for (dx, dy, dz) in FACES {
                let n = (sx + dx, sy + dy, sz + dz);
                if logs.contains(&n) {
                    supported = true;
                    break 'bfs;
                }
                if dist + 1 < MAX_LOG_DISTANCE && leaves.contains(&n) && visited.insert(n) {
                    frontier.push_back((n, dist + 1));
                }
            }
        }
        assert!(
            supported,
            "{what}: leaf at {start:?} only diagonally attached — it would decay"
        );
    }
}

/// The `TreeFeature` canopies against a closed half-space — a hillside or the
/// wall of a cave beside the trunk. No leaf may land in a closed cell (the old
/// layer loops skipped the wall and kept placing in the cave air behind it),
/// and everything placed must still satisfy the decay-support rule (the old
/// loops stranded outer leaves whose inward path the wall interrupted).
#[test]
fn tree_canopies_respect_closed_cells_and_stay_supported() {
    use crate::data::features;

    for (name, feat) in [
        ("spruce", features::spruce()),
        ("oak_swamp", features::oak_swamp()),
        ("acacia", features::acacia()),
    ] {
        for seed in [1u32, 7, 42, 99, 1000, 31337] {
            // Wall right beside the trunk column: everything at x ≥ 2 closed.
            let map = generate_into_map_with_open(feat, seed, &mut |p| p.x < 2);
            for (p, b) in &map {
                assert!(
                    !(b.is_leaves() && p.x >= 2),
                    "{name} seed {seed}: leaf at {p:?} inside the closed half-space"
                );
            }
            assert_all_leaves_supported(&map, &format!("{name} (walled) seed {seed}"));
        }
    }
}

/// Seen from straight above, an oak's trunk top must end in leaves, never
/// a bare log end — the exposed-top-log artifact playtesting flagged
/// (2026-07-12). The trunk centre wanders within ±1 of the origin, so the
/// tallest log column in that window is the trunk top; its column must
/// hold a leaf above the log.
#[test]
fn oak_crowns_bury_the_trunk_top() {
    use crate::data::features;

    for (name, feat) in [
        ("oak_young", features::oak_young()),
        ("oak_small", features::oak_small()),
        ("oak_big", features::oak_big()),
    ] {
        for seed in [1u32, 7, 42, 99, 1000, 31337] {
            let map = generate_into_map(feat, seed);
            let top_log = |x: i32, z: i32| {
                map.iter()
                    .filter(|(p, b)| p.x == x && p.z == z && b.is_log())
                    .map(|(p, _)| p.y)
                    .max()
            };
            let best = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
                .filter_map(|(x, z)| top_log(x, z).map(|y| (x, z, y)))
                .max_by_key(|&(_, _, y)| y)
                .expect("trunk has logs");
            let covered = map
                .iter()
                .any(|(p, b)| p.x == best.0 && p.z == best.1 && p.y > best.2 && b.is_leaves());
            assert!(
                covered,
                "{name} seed {seed}: bare trunk-top log exposed at column ({}, {}), y {}",
                best.0, best.1, best.2
            );
        }
    }
}

/// The oak anchoring gate: flat ground accepts, a drop under the root
/// splay rejects the whole tree — the floating-tree guard.
#[test]
fn oaks_refuse_sites_where_roots_would_hang() {
    use petramond_world::mathh::IVec3;
    use crate::data::features;
    use crate::rng::FeatureRng;

    for (name, feat) in [
        ("oak_young", features::oak_young()),
        ("oak_small", features::oak_small()),
        ("oak_big", features::oak_big()),
    ] {
        for seed in [1u32, 7, 42, 99, 1000, 31337] {
            let origin = IVec3::new(0, 64, 0);
            let rng = FeatureRng::positional(seed, 0xACAC, 0, 0, 0);
            assert!(
                feat.feature.is_anchored(&mut |_, _| 64, origin, rng),
                "{name} seed {seed}: flat ground must anchor"
            );
            // Everything but the origin column drops far below: some base
            // cell always lies off-column, so the site must be refused.
            assert!(
                !feat.feature.is_anchored(
                    &mut |x, z| if x == 0 && z == 0 { 64 } else { 40 },
                    origin,
                    rng,
                ),
                "{name} seed {seed}: a cliff under the roots must refuse the site"
            );
        }
    }
}

fn accepted_tree_origins(seed: u32, chunk_radius: i32) -> Vec<(i32, i32, i32)> {
    let mut origins = Vec::new();

    for cz in -chunk_radius..=chunk_radius {
        for cx in -chunk_radius..=chunk_radius {
            let ox = cx * CHUNK_SX as i32;
            let oz = cz * CHUNK_SZ as i32;
            let (x0, z0, w, h) = feature_region_bounds(ox, oz);
            let field = synthetic_tree_region(x0, z0, w, h);
            let mut field = &field;
            for wz in oz..(oz + CHUNK_SZ as i32) {
                for wx in ox..(ox + CHUNK_SX as i32) {
                    let Some(candidate) = tree_candidate_at(&mut field, seed, wx, wz) else {
                        continue;
                    };
                    if tree_spacing_allows(candidate, &mut field, seed, wx, wz) {
                        origins.push((wx, wz, candidate.spacing_radius));
                    }
                }
            }
        }
    }

    origins
}

#[test]
fn tree_origin_spacing_rule_enforces_configured_radius() {
    for seed in [1u32, 7, 42, 0x1234_5678] {
        let origins = accepted_tree_origins(seed, 2);
        assert!(
            origins.len() > 10,
            "spacing test sampled too few tree origins for seed {seed:#x}"
        );

        for i in 0..origins.len() {
            for j in (i + 1)..origins.len() {
                let (ax, az, ar) = origins[i];
                let (bx, bz, br) = origins[j];
                let dx = (ax - bx).abs();
                let dz = (az - bz).abs();
                let required = ar.max(br);
                assert!(
                    dx > required || dz > required,
                    "tree origins ({ax},{az}) and ({bx},{bz}) are within {required} blocks"
                );
            }
        }
    }
}

#[test]
fn live_density_feature_region_covers_margin_and_spacing_queries() {
    let seed = 7u32;
    let surface = SurfaceDensitySystem::new(seed);

    for (cx, cz) in [(0, 0), (-2, 1), (4, -3)] {
        let ox = cx * CHUNK_SX as i32;
        let oz = cz * CHUNK_SZ as i32;
        let (x0, z0, w, h) = feature_region_bounds(ox, oz);
        let field = surface.region(x0, z0, w, h);
        let mut field = &field;

        for wz in (oz - MARGIN)..(oz + CHUNK_SZ as i32 + MARGIN) {
            for wx in (ox - MARGIN)..(ox + CHUNK_SX as i32 + MARGIN) {
                if let Some(candidate) = tree_candidate_at(&mut field, seed, wx, wz) {
                    let _ = tree_spacing_allows(candidate, &mut field, seed, wx, wz);
                }
            }
        }
    }
}

#[test]
fn runtime_feature_field_matches_full_region_features() {
    let seed = 0x1234_5678;
    let surface = SurfaceDensitySystem::new(seed);
    let caves = crate::noise::cave_field::CaveField::new(seed);

    for (cx, cz) in [(0, 0), (-3, 5), (12, -7), (4, -3)] {
        let ox = cx * CHUNK_SX as i32;
        let oz = cz * CHUNK_SZ as i32;
        let (x0, z0, w, h) = feature_region_bounds(ox, oz);
        // The runtime field bakes the cave adjustment into its candidate
        // window, so the reference full-region field gets the same per-cell
        // adjustment before comparing.
        let mut full_region = surface.region(x0, z0, w, h);
        for (i, s) in full_region.surf.iter_mut().enumerate() {
            let wx = full_region.x0 + (i % full_region.w) as i32;
            let wz = full_region.z0 + (i / full_region.w) as i32;
            *s = caves.feature_surface_after_caves(wx, wz, *s);
        }
        let mut full_field = &full_region;

        let mut full_chunk = Chunk::new(cx, cz);
        place_features_with_field(&mut full_chunk, &mut full_field, seed);

        let mut runtime_chunk = Chunk::new(cx, cz);
        let mut field = RuntimeFeatureField::new(&surface, &caves, seed, ox, oz);
        place_features_with_field(&mut runtime_chunk, &mut field, seed);

        assert_eq!(
            full_chunk.blocks_slice(),
            runtime_chunk.blocks_slice(),
            "feature blocks differ at ({cx},{cz})"
        );
    }
}

#[test]
fn generate_chunk_is_deterministic() {
    let seed = 0x1234_5678;
    for &(cx, cz) in &[(0, 0), (3, -2), (-5, 7), (12, 9)] {
        let a = generate_chunk(seed, cx, cz);
        let b = generate_chunk(seed, cx, cz);
        assert_eq!(
            a.blocks_slice(),
            b.blocks_slice(),
            "blocks differ at {cx},{cz}"
        );
        assert_eq!(
            a.biomes_slice(),
            b.biomes_slice(),
            "biomes differ at {cx},{cz}"
        );
    }
}

#[test]
fn features_occupy_chunk_edges() {
    // P4 removed the chunk-edge skip: trees may now sit on the border.
    for seed in [1u32, 7, 42, 0x1234_5678] {
        let mut c = Chunk::new(0, 0);
        let (x0, z0, w, h) = feature_region_bounds(0, 0);
        let field = synthetic_tree_region(x0, z0, w, h);
        let mut field = &field;
        place_features_with_field(&mut c, &mut field, seed);

        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let edge = x == 0 || x == CHUNK_SX - 1 || z == 0 || z == CHUNK_SZ - 1;
                if !edge {
                    continue;
                }
                for y in 0..CHUNK_SY {
                    if is_tree(c.block_raw(x, y, z)) {
                        return;
                    }
                }
            }
        }
    }
    panic!("no tree blocks on any chunk edge — edge-skip not removed?");
}

#[test]
fn trees_span_chunk_seams() {
    // A trunk rooted on the west border of chunk (cx,cz) (world x = cx*16)
    // must have canopy reaching into the previous chunk's east column
    // (local x = 15). Any one confirmed seam-spanning tree proves the
    // cross-chunk feature mechanism (no bald seam, no gap).
    for seed in [1u32, 7, 13, 42, 0x1234_5678] {
        for cz in 0..6 {
            for cx in 1..6 {
                let west = generate_chunk(seed, cx - 1, cz);
                let east = generate_chunk(seed, cx, cz);
                for z in 0..CHUNK_SZ {
                    for y in 2..CHUNK_SY - 2 {
                        if east.block_raw(0, y, z) != Block::OakLog.id() {
                            continue;
                        }
                        // Canopy of this trunk should reach the west chunk's
                        // x = 15 column near (y.., z..).
                        let z_lo = z.saturating_sub(2);
                        let z_hi = (z + 3).min(CHUNK_SZ);
                        for yy in y..(y + 8).min(CHUNK_SY) {
                            for zz in z_lo..z_hi {
                                if is_tree(west.block_raw(15, yy, zz)) {
                                    return; // seam-spanning tree confirmed
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    panic!("no seam-spanning tree found in the sampled region");
}
