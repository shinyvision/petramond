use super::*;
use petramond_world::block::{Block, BlockTag};
use petramond_world::particle_emitters::{AmbientHit, AmbientKill, AmbientMotion};
use petramond_world::tile::Tile;

/// A synthetic flier row: soil-bound, a 7-block lattice, a 24-block gather.
fn flier_spec(sprite: bool) -> AmbientSpec {
    AmbientSpec {
        count_per_intensity: 0.0,
        max_count: 0,
        radius: 24.0,
        height: [8.0, 8.0],
        fall_speed: [0.0, 0.0],
        drift_wind: 0.0,
        flutter: [0.0, 0.0],
        size: [0.32, 0.42],
        stretch: 1.0,
        alpha: [1.0, 1.0],
        color: [[1.0; 3], [1.0; 3]],
        color_bias: 1.0,
        palette: Vec::new(),
        hit: AmbientHit::Die,
        motion: AmbientMotion::Flight(FlightSpec {
            spacing: 7.0,
            occupancy: 1.0,
            orbit: [1.6, 1.3],
            hover: [1.1, 0.45],
            speed: [0.35, 0.65],
            flap_hz: [5.0, 7.0],
            sprite: sprite.then(|| "dirt".to_owned()),
            ground_tags: vec!["soil".to_owned()],
            sprite_tile: sprite.then(|| Tile::named("dirt")),
            ground: vec![BlockTag::SOIL],
        }),
        kill: AmbientKill::Ceiling,
        light: AmbientLight::World,
        biomes: Vec::new(),
        exclude_biomes: Vec::new(),
        biome_allow: None,
    }
}

fn flight_of(spec: &AmbientSpec) -> &FlightSpec {
    spec.motion.flight().expect("a flight row")
}

fn activation<'a>(intensity: f32, weight: &'a dyn Fn(u8) -> f32) -> Activation<'a> {
    Activation {
        seed: 7,
        intensity,
        wind: [0.0, 0.0],
        adv: [0.0, 0.0],
        biome_weight: Some(weight),
    }
}

fn collect(
    spec: &AmbientSpec,
    world: &World,
    cam: Vec3,
    time: f32,
    intensity: f32,
    weight: &dyn Fn(u8) -> f32,
    out: &mut Vec<ParticlePresentation>,
) {
    derive_flight(
        spec,
        flight_of(spec),
        &activation(intensity, weight),
        &View { world, cam, time },
        out,
    );
}

/// A 3×3-chunk dirt plateau (top face 65) split down x = 0 into biome 1
/// (west) and biome 2 (east).
fn habitat() -> World {
    habitat_with_height(|_, _| 64)
}

fn habitat_with_height(height: impl Fn(i32, i32) -> usize) -> World {
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    let mut world = World::new(0, 1);
    for cz in -1..=1 {
        for cx in -1..=1 {
            let mut chunk = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    let top = height(
                        cx * CHUNK_SX as i32 + x as i32,
                        cz * CHUNK_SZ as i32 + z as i32,
                    );
                    for y in 64..=top {
                        chunk.set_block(x, y, z, Block::Dirt);
                    }
                    chunk.set_biome(x, z, if cx < 0 { 1 } else { 2 });
                }
            }
            world.insert_chunk_for_test(ChunkPos::new(cx, cz), chunk);
        }
    }
    world
}

fn west_only(id: u8) -> f32 {
    if id == 1 {
        1.0
    } else {
        0.0
    }
}

fn everywhere(_: u8) -> f32 {
    1.0
}

#[test]
fn fliers_gate_their_own_columns_and_stay_anchored_when_the_camera_jumps() {
    let spec = flier_spec(true);
    let world = habitat();
    let cam = Vec3::new(0.0, 67.0, 8.0);
    let (mut base, mut jumped) = (Vec::new(), Vec::new());
    collect(&spec, &world, cam, 7.0, 1.0, &west_only, &mut base);
    collect(
        &spec,
        &world,
        cam + Vec3::Y,
        7.0,
        1.0,
        &west_only,
        &mut jumped,
    );
    assert!(!base.is_empty());
    assert_eq!(base, jumped, "camera motion never drags a flier");
    for wings in base.chunks_exact(2) {
        let center = (wings[0].pos + wings[1].pos) * 0.5;
        assert!(
            center.x < 0.0,
            "an orbit must not cross into a zero-density biome"
        );
        assert!(center.y > 65.0);
        assert!(wings.iter().all(|w| w.quad_axes.is_some()));
    }
    base.clear();
    collect(&spec, &world, cam, 7.0, 1.0, &|_| 0.0, &mut base);
    assert!(base.is_empty(), "zero density everywhere derives nothing");
    collect(
        &spec,
        &world,
        Vec3::new(0.0, 50.0, 8.0),
        7.0,
        1.0,
        &everywhere,
        &mut base,
    );
    assert!(
        base.is_empty(),
        "surface fliers must not follow a camera underground"
    );
}

#[test]
fn without_a_sprite_a_flier_is_an_ordinary_cube() {
    let spec = flier_spec(false);
    let world = habitat();
    let mut out = Vec::new();
    collect(
        &spec,
        &world,
        Vec3::new(0.0, 67.0, 8.0),
        7.0,
        1.0,
        &everywhere,
        &mut out,
    );
    assert!(!out.is_empty());
    assert!(out
        .iter()
        .all(|p| p.quad_axes.is_none() && p.atlas == ParticleAtlas::Solid));
}

#[test]
fn zero_intensity_derives_nothing() {
    let spec = flier_spec(true);
    let world = habitat();
    let mut out = Vec::new();
    collect(
        &spec,
        &world,
        Vec3::new(0.0, 67.0, 8.0),
        1.0,
        0.0,
        &everywhere,
        &mut out,
    );
    assert!(out.is_empty());
}

