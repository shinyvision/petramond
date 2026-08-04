use super::*;

/// No solid surfaces (particles never hit ground).
fn empty(_p: Vec3) -> bool {
    false
}

/// Every fleck patch must stay STRICTLY inside its tile rect on both axes and
/// both edges, for every tile and both atlas halves (base + dye twin). A patch
/// touching a tile boundary samples the ADJACENT tile under nearest filtering
/// (the iron-ore-flashes-magenta-flower bug: the old scalar `uv_size` scaled V
/// by tile WIDTH, overshooting into the tile below in the double-height atlas).
#[test]
fn atlas_uv_stays_strictly_inside_the_tile_rect() {
    // Quarter of a base texel: comfortably above f32 rounding error at these
    // magnitudes, below the half-texel inset the mapping guarantees.
    let quarter_texel = 0.25 / atlas::TILE as f32;
    for tile in Tile::all() {
        let [u0, v0, u1, v1] = atlas::tile_uv(tile);
        let (tw, th) = (u1 - u0, v1 - v0);
        // Patch origins at the spawner's extremes (`patch_min` yields
        // `[0, 1 - PATCH_FRAC)`; 1 - PATCH_FRAC itself is even more adverse).
        for m in [0.0, 1.0 - PATCH_FRAC] {
            for dyed in [false, true] {
                let p = Particle {
                    pos: Vec3::ZERO,
                    vel: Vec3::ZERO,
                    skylight: 63,
                    blocklight: petramond_world::light::BlockLight6::DARK,
                    tile,
                    model: None,
                    dyed,
                    uv_min: [m, m],
                    uv_size: [PATCH_FRAC; 2],
                    tint: NO_TINT,
                    solid: false,
                    die_on_contact: false,
                    age: 0.0,
                    lifetime: 1.0,
                    size: 0.1,
                };
                let (abs_min, abs_size) = p.atlas_uv();
                let dye = if dyed { atlas::DYE_V_OFFSET } else { 0.0 };
                let name = tile.name();
                assert!(
                    abs_min[0] > u0 + quarter_texel * tw
                        && abs_min[0] + abs_size[0] < u1 - quarter_texel * tw,
                    "{name} dyed={dyed} m={m}: U patch [{}, {}] escapes tile [{u0}, {u1}]",
                    abs_min[0],
                    abs_min[0] + abs_size[0],
                );
                assert!(
                    abs_min[1] > v0 + dye + quarter_texel * th
                        && abs_min[1] + abs_size[1] < v1 + dye - quarter_texel * th,
                    "{name} dyed={dyed} m={m}: V patch [{}, {}] escapes tile [{}, {}]",
                    abs_min[1],
                    abs_min[1] + abs_size[1],
                    v0 + dye,
                    v1 + dye,
                );
            }
        }
    }
}

#[test]
fn alpha_fades_at_end_of_life() {
    let mut p = Particle {
        pos: Vec3::ZERO,
        vel: Vec3::ZERO,
        skylight: 63,
        blocklight: petramond_world::light::BlockLight6::DARK,
        tile: Tile::from_name("grass_top").unwrap(),
        model: None,
        dyed: false,
        uv_min: [0.0, 0.0],
        uv_size: [0.25; 2],
        tint: NO_TINT,
        solid: false,
        die_on_contact: false,
        age: 0.0,
        lifetime: 1.0,
        size: 0.1,
    };
    assert_eq!(p.alpha(), 1.0, "young particle is opaque");
    p.age = 0.5;
    assert_eq!(p.alpha(), 1.0, "still inside the solid phase");
    p.age = 1.0;
    assert!(p.alpha() <= 1e-6, "fully aged is transparent");
    p.age = 0.8; // 80% through a 40% tail → 0.5
    assert!(
        (p.alpha() - 0.5).abs() < 1e-3,
        "mid-fade ~0.5, got {}",
        p.alpha()
    );
}

