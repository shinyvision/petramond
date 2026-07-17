use super::*;
use glam::Vec3;

fn inst(alpha: f32) -> ParticleInstance {
    ParticleInstance {
        pos: Vec3::new(1.0, 2.0, 3.0),
        uv_min: [0.1, 0.2],
        uv_size: 0.05,
        tint: [1.0, 1.0, 1.0],
        alpha,
        size: 0.1,
        skylight: lighting::FULL_SKYLIGHT,
        blocklight: 0,
    }
}

fn emitter_inst() -> ParticleEmitterInstance {
    ParticleEmitterInstance {
        origin: Vec3::new(1.0, 2.0, 3.0),
        emitter: crate::block::ParticleEmitter {
            anchor: crate::block::ParticleEmitterAnchor::BlockTop,
            origin: [0.5, 1.0, 0.5],
            offset: [0.0, 0.0, 0.0],
            rate: [1.0, 1.0],
            lifetime: [1.0, 1.0],
            size: [0.2, 0.2],
            spawn_box: [0.0, 0.0, 0.0],
            velocity: [0.0, 1.0, 0.0],
            velocity_jitter: [0.0, 0.0, 0.0],
            color: Some([[1.0, 0.5, 0.0], [1.0, 1.0, 0.0]]),
            color_ramp: None,
            alpha: [0.8, 0.8],
            fade_power: 2.0,
            shrink_power: 1.0,
            fullbright: true,
            spiral: [0.0, 0.0],
        },
        seed: 0x1234_5678_9ABC_DEF0,
        skylight: 0,
        blocklight: 0,
    }
}

fn one_live_emitter_time(inst: &ParticleEmitterInstance, age: f32) -> f32 {
    let schedule = emitter_schedule(inst.seed, inst.emitter.rate);
    // Sequence 10 keeps the test time positive for every phase in [0, 1).
    emitter_birth_time(inst.seed, schedule, 10) + age
}

fn vertex_center(v: &[ParticleVertex]) -> Vec3 {
    let sum = v.iter().fold(Vec3::ZERO, |acc, p| acc + Vec3::from(p.pos));
    sum / v.len() as f32
}

fn x_extent(v: &[ParticleVertex]) -> f32 {
    let min_x = v.iter().map(|p| p.pos[0]).fold(f32::INFINITY, f32::min);
    let max_x = v.iter().map(|p| p.pos[0]).fold(f32::NEG_INFINITY, f32::max);
    max_x - min_x
}

fn max_alpha(v: &[ParticleVertex]) -> f32 {
    v.iter().map(|p| p.alpha).fold(0.0, f32::max)
}

#[test]
fn each_visible_particle_is_one_cube() {
    let mut v = Vec::new();
    let n = build_particles(&[inst(1.0), inst(0.5)], &mut v);
    assert_eq!(
        n as usize,
        2 * VERTS_PER_CUBE,
        "two particles = two cubes = 48 verts"
    );
    assert_eq!(v.len(), 2 * VERTS_PER_CUBE);
    // Alpha is carried per vertex.
    assert_eq!(v[0].alpha, 1.0);
    assert_eq!(v[VERTS_PER_CUBE].alpha, 0.5);
}

#[test]
fn tint_is_carried_to_every_vertex() {
    let green = ParticleInstance {
        tint: [0.5, 0.72, 0.38],
        ..inst(1.0)
    };
    let mut v = Vec::new();
    build_particles(std::slice::from_ref(&green), &mut v);
    assert_eq!(v.len(), VERTS_PER_CUBE);
    for vert in &v {
        assert_eq!(
            vert.tint,
            [0.5, 0.72, 0.38],
            "every cube vertex carries the tint"
        );
    }
}

#[test]
fn faces_carry_distinct_directional_shades() {
    let mut v = Vec::new();
    build_particles(std::slice::from_ref(&inst(1.0)), &mut v);
    // Top face (index 2) is brightest, bottom (index 3) darkest.
    let top = v[2 * 4].shade;
    let bottom = v[3 * 4].shade;
    let side = v[0].shade;
    assert!(
        top > side && side > bottom,
        "top > side > bottom shading reads 3D"
    );
    assert_eq!(top, 1.0);
}