#[test]
fn orbit_is_continuous_and_stays_inside_its_cell() {
    let spec = flier_spec(true);
    let flight = flight_of(&spec);
    for seed in 0..100 {
        for time in [0.0, 10.0, 1000.0] {
            let (a, heading) = orbit(flight, seed, time);
            let (b, _) = orbit(flight, seed, time + 0.01);
            assert!(a.distance(b) < 0.03);
            assert!(a.y > 0.0 && Vec3::new(a.x, 0.0, a.z).length() < flight.spacing * 0.5);
            assert!(heading.is_finite() && (heading.length() - 1.0).abs() < 1e-5);
        }
    }
}

#[test]
fn an_admitted_flier_survives_its_whole_orbit_over_steps_and_density_borders() {
    let spec = flier_spec(true);
    let flight = flight_of(&spec);
    let world = habitat_with_height(|x, _| if x.rem_euclid(4) < 2 { 67 } else { 64 });
    let abundance = |id| if id == 1 { 1.0 } else { 0.8 };
    let act = activation(1.0, &abundance);
    let (seed, x, z, ground) = (-1i32..=1)
        .find_map(|gx| {
            let seed = cell_seed(&act, gx, 0);
            let (x, z) = cell_anchor(flight, seed, gx, 0);
            let roll = hash01(seed ^ 5) / flight.occupancy;
            orbit_ground(&spec, flight, &act, &world, x, z, roll).map(|g| (seed, x, z, g))
        })
        .expect("fixture contains an admitted flight");
    let cam = Vec3::new(x, ground + 1.0, z);
    let mut out = Vec::new();
    for step in 0..120 {
        let time = step as f32 * 0.25;
        let (offset, _) = orbit(flight, seed, time);
        out.clear();
        collect(&spec, &world, cam, time, 1.0, &abundance, &mut out);
        let wings = out
            .chunks_exact(2)
            .find(|wings| {
                let center = (wings[0].pos + wings[1].pos) * 0.5;
                (center.x - x - offset.x).abs() < 1e-4 && (center.z - z - offset.z).abs() < 1e-4
            })
            .expect("an admitted flier must never disappear partway through its orbit");
        for wing in wings {
            let [right, up] = wing.quad_axes.unwrap();
            for corner in [
                wing.pos - right - up,
                wing.pos + right - up,
                wing.pos + right + up,
                wing.pos - right + up,
            ] {
                let floor = world
                    .precipitation_ceiling_y(corner.x.floor() as i32, corner.z.floor() as i32)
                    .unwrap();
                assert!(
                    corner.y > floor as f32 + 1.0,
                    "every wing corner clears the ground"
                );
            }
        }
    }
}

#[test]
fn an_orbit_touching_a_zero_density_biome_is_never_admitted() {
    let spec = flier_spec(true);
    let flight = flight_of(&spec);
    let world = habitat();
    let act = activation(1.0, &west_only);
    assert!(orbit_ground(&spec, flight, &act, &world, -0.5, 8.0, 0.0).is_none());
    assert!(orbit_ground(&spec, flight, &act, &world, -8.0, 8.0, 0.0).is_some());
}

#[test]
fn roofs_and_overhangs_reject_flights_even_above_a_ground_floor() {
    let spec = flier_spec(true);
    let flight = flight_of(&spec);
    let mut world = habitat();
    let act = activation(1.0, &everywhere);
    let ground = |world: &World, x: f32| orbit_ground(&spec, flight, &act, world, x, 8.5, 0.0);
    assert!(ground(&world, 0.5).is_some());
    for roof in [Block::OakPlanks, Block::Glass, Block::StoneSlab] {
        assert!(world.set_block_world(0, 70, 8, roof));
        assert!(ground(&world, 0.5).is_none());
        // The anchor stays outside: only part of the orbit passes under the eave.
        assert!(ground(&world, -1.0).is_none());
        assert!(ground(&world, -8.0).is_some());
        assert!(world.set_block_world(0, 70, 8, Block::Air));
        assert!(ground(&world, 0.5).is_some());
    }
}

#[test]
fn roof_clearance_includes_diagonally_rotated_wing_corners() {
    let spec = flier_spec(true);
    let flight = flight_of(&spec);
    let mut world = habitat();
    let act = activation(1.0, &everywhere);
    let x = -flight.orbit[0] - spec.size[1] * FRAC_1_SQRT_2 + 0.01;
    assert!(orbit_ground(&spec, flight, &act, &world, x, 8.5, 0.0).is_some());
    assert!(world.set_block_world(0, 70, 8, Block::Stone));
    assert!(orbit_ground(&spec, flight, &act, &world, x, 8.5, 0.0).is_none());
}

#[test]
fn canopies_reject_ground_flights_instead_of_lifting_them_to_the_treetop() {
    let spec = flier_spec(true);
    let flight = flight_of(&spec);
    let mut world = habitat();
    let act = activation(1.0, &everywhere);
    let ground = |world: &World, x: f32| orbit_ground(&spec, flight, &act, world, x, 8.5, 0.0);
    let original = ground(&world, 0.5);
    assert!(original.is_some());
    for canopy_y in [67, 75] {
        assert!(world.set_block_world(0, canopy_y, 8, Block::OakLeaves));
        assert!(ground(&world, 0.5).is_none());
        // A canopy at the orbit's edge rejects it for every flight phase too.
        assert!(ground(&world, -1.0).is_none());
        assert!(ground(&world, -8.0).is_some());
        assert!(world.set_block_world(0, canopy_y, 8, Block::Air));
        assert_eq!(ground(&world, 0.5), original);
    }
}