#[test]
fn render_size_shrinks_during_fade() {
    let mut p = Particle {
        pos: Vec3::ZERO,
        vel: Vec3::ZERO,
        skylight: 63,
        blocklight: petramond_world::light::BlockLight6::DARK,
        tile: Tile::from_name("grass_top").unwrap(),
        model: None,
        dyed: false,
        uv_min: [0.0, 0.0],
        uv_size: [0.25; 2],
        tint: NO_TINT,
        solid: false,
        die_on_contact: false,
        age: 0.0,
        lifetime: 1.0,
        size: 0.1,
    };
    assert!(
        (p.render_size() - 0.1).abs() < 1e-6,
        "young particle is full size"
    );
    p.age = 0.8; // mid-fade, alpha ~0.5
    assert!(
        (p.render_size() - 0.05).abs() < 1e-3,
        "shrinks with the fade curve"
    );
    p.age = 1.0;
    assert!(p.render_size() <= 1e-6, "fully aged collapses to nothing");
}

#[test]
fn spawn_mining_emits_two_to_four() {
    let mut sys = ParticleSystem::new();
    let before = sys.len();
    sys.spawn_mining(IVec3::new(0, 64, 0), IVec3::Y, Block::Stone);
    let n = sys.len() - before;
    assert!((2..=4).contains(&n), "mining emits 2-4 particles, got {n}");
    for p in sys.particles() {
        assert!(
            (0.5..=1.5).contains(&p.lifetime),
            "mining lifetime 0.5-1.5s"
        );
    }
}

#[test]
fn spawn_break_burst_emits_a_handful() {
    let mut sys = ParticleSystem::new();
    sys.spawn_break_burst(IVec3::new(1, 2, 3), Block::Dirt);
    let n = sys.len();
    assert!(
        (16..=32).contains(&n),
        "burst emits 16-32 particles, got {n}"
    );
    for p in sys.particles() {
        assert!((1.0..=3.0).contains(&p.lifetime), "burst lifetime 1-3s");
    }
}

#[test]
fn particle_passes_inset_margin_but_stops_in_the_box() {
    // Model-aware: a fleck drifting through the empty SIDE MARGIN of an inset/model cell
    // keeps moving; one dropping into the actual box stops. Proves particles settle on
    // the real shape (`point_in_solid` / `World::point_blocked`), not the full cell.
    let chest = Block::Chest.collision_boxes(); // inset: x/z in [1/16, 15/16]
    let chest_top = chest.iter().map(|b| b.max[1]).fold(0.0, f32::max);
    let blocked = |p: Vec3| {
        petramond_world::collision::point_in_solid(
            [p.x, p.y, p.z],
            |_x, y, _z| if y == 0 { chest } else { &[][..] },
        )
    };
    let fleck = |pos: Vec3, vel: Vec3| Particle {
        pos,
        vel,
        skylight: 63,
        blocklight: petramond_world::light::BlockLight6::DARK,
        tile: Tile::from_name("grass_top").unwrap(),
        model: None,
        dyed: false,
        uv_min: [0.0; 2],
        uv_size: [0.1; 2],
        tint: NO_TINT,
        solid: false,
        die_on_contact: false,
        age: 0.0,
        lifetime: 100.0,
        size: 0.1,
    };
    // In the 1/16 side margin (x = 0.02, left of the inset face at 1/16): falls through.
    let mut sys = ParticleSystem::new();
    sys.push(fleck(Vec3::new(0.02, 0.5, 0.5), Vec3::new(0.0, -1.0, 0.0)));
    let y0 = sys.particles()[0].pos.y;
    sys.tick_with(0.05, &blocked, &empty);
    assert!(
        sys.particles()[0].pos.y < y0,
        "a fleck in the side margin keeps falling"
    );
    // Centred, dropping just into the box top: stops dead on the surface.
    let mut hit = ParticleSystem::new();
    hit.push(fleck(
        Vec3::new(0.5, chest_top + 0.02, 0.5),
        Vec3::new(0.0, -1.0, 0.0),
    ));
    hit.tick_with(0.05, &blocked, &empty);
    assert_eq!(
        hit.particles()[0].vel,
        Vec3::ZERO,
        "a fleck entering the box stops"
    );
}