#[test]
fn sampled_light_folds_into_the_particle_tint() {
    // The two-channel RGB light rides the vertex TINT (shade keeps only the
    // directional term), so a dark sample dims the tint, not the shade.
    let mut v = Vec::new();
    let dark = ParticleInstance {
        skylight: 0,
        ..inst(1.0)
    };

    build_particles(std::slice::from_ref(&dark), &mut v);

    assert_eq!(v[2 * 4].shade, 1.0, "shade stays directional-only");
    let expect = lighting::light_rgb(DynLight { sky: 0, block: 0 }, LightEnv::IDENTITY);
    assert_eq!(v[2 * 4].tint, expect, "unlit sample dims the tint");
    assert!(expect[0] < 1.0);
}

#[test]
fn block_emitter_particles_rise_shrink_and_fade() {
    let inst = emitter_inst();
    let mut young = Vec::new();
    let mut old = Vec::new();
    let mut scratch = Vec::new();

    let young_n = build_transparent_emitter_particles(
        std::slice::from_ref(&inst),
        &[],
        one_live_emitter_time(&inst, 0.25),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut young,
        &mut scratch,
    );
    let old_n = build_transparent_emitter_particles(
        std::slice::from_ref(&inst),
        &[],
        one_live_emitter_time(&inst, 0.75),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut old,
        &mut scratch,
    );

    assert_eq!(young_n as usize, VERTS_PER_CUBE);
    assert_eq!(old_n as usize, VERTS_PER_CUBE);
    assert!(
        vertex_center(&old).y > vertex_center(&young).y,
        "emitter particles move upward over their lifetime"
    );
    assert!(
        x_extent(&old) < x_extent(&young),
        "emitter particles shrink as they age"
    );
    assert!(
        max_alpha(&old) < max_alpha(&young),
        "emitter particles fade as they age"
    );
}

#[test]
fn spiral_emitter_particles_orbit_the_vertical_axis_as_they_age() {
    let mut inst = emitter_inst();
    inst.emitter.spiral = [0.5, 1.0]; // one revolution per second at 0.5 blocks
    let mut early = Vec::new();
    let mut late = Vec::new();
    let mut scratch = Vec::new();

    // A quarter revolution apart: the particle's horizontal offset from the
    // emitter axis must keep its radius but rotate to a different angle.
    build_transparent_emitter_particles(
        std::slice::from_ref(&inst),
        &[],
        one_live_emitter_time(&inst, 0.25),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut early,
        &mut scratch,
    );
    build_transparent_emitter_particles(
        std::slice::from_ref(&inst),
        &[],
        one_live_emitter_time(&inst, 0.5),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut late,
        &mut scratch,
    );

    let axis = inst.origin;
    let horiz = |v: &[ParticleVertex]| {
        let c = vertex_center(v);
        Vec3::new(c.x - axis.x, 0.0, c.z - axis.z)
    };
    let (a, b) = (horiz(&early), horiz(&late));
    // Orbit radius is per-particle (60-100% of the row's 0.5) but stable
    // over one particle's life: both samples see the same particle.
    assert!(
        (a.length() - b.length()).abs() < 1e-4,
        "one particle keeps its orbit radius: {} vs {}",
        a.length(),
        b.length()
    );
    assert!(
        (0.3 - 1e-4..=0.5 + 1e-4).contains(&a.length()),
        "orbit radius stays within the row's spiral radius: {}",
        a.length()
    );
    assert!(
        a.angle_between(b) > 0.3,
        "a quarter nominal revolution rotates the offset (angle {})",
        a.angle_between(b)
    );
}