#[test]
fn grass_top_mining_dust_is_green_but_dirt_side_is_not() {
    let grass = Biome::Plains.grass_color();
    // Mining the grass-block TOP samples GrassTop -> green flecks.
    let mut sys = ParticleSystem::new();
    sys.spawn_mining(IVec3::new(0, 64, 0), IVec3::Y, Block::Grass);
    assert!(!sys.is_empty());
    for p in sys.particles() {
        assert_eq!(p.tile, Tile::from_name("grass_top").unwrap());
        assert_eq!(p.tint, grass, "grass-top dust must be tinted green");
    }
    // Mining a grass-block SIDE samples the pre-baked GrassSide tile -> no tint.
    let mut side = ParticleSystem::new();
    side.spawn_mining(IVec3::new(0, 64, 0), IVec3::new(1, 0, 0), Block::Grass);
    for p in side.particles() {
        assert_eq!(p.tile, Tile::from_name("grass_side").unwrap());
        assert_eq!(p.tint, NO_TINT, "grass-block side dust stays untinted");
    }
    // A plain non-foliage block is never tinted on any face.
    let mut stone = ParticleSystem::new();
    stone.spawn_mining(IVec3::new(0, 64, 0), IVec3::Y, Block::Stone);
    for p in stone.particles() {
        assert_eq!(p.tint, NO_TINT, "stone dust stays untinted");
    }
}

#[test]
fn leaf_burst_flecks_carry_the_foliage_tint() {
    let foliage = Biome::Plains.foliage_color();
    let mut sys = ParticleSystem::new();
    sys.spawn_break_burst(IVec3::ZERO, Block::OakLeaves);
    assert!(!sys.is_empty());
    // Leaves use the same tile on every face, so every fleck is foliage-tinted.
    for p in sys.particles() {
        assert_eq!(p.tint, foliage, "leaf fleck must carry the foliage tint");
    }
}

fn splash_spec() -> petramond_world::particle_emitters::BurstSpec {
    petramond_world::particle_emitters::BurstSpec {
        count_per_intensity: 4.0,
        max_count: 20,
        up_speed: [1.5, 3.5],
        radial_speed: [0.5, 2.0],
        lifetime: [0.5, 1.0],
        size: [0.05, 0.11],
        color: [[0.05, 0.1, 0.5], [0.3, 0.7, 0.95]],
        color_bias: 2.5,
        die_on_contact: true,
    }
}

#[test]
fn emitter_burst_count_scales_with_intensity_and_caps() {
    let spec = splash_spec();
    let mut small = ParticleSystem::new();
    small.spawn_emitter_burst(&spec, Vec3::ZERO, 2.0, 63, petramond_world::light::BlockLight6::DARK);
    assert_eq!(small.len(), 8, "4 per intensity unit × 2");
    let mut big = ParticleSystem::new();
    big.spawn_emitter_burst(
        &spec,
        Vec3::ZERO,
        100.0,
        63,
        petramond_world::light::BlockLight6::DARK,
    );
    assert_eq!(big.len(), 20, "hard-capped at max_count");
    for p in big.particles() {
        assert!(p.solid && p.die_on_contact);
        assert!(p.vel.y >= 1.5 && p.vel.y <= 3.5, "launched upward");
        let radial = Vec3::new(p.vel.x, 0.0, p.vel.z).length();
        assert!(
            (0.5..=2.0).contains(&radial),
            "launched outward in the radial band: {radial}"
        );
    }
}

#[test]
fn burst_color_bias_favors_the_first_endpoint() {
    let spec = splash_spec(); // bias 2.5 toward the deep first endpoint
    let mut sys = ParticleSystem::new();
    for _ in 0..10 {
        sys.spawn_emitter_burst(&spec, Vec3::ZERO, 5.0, 63, petramond_world::light::BlockLight6::DARK);
    }
    // mix^2.5 has mean ~0.29: most droplets sit nearer color[0] (deep).
    let mean_g: f32 = sys.particles().iter().map(|p| p.tint[1]).sum::<f32>() / sys.len() as f32;
    let mid_g = (spec.color[0][1] + spec.color[1][1]) * 0.5;
    assert!(
        mean_g < mid_g,
        "biased mix leans toward the first endpoint: mean {mean_g} vs midpoint {mid_g}"
    );
}

#[test]
fn die_on_contact_particles_vanish_on_blocks_and_water() {
    let spec = splash_spec();
    let floor = |p: Vec3| p.y < 0.0;
    let pool = |p: Vec3| p.y < 0.0;
    let none = |_: Vec3| false;

    // Falling onto a solid: a contact-dying droplet is culled, while an
    // ordinary fleck would have settled (velocity zeroed, still alive).
    let mut sys = ParticleSystem::new();
    sys.spawn_emitter_burst(
        &spec,
        Vec3::new(0.0, 0.05, 0.0),
        1.0,
        63,
        petramond_world::light::BlockLight6::DARK,
    );
    for p in &mut sys.particles {
        p.vel = Vec3::new(0.0, -2.0, 0.0); // force straight down
    }
    sys.tick_with(0.1, &floor, &none);
    assert!(sys.is_empty(), "droplets die on solid contact");

    // Falling into water: same instant destruction.
    let mut wet = ParticleSystem::new();
    wet.spawn_emitter_burst(
        &spec,
        Vec3::new(0.0, 0.05, 0.0),
        1.0,
        63,
        petramond_world::light::BlockLight6::DARK,
    );
    for p in &mut wet.particles {
        p.vel = Vec3::new(0.0, -2.0, 0.0);
    }
    wet.tick_with(0.1, &none, &pool);
    assert!(wet.is_empty(), "droplets die on water contact");

    // Ordinary dust on the same floor settles instead of dying.
    let mut dust = ParticleSystem::new();
    dust.spawn_break_burst(IVec3::new(0, 0, 0), Block::Stone);
    for p in &mut dust.particles {
        p.pos = Vec3::new(0.0, 0.05, 0.0);
        p.vel = Vec3::new(0.0, -2.0, 0.0);
    }
    dust.tick_with(0.1, &floor, &pool);
    assert!(!dust.is_empty(), "dust settles rather than dying");
    assert!(dust.particles().iter().all(|p| p.vel == Vec3::ZERO));
}

#[test]
fn tick_ages_and_culls_dead() {
    let mut sys = ParticleSystem::new();
    sys.spawn_break_burst(IVec3::ZERO, Block::Dirt);
    assert!(!sys.is_empty());
    // Step past the maximum lifetime (3 s) so all are culled.
    for _ in 0..400 {
        sys.tick_with(0.01, &empty, &empty);
    }
    assert!(
        sys.is_empty(),
        "all particles should be culled after lifetime"
    );
}

#[test]
fn respects_fixed_capacity() {
    let mut sys = ParticleSystem::new();
    // Spawn far more than capacity; the pool must never exceed PARTICLE_CAPACITY.
    for _ in 0..1000 {
        sys.spawn_break_burst(IVec3::ZERO, Block::Stone);
        assert!(
            sys.len() <= PARTICLE_CAPACITY,
            "exceeded capacity: {}",
            sys.len()
        );
    }
    assert_eq!(
        sys.len(),
        PARTICLE_CAPACITY,
        "pool should saturate at capacity"
    );
    // The backing Vec never grew past its reserved capacity (no realloc churn).
    assert_eq!(sys.particles.capacity(), PARTICLE_CAPACITY);
}

#[test]
fn particles_fall_under_gravity() {
    let mut sys = ParticleSystem::new();
    sys.spawn_break_burst(IVec3::new(0, 100, 0), Block::Dirt);
    let y_before: f32 = sys.particles().iter().map(|p| p.pos.y).sum::<f32>() / sys.len() as f32;
    for _ in 0..30 {
        sys.tick_with(1.0 / 60.0, &empty, &empty);
    }
    let y_after: f32 = sys.particles().iter().map(|p| p.pos.y).sum::<f32>() / sys.len() as f32;
    assert!(
        y_after < y_before,
        "gravity should lower particles on average"
    );
}