#[test]
fn ramp_emitter_particles_cool_through_the_ramp_as_they_age() {
    let mut inst = emitter_inst();
    let ramp: crate::block::ColorRamp =
        serde_json::from_str(r#"[[1.0, 1.0, 0.9], [1.0, 0.5, 0.1], [0.1, 0.1, 0.1]]"#)
            .expect("3-stop ramp parses");
    inst.emitter.color = None;
    inst.emitter.color_ramp = Some(ramp);
    let mut young = Vec::new();
    let mut old = Vec::new();
    let mut scratch = Vec::new();

    build_transparent_emitter_particles(
        std::slice::from_ref(&inst),
        &[],
        one_live_emitter_time(&inst, 0.1),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut young,
        &mut scratch,
    );
    build_transparent_emitter_particles(
        std::slice::from_ref(&inst),
        &[],
        one_live_emitter_time(&inst, 0.9),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut old,
        &mut scratch,
    );

    let luma = |v: &[ParticleVertex]| {
        let t = v[0].tint;
        t[0] + t[1] + t[2]
    };
    assert!(
        luma(&young) > 2.0,
        "a young particle sits near the hot end of the ramp: {}",
        luma(&young)
    );
    assert!(
        luma(&old) < 1.0,
        "an old particle has cooled toward the dark end: {}",
        luma(&old)
    );
}

#[test]
fn lower_fade_power_keeps_late_life_particles_more_visible() {
    let quick = emitter_inst();
    let mut lingering = emitter_inst();
    lingering.emitter.fade_power = 1.0;
    let mut a = Vec::new();
    let mut b = Vec::new();
    let mut scratch = Vec::new();

    build_transparent_emitter_particles(
        std::slice::from_ref(&quick),
        &[],
        one_live_emitter_time(&quick, 0.75),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut a,
        &mut scratch,
    );
    build_transparent_emitter_particles(
        std::slice::from_ref(&lingering),
        &[],
        one_live_emitter_time(&lingering, 0.75),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut b,
        &mut scratch,
    );

    assert!(
        max_alpha(&b) > max_alpha(&a),
        "fade_power 1 lingers longer than the default quadratic: {} vs {}",
        max_alpha(&b),
        max_alpha(&a)
    );
}

#[test]
fn lower_shrink_power_keeps_late_life_particles_larger() {
    let linear = emitter_inst();
    let mut chunky = emitter_inst();
    chunky.emitter.shrink_power = 0.4;
    let mut a = Vec::new();
    let mut b = Vec::new();
    let mut scratch = Vec::new();

    build_transparent_emitter_particles(
        std::slice::from_ref(&linear),
        &[],
        one_live_emitter_time(&linear, 0.75),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut a,
        &mut scratch,
    );
    build_transparent_emitter_particles(
        std::slice::from_ref(&chunky),
        &[],
        one_live_emitter_time(&chunky, 0.75),
        Vec3::ZERO,
        LightEnv::IDENTITY,
        1.0,
        &mut b,
        &mut scratch,
    );

    assert!(
        x_extent(&b) > x_extent(&a),
        "shrink_power 0.4 keeps an old cube chunkier than linear shrink: {} vs {}",
        x_extent(&b),
        x_extent(&a)
    );
}

#[test]
fn block_emitter_rate_range_jitters_spawn_intervals() {
    let seed = 0xCAFE_BABE_F00D_1234;
    let schedule = emitter_schedule(seed, [1.0, 2.0]);
    let mut saw_variation = false;
    let mut prev = emitter_birth_time(seed, schedule, 0);

    for seq in 1..64 {
        let birth = emitter_birth_time(seed, schedule, seq);
        let gap = birth - prev;
        assert!(
            (0.5..=1.0).contains(&gap),
            "rate range 1.0-2.0 should produce gaps in 0.5-1.0 seconds, got {gap}"
        );
        saw_variation |= (gap - schedule.base_gap).abs() > 0.01;
        prev = birth;
    }

    assert!(
        saw_variation,
        "range emitters should not spawn on a fixed cadence"
    );
}

#[test]
fn fully_faded_particles_are_skipped() {
    let mut v = Vec::new();
    let n = build_particles(&[inst(0.0), inst(1.0)], &mut v);
    assert_eq!(
        n as usize, VERTS_PER_CUBE,
        "the alpha=0 particle is dropped"
    );
}

#[test]
fn cube_is_centred_on_pos() {
    let mut v = Vec::new();
    build_particles(std::slice::from_ref(&inst(1.0)), &mut v);
    let cx: f32 = v.iter().map(|p| p.pos[0]).sum::<f32>() / v.len() as f32;
    let cy: f32 = v.iter().map(|p| p.pos[1]).sum::<f32>() / v.len() as f32;
    let cz: f32 = v.iter().map(|p| p.pos[2]).sum::<f32>() / v.len() as f32;
    assert!((cx - 1.0).abs() < 1e-5 && (cy - 2.0).abs() < 1e-5 && (cz - 3.0).abs() < 1e-5);
}

#[test]
fn cube_extent_matches_size() {
    let mut v = Vec::new();
    build_particles(std::slice::from_ref(&inst(1.0)), &mut v);
    let min_x = v.iter().map(|p| p.pos[0]).fold(f32::INFINITY, f32::min);
    let max_x = v.iter().map(|p| p.pos[0]).fold(f32::NEG_INFINITY, f32::max);
    // Side length == size (0.1), so extent on each axis is the full size.
    assert!(
        (max_x - min_x - 0.1).abs() < 1e-5,
        "cube spans `size` on each axis"
    );
}

#[test]
fn faces_are_offset_to_the_cube_surface_not_the_centre() {
    // Regression for the "star/+" bug: every face used to pass through the
    // cube centre (corners = c +/- r +/- up). A real cube has each face
    // offset outward by `normal*h`, giving 8 distinct corner positions.
    let mut v = Vec::new();
    build_particles(std::slice::from_ref(&inst(1.0)), &mut v);
    let c = Vec3::new(1.0, 2.0, 3.0);
    let h = 0.1 * 0.5; // size 0.1
                       // +X face is FACES[0]; its 4 verts must all sit on the +X plane
                       // (x=c.x+h), NOT through the centre (x=c.x). -X face (FACES[1]) sits at
                       // x=c.x-h.
    for i in 0..4 {
        assert!(
            (v[i].pos[0] - (c.x + h)).abs() < 1e-6,
            "+X face on the +X surface"
        );
        assert!(
            (v[4 + i].pos[0] - (c.x - h)).abs() < 1e-6,
            "-X face on the -X surface"
        );
    }
    // +Y / -Y faces (FACES[2], [3]) on the top/bottom planes.
    for i in 0..4 {
        assert!(
            (v[8 + i].pos[1] - (c.y + h)).abs() < 1e-6,
            "+Y face on the top surface"
        );
        assert!(
            (v[12 + i].pos[1] - (c.y - h)).abs() < 1e-6,
            "-Y face on the bottom surface"
        );
    }
    // +Z / -Z faces (FACES[4], [5]) on the front/back planes.
    for i in 0..4 {
        assert!(
            (v[16 + i].pos[2] - (c.z + h)).abs() < 1e-6,
            "+Z face on the +Z surface"
        );
        assert!(
            (v[20 + i].pos[2] - (c.z - h)).abs() < 1e-6,
            "-Z face on the -Z surface"
        );
    }
    // A real cube has exactly 8 distinct corner positions (the 24 verts are
    // the 8 corners shared 3 ways). The buggy star had only 6 (centre-crossed
    // squares share the 4 mid-edge points differently); assert 8 here.
    let mut corners: Vec<[i32; 3]> = v
        .iter()
        .map(|p| {
            [
                (p.pos[0] * 1e4) as i32,
                (p.pos[1] * 1e4) as i32,
                (p.pos[2] * 1e4) as i32,
            ]
        })
        .collect();
    corners.sort_unstable();
    corners.dedup();
    assert_eq!(
        corners.len(),
        8,
        "a real cube has 8 distinct corner positions"
    );
}

#[test]
fn caps_at_capacity_and_reuses_buffer() {
    let mut v = Vec::new();
    let many = vec![inst(1.0); MAX_PARTICLE_CUBES + 100];
    let n = build_particles(&many, &mut v);
    assert_eq!(
        n as usize, MAX_PARTICLE_VERTICES,
        "capped at the vertex budget"
    );
    let cap = v.capacity();
    // Same input -> identical (capped) vert count, so the cleared+refilled
    // buffer keeps its capacity: rebuilding to the same size never reallocs.
    let n = build_particles(&many, &mut v);
    assert_eq!(n as usize, MAX_PARTICLE_VERTICES);
    assert_eq!(v.capacity(), cap, "vert buffer reused");
}

#[test]
fn index_buffer_is_thirtysix_per_cube() {
    let idx = particle_indices();
    assert_eq!(idx.len(), MAX_PARTICLE_INDICES);
    // First face of first cube: 0,1,2, 0,2,3.
    assert_eq!(&idx[..6], &[0, 1, 2, 0, 2, 3]);
    // Second face starts at vertex 4.
    assert_eq!(&idx[6..12], &[4, 5, 6, 4, 6, 7]);
    // Second cube starts at vertex 24.
    let c2 = INDICES_PER_CUBE;
    assert_eq!(&idx[c2..c2 + 6], &[24, 25, 26, 24, 26, 27]);
}
