use super::*;

fn rain_spec(hit: AmbientHit) -> AmbientSpec {
    AmbientSpec {
        count_per_intensity: 200.0,
        max_count: 400,
        radius: 16.0,
        height: [4.0, 20.0],
        fall_speed: [16.0, 22.0],
        drift_wind: 1.0,
        flutter: [0.0, 0.0],
        size: [0.04, 0.06],
        stretch: 6.0,
        alpha: [0.4, 0.7],
        color: [[0.5, 0.6, 0.7], [0.7, 0.8, 0.9]],
        color_bias: 1.0,
        palette: Vec::new(),
        hit,
        motion: AmbientMotion::Precipitation,
        kill: AmbientKill::Ceiling,
        light: AmbientLight::Sky,
        biomes: Vec::new(),
        exclude_biomes: Vec::new(),
        biome_allow: None,
    }
}

fn splash_spec() -> BurstSpec {
    BurstSpec {
        count_per_intensity: 3.0,
        max_count: 24,
        up_speed: [1.0, 2.0],
        radial_speed: [0.5, 1.5],
        lifetime: [0.3, 0.5],
        size: [0.04, 0.08],
        color: [[0.2, 0.3, 0.8], [0.5, 0.7, 1.0]],
        color_bias: 1.0,
        die_on_contact: true,
    }
}

/// The shared 3×3-chunk fixture: a stone floor at y=64 (top face 65),
/// open sky above. The camera floats over the floor with the ground
/// INSIDE the fall band (`below` = 4 → band bottom 64 < floor top 65),
/// so drops really land.
const CAM: Vec3 = Vec3::new(8.0, 68.0, 8.0);
const FLOOR_TOP: f32 = 65.0;

#[test]
fn particles_stay_inside_the_volume_and_above_the_ground() {
    let world = petramond::world::testutil::flat_world();
    let spec = rain_spec(AmbientHit::Die);
    let mut out = Vec::new();
    let mut ceilings = FxHashMap::default();
    for step in 0..40 {
        let time = step as f32 * 0.05;
        out.clear();
        ceilings.clear();
        derive_volume(
            &spec,
            None,
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind: [0.0, 0.0],
                adv: [0.0, 0.0],
                biome_weight: None,
            },
            &View {
                world: &world,
                cam: CAM,
                time,
            },
            &mut ceilings,
            &mut out,
        );
        assert!(!out.is_empty(), "an active volume derives particles");
        for p in &out {
            assert!(
                p.pos.y > FLOOR_TOP - 0.001,
                "nothing renders below the floor top (y={})",
                p.pos.y
            );
            assert!(p.pos.y <= CAM.y + spec.height[1] + 0.001);
            let (dx, dz) = (p.pos.x - CAM.x, p.pos.z - CAM.z);
            assert!(
                dx * dx + dz * dz <= (spec.radius + 0.001).powi(2),
                "particles stay inside the radius disc"
            );
            assert_eq!(p.stretch, spec.stretch);
        }
    }
}

#[test]
fn covered_camera_derives_nothing_below_the_roof() {
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    // A floor at y=64 AND a roof at y=80; the camera stands between them.
    let mut world = petramond::world::World::new(0, 1);
    for cz in -1..=1 {
        for cx in -1..=1 {
            let mut c = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    c.set_block(x, 64, z, Block::Stone);
                    c.set_block(x, 80, z, Block::Stone);
                }
            }
            world.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
        }
    }
    let spec = rain_spec(AmbientHit::Die);
    let mut out = Vec::new();
    let mut ceilings = FxHashMap::default();
    for step in 0..40 {
        out.clear();
        ceilings.clear();
        derive_volume(
            &spec,
            None,
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind: [0.0, 0.0],
                adv: [0.0, 0.0],
                biome_weight: None,
            },
            &View {
                world: &world,
                cam: CAM,
                time: step as f32 * 0.07,
            },
            &mut ceilings,
            &mut out,
        );
        for p in &out {
            assert!(
                p.pos.y > 81.0 - 0.001,
                "under a roof nothing falls below its top (y={})",
                p.pos.y
            );
        }
    }
}

#[test]
fn splashes_appear_at_the_kill_height_shortly_after_hits() {
    let world = petramond::world::testutil::flat_world();
    let spec = rain_spec(AmbientHit::Burst("resolved-by-caller".into()));
    let splash = splash_spec();
    let mut splashes_seen = 0;
    let mut out = Vec::new();
    let mut ceilings = FxHashMap::default();
    for step in 0..200 {
        out.clear();
        ceilings.clear();
        derive_volume(
            &spec,
            Some(&splash),
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind: [0.0, 0.0],
                adv: [0.0, 0.0],
                biome_weight: None,
            },
            &View {
                world: &world,
                cam: CAM,
                time: step as f32 * 0.03,
            },
            &mut ceilings,
            &mut out,
        );
        for p in &out {
            // Splash droplets are the only rows without the rain stretch.
            if p.stretch == 1.0 {
                splashes_seen += 1;
                assert!(
                    p.pos.y >= FLOOR_TOP - 0.001 && p.pos.y < FLOOR_TOP + 1.5,
                    "droplets arc just above the floor top (y={})",
                    p.pos.y
                );
            }
        }
    }
    assert!(splashes_seen > 10, "steady rain shows steady splashes");
}

#[test]
fn jumping_does_not_restart_or_move_ground_splashes() {
    let world = petramond::world::testutil::flat_world();
    let spec = rain_spec(AmbientHit::Burst("resolved-by-caller".into()));
    let splash = splash_spec();
    let sample = |cam| {
        let mut out = Vec::new();
        derive_volume(
            &spec,
            Some(&splash),
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind: [0.0, 0.0],
                adv: [0.0, 0.0],
                biome_weight: None,
            },
            &View {
                world: &world,
                cam,
                time: 3.25,
            },
            &mut FxHashMap::default(),
            &mut out,
        );
        out.into_iter()
            .filter(|p| p.stretch == 1.0)
            .collect::<Vec<_>>()
    };
    let base = sample(CAM - Vec3::Y);
    let jumped = sample(CAM + Vec3::Y * 0.25);
    assert!(base.len() > 10, "the fixture must show active splashes");
    assert_eq!(base.len(), jumped.len());
    for p in &base {
        assert!(
            jumped
                .iter()
                .any(|q| { p.pos.distance(q.pos) < 1e-4 && (p.alpha - q.alpha).abs() < 1e-4 }),
            "jumping changed a splash position or age"
        );
    }
}

#[test]
fn drives_ease_in_and_retire_after_easing_out() {
    let world = petramond::world::testutil::flat_world();
    let mut drives = AmbientDrives::default();
    drives.set("weather", 200, 1.0, [0.0, 0.0]);
    // Unknown bundle id: the drive exists but derives nothing — inert.
    let mut out = Vec::new();
    drives.collect(&world, CAM, 0.0, 1.0, &mut out);
    assert!(out.is_empty());
    // Easing math: intensity approaches the target.
    let d = drives
        .drives
        .get("weather")
        .and_then(|m| m.get(&200))
        .unwrap();
    assert!(d.intensity < 1.0);
    for step in 1..200 {
        out.clear();
        drives.collect(&world, CAM, step as f32 * 0.1, 1.0, &mut out);
    }
    let d = drives
        .drives
        .get("weather")
        .and_then(|m| m.get(&200))
        .unwrap();
    assert!(d.intensity > 0.95, "intensity converges on the target");
    // Zero target: eases out, then the drive retires.
    drives.set("weather", 200, 0.0, [0.0, 0.0]);
    for step in 200..400 {
        out.clear();
        drives.collect(&world, CAM, step as f32 * 0.1, 1.0, &mut out);
    }
    assert!(drives.drives.is_empty(), "a zeroed drive retires");
    // A fresh zero-target set never creates a drive.
    drives.set("weather", 200, 0.0, [0.0, 0.0]);
    assert!(drives.drives.is_empty());
}

/// The round-3 regression guard: precipitation advects by the INTEGRAL
/// of the wind, so under constant wind a splash crown's anchor must stay
/// PUT across frames (the rewind exactly cancels the integral), and all
/// falling rows must keep the disc/band/floor invariants while the
/// integral wraps the camera box repeatedly. A revert to
/// `wind × absolute time` (the defect round 3 caught) drifts the anchors
/// and fails the cross-frame equality below.
#[test]
fn windy_advection_keeps_invariants_and_splash_anchors_static() {
    let world = petramond::world::testutil::flat_world();
    let spec = rain_spec(AmbientHit::Burst("resolved-by-caller".into()));
    // Zero launch speeds: droplets sit exactly ON their anchor for their
    // whole lifetime, so cross-frame anchor equality is directly
    // observable from the rows.
    let splash = BurstSpec {
        up_speed: [0.0, 0.0],
        radial_speed: [0.0, 0.0],
        ..splash_spec()
    };
    let diameter = spec.radius * 2.0;
    let mut adv = [0.0f32, 0.0];
    let mut ceilings = FxHashMap::default();
    let mut prev: Vec<(i32, i32)> = Vec::new();
    let mut prev_falling: Vec<(f32, f32)> = Vec::new();
    let mut crown_frames = 0;
    let mut displacement_frames = 0;
    let dt = 1.0 / 60.0;
    for step in 0..2000 {
        // The wind CHANGES mid-run — the whole point: starting from
        // adv=0 under constant wind, `wind × absolute time` equals the
        // integral and a revert to the round-3 defect would pass. After
        // the flip they diverge (the defect displaces every position by
        // Δwind × elapsed time; the integral glides).
        let wind = if step < 1000 {
            [5.0f32, -4.0]
        } else {
            [-3.0f32, 6.0]
        };
        let time = step as f32 * dt;
        adv[0] = (adv[0] + wind[0] * dt).rem_euclid(diameter);
        adv[1] = (adv[1] + wind[1] * dt).rem_euclid(diameter);
        let mut out = Vec::new();
        ceilings.clear();
        derive_volume(
            &spec,
            Some(&splash),
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind,
                adv,
                biome_weight: None,
            },
            &View {
                world: &world,
                cam: CAM,
                time,
            },
            &mut ceilings,
            &mut out,
        );
        let mut crowns: Vec<(i32, i32)> = Vec::new();
        let mut falling: Vec<(f32, f32)> = Vec::new();
        for p in &out {
            let (dx, dz) = (p.pos.x - CAM.x, p.pos.z - CAM.z);
            assert!(
                dx * dx + dz * dz <= (spec.radius + 0.001).powi(2),
                "row outside the disc under wind (step {step})"
            );
            assert!(p.pos.y > FLOOR_TOP - 0.001, "row below the floor");
            assert!(p.pos.y <= CAM.y + spec.height[1] + 0.001);
            if p.stretch == 1.0 {
                // Quantize to catch drift far above f32 noise but far
                // below one particle spacing.
                crowns.push((
                    (p.pos.x * 64.0).round() as i32,
                    (p.pos.z * 64.0).round() as i32,
                ));
            } else {
                falling.push((p.pos.x, p.pos.z));
            }
        }
        // THE regression signature: `wind × absolute time` displaces
        // every position by Δwind × session-age the moment the wind
        // changes (~2.5 blocks/frame here at the flip, vs ≤ ~0.6 for
        // legitimate fall+wind+flutter motion). Nearest-neighbour median
        // displacement of the falling rows stays small under the
        // integral, always.
        if !prev_falling.is_empty() && falling.len() >= 20 {
            let mut moved: Vec<f32> = falling
                .iter()
                .take(40)
                .map(|(x, z)| {
                    prev_falling
                        .iter()
                        .map(|(px, pz)| {
                            let (dx, dz) = (x - px, z - pz);
                            dx * dx + dz * dz
                        })
                        .fold(f32::INFINITY, f32::min)
                })
                .collect();
            moved.sort_by(f32::total_cmp);
            let median = moved[moved.len() / 2].sqrt();
            assert!(
                median < 1.2,
                "falling rows jumped {median} blocks in one frame (step {step}) — \
                     advection must be the wind INTEGRAL, never wind × absolute time"
            );
            displacement_frames += 1;
        }
        prev_falling = falling;
        let in_flip_window = (995..1075).contains(&step);
        if step > 10 && !in_flip_window && !prev.is_empty() && !crowns.is_empty() {
            crown_frames += 1;
            let stable = crowns.iter().filter(|c| prev.contains(c)).count();
            // Splash windows (~0.2-0.3 s) span many 60 fps frames, so
            // MOST crowns persist frame-to-frame at identical positions.
            assert!(
                stable * 2 >= crowns.len(),
                "crown anchors drift under wind (step {step}: {stable}/{})",
                crowns.len()
            );
        }
        prev = crowns;
    }
    assert!(
        crown_frames > 200,
        "the stability assertion must not be vacuous ({crown_frames} crown frames)"
    );
    assert!(
        displacement_frames > 1500,
        "the displacement assertion must not be vacuous ({displacement_frames})"
    );
}

/// A 3×3-chunk box: stone floor at y=64, stone roof at y=80, air between.
fn roofed_world() -> petramond::world::World {
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    let mut world = petramond::world::World::new(0, 1);
    for cz in -1..=1 {
        for cx in -1..=1 {
            let mut c = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    c.set_block(x, 64, z, Block::Stone);
                    c.set_block(x, 80, z, Block::Stone);
                }
            }
            world.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
        }
    }
    world
}

/// THE reason an interior volume exists. Every enclosed space in the world
/// sits under some column's precipitation ceiling, so a `kill: "ceiling"`
/// bundle derives NOTHING down there however it is tuned.
/// `kill: "interior"` fills the band around the camera regardless, and
/// still refuses the cells inside the rock.
#[test]
fn an_interior_volume_derives_where_precipitation_cannot() {
    let world = roofed_world();
    // The camera sits between the floor and the roof, the way a player
    // stands in a cave: the whole band is under the ceiling.
    let cam = Vec3::new(8.0, 70.0, 8.0);
    let mut spec = rain_spec(AmbientHit::Die);
    spec.height = [6.0, 6.0];
    let mut ceilings = FxHashMap::default();
    let mut counts = [0usize; 2];
    for (i, kill) in [AmbientKill::Ceiling, AmbientKill::Interior]
        .into_iter()
        .enumerate()
    {
        spec.kill = kill;
        for step in 0..20 {
            let mut out = Vec::new();
            ceilings.clear();
            derive_volume(
                &spec,
                None,
                &Activation {
                    seed: 7,
                    intensity: 1.0,
                    wind: [0.0, 0.0],
                    adv: [0.0, 0.0],
                    biome_weight: None,
                },
                &View {
                    world: &world,
                    cam,
                    time: step as f32 * 0.11,
                },
                &mut ceilings,
                &mut out,
            );
            counts[i] += out.len();
        }
    }
    assert_eq!(counts[0], 0, "precipitation cannot exist under a roof");
    assert!(
        counts[1] > 500,
        "an interior volume fills the band anyway, got {}",
        counts[1]
    );
    // …and it still refuses to draw inside the walls. The band's bottom
    // metre is the stone floor, so this is not a vacuous assertion.
    let mut out = Vec::new();
    ceilings.clear();
    derive_volume(
        &spec,
        None,
        &Activation {
            seed: 7,
            intensity: 1.0,
            wind: [0.0, 0.0],
            adv: [0.0, 0.0],
            biome_weight: None,
        },
        &View {
            world: &world,
            cam,
            time: 3.0,
        },
        &mut ceilings,
        &mut out,
    );
    assert!(
        out.iter().all(|p| p.pos.y >= 65.0),
        "no mote inside the floor"
    );
    assert!(
        out.len() < (spec.count_per_intensity * 0.79) as usize,
        "the floor must actually reject motes ({} of {})",
        out.len(),
        spec.count_per_intensity
    );
}

/// The lighting knob, which is what stops an interior volume from being a
/// field of glowing dots in a pitch-dark cave: `light: "world"` samples
/// the cell, `light: "sky"` keeps the precipitation constant.
#[test]
fn world_lit_motes_take_the_cells_own_light() {
    use petramond_world::chunk::SECTION_VOLUME;
    use petramond_world::light::{BlockLight6, LightRgb};
    let mut world = roofed_world();
    // Bake the band the camera stands in DARK, then flood ONE chunk's
    // section with a coloured emitter's light. A constant would pass a
    // dark-only assertion; only a positional sample can tell the two
    // halves of this fixture apart.
    let lamp = LightRgb::new(28, 6, 24);
    for cz in -1..=1i32 {
        for cx in -1..=1i32 {
            let section = world
                .section_at_world_mut_for_test(cx * 16, 70, cz * 16)
                .expect("the band's section is loaded");
            section.set_skylight(vec![0u8; SECTION_VOLUME].into());
            if (cx, cz) == (0, 0) {
                section.set_blocklight(vec![lamp; SECTION_VOLUME].into());
            }
        }
    }
    let cam = Vec3::new(8.0, 70.0, 8.0);
    let mut spec = rain_spec(AmbientHit::Die);
    spec.height = [6.0, 6.0];
    spec.kill = AmbientKill::Interior;
    let sample = |spec: &AmbientSpec| {
        let mut out = Vec::new();
        let mut ceilings = FxHashMap::default();
        derive_volume(
            spec,
            None,
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind: [0.0, 0.0],
                adv: [0.0, 0.0],
                biome_weight: None,
            },
            &View {
                world: &world,
                cam,
                time: 1.0,
            },
            &mut ceilings,
            &mut out,
        );
        assert!(!out.is_empty());
        out
    };
    spec.light = AmbientLight::Sky;
    for p in sample(&spec) {
        assert_eq!(p.skylight, SKY_OPEN_LIGHT);
        assert_eq!(p.blocklight, BlockLight6::DARK);
    }
    spec.light = AmbientLight::World;
    let (mut dark, mut lit) = (0usize, 0usize);
    for p in sample(&spec) {
        assert_eq!(p.skylight, 0, "a sealed room has no skylight");
        let in_lamp = (0.0..16.0).contains(&p.pos.x) && (0.0..16.0).contains(&p.pos.z);
        if in_lamp {
            assert_eq!(p.blocklight, BlockLight6::from_x2(lamp));
            lit += 1;
        } else {
            assert_eq!(
                p.blocklight,
                BlockLight6::DARK,
                "a mote outside the lit chunk must stay dark"
            );
            dark += 1;
        }
    }
    assert!(
        lit > 20 && dark > 20,
        "both halves populated ({lit}/{dark})"
    );
}

#[test]
fn ambient_particles_stay_world_anchored_when_the_camera_moves() {
    // Open air well above the fixture's floor: every mote survives, so
    // the two frames hold the same motes and nothing else can explain a
    // difference.
    let world = petramond::world::testutil::flat_world();
    let base_cam = Vec3::new(8.0, 120.0, 8.0);
    let mut spec = rain_spec(AmbientHit::Die);
    spec.count_per_intensity = 400.0;
    spec.max_count = 400;
    spec.height = [8.0, 10.0];
    spec.fall_speed = [0.22, 0.5];
    spec.flutter = [0.55, 0.06];
    spec.kill = AmbientKill::Interior;
    spec.light = AmbientLight::World;
    let span = spec.height[0] + spec.height[1];
    let time = 37.0;
    let sample = |spec: &AmbientSpec, cam: Vec3| {
        let mut out = Vec::new();
        let mut ceilings = FxHashMap::default();
        derive_volume(
            spec,
            None,
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind: [0.0, 0.0],
                adv: [0.0, 0.0],
                biome_weight: None,
            },
            &View {
                world: &world,
                cam,
                time,
            },
            &mut ceilings,
            &mut out,
        );
        assert!(out.len() > 100, "the fixture must derive a real field");
        out.iter().map(|p| p.pos).collect::<Vec<_>>()
    };
    // Fraction of `b` that still sits at a position present in `a`,
    // identified by the two axes the camera did not move along.
    let anchored = |a: &[Vec3], b: &[Vec3], axis: usize| -> f32 {
        let others: Vec<usize> = (0..3).filter(|i| *i != axis).collect();
        let kept = b
            .iter()
            .filter(|p| {
                a.iter().any(|q| {
                    others.iter().all(|i| (p[*i] - q[*i]).abs() < 1e-4)
                        && (p[axis] - q[axis]).abs() < 1e-3
                })
            })
            .count();
        kept as f32 / b.len() as f32
    };

    for motion in [AmbientMotion::Volume, AmbientMotion::Precipitation] {
        spec.motion = motion.clone();
        let jump = 1.25; // roughly a player's jump apex
        let base = sample(&spec, base_cam);
        let jumped = sample(&spec, base_cam + Vec3::new(0.0, jump, 0.0));
        let held = anchored(&base, &jumped, 1);
        // Only particles that wrap past the band edge may move.
        assert!(
            held > 1.0 - jump / span - 0.04,
            "{motion:?}: particles must keep their world height through a jump (kept {held})"
        );
        // Either way the body follows the player: still populated, still
        // inside the band and the disc around the NEW camera.
        let cam = base_cam + Vec3::new(0.0, jump, 0.0);
        for p in &jumped {
            assert!(
                p.y >= cam.y - spec.height[0] - 1e-3 && p.y <= cam.y + spec.height[1] + 1e-3,
                "mote outside the band around the new camera (y={})",
                p.y
            );
            let (dx, dz) = (p.x - cam.x, p.z - cam.z);
            assert!(dx * dx + dz * dz <= (spec.radius + 1e-3).powi(2));
        }
        // Control: the HORIZONTAL anchor both kinds already had. If the
        // helper could not detect anchoring at all, this would fail too.
        let strafed = sample(&spec, base_cam + Vec3::new(3.0, 0.0, 0.0));
        let held_x = anchored(&base, &strafed, 0);
        // A 3-block strafe swaps ~12% of the disc for fresh motes, so
        // this is loose by construction; it is here to prove the helper
        // can SEE anchoring, not to measure it.
        assert!(
            held_x > 0.75,
            "{motion:?}: motes must keep their world X when the camera strafes ({held_x})"
        );
    }
}

/// A volume must be populated wherever the player is, including far from
/// where it was last derived — the wrap, not a lattice, is what makes the
/// world anchor affordable.
#[test]
fn a_teleported_camera_still_stands_in_a_full_volume() {
    let world = petramond::world::testutil::flat_world();
    let mut spec = rain_spec(AmbientHit::Die);
    spec.motion = AmbientMotion::Volume;
    spec.height = [8.0, 10.0];
    spec.kill = AmbientKill::Interior;
    spec.light = AmbientLight::World;
    let mut ceilings = FxHashMap::default();
    // All well above the fixture's floor: below it the world reads as
    // virtual stone and the interior kill would (correctly) empty the band.
    for cam_y in [120.0f32, 200.0, 90.0] {
        let mut out = Vec::new();
        ceilings.clear();
        derive_volume(
            &spec,
            None,
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind: [0.0, 0.0],
                adv: [0.0, 0.0],
                biome_weight: None,
            },
            &View {
                world: &world,
                cam: Vec3::new(8.0, cam_y, 8.0),
                time: 101.0,
            },
            &mut ceilings,
            &mut out,
        );
        assert!(
            out.len() > 100,
            "the volume follows to y={cam_y} ({} motes)",
            out.len()
        );
        for p in &out {
            assert!(p.pos.y >= cam_y - spec.height[0] - 1e-3);
            assert!(p.pos.y <= cam_y + spec.height[1] + 1e-3);
        }
    }
}

/// The per-column biome filter: a bundle whose allow-set excludes the
/// fixture's biome derives NOTHING; the complement derives normally.
/// A biome-driven bundle thins every particle by its own column's
/// density: 0 draws nothing, 1 draws exactly the un-thinned set, and a
/// fraction keeps a proportional share of the same slots.
#[test]
fn biome_density_thins_a_driven_fall_per_particle() {
    let world = petramond::world::testutil::flat_world();
    let spec = rain_spec(AmbientHit::Die);
    let sample = |weight: Option<&dyn Fn(u8) -> f32>| {
        let mut out = Vec::new();
        derive_volume(
            &spec,
            None,
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind: [0.0, 0.0],
                adv: [0.0, 0.0],
                biome_weight: weight,
            },
            &View {
                world: &world,
                cam: CAM,
                time: 1.0,
            },
            &mut FxHashMap::default(),
            &mut out,
        );
        out
    };
    let full = sample(None);
    assert!(full.len() > 100);
    assert_eq!(
        sample(Some(&|_| 1.0)),
        full,
        "density 1 is the un-thinned set"
    );
    assert!(sample(Some(&|_| 0.0)).is_empty(), "density 0 draws nothing");
    let half = sample(Some(&|_| 0.5));
    let share = half.len() as f32 / full.len() as f32;
    assert!(
        (0.35..=0.65).contains(&share),
        "a fraction keeps its share ({share})"
    );
    assert!(
        half.iter().all(|p| full.contains(p)),
        "thinning removes particles, never moves them"
    );
}

#[test]
fn biome_filter_gates_columns() {
    let world = petramond::world::testutil::flat_world();
    let mut allowed = rain_spec(AmbientHit::Die);
    let mut denied = rain_spec(AmbientHit::Die);
    // The fixture's columns default to biome 0.
    allowed.biome_allow = Some([1u64, 0, 0, 0]); // bit 0 set
    denied.biome_allow = Some([!1u64, u64::MAX, u64::MAX, u64::MAX]);
    let mut ceilings = FxHashMap::default();
    for (spec, expect_some) in [(&allowed, true), (&denied, false)] {
        let mut out = Vec::new();
        ceilings.clear();
        derive_volume(
            spec,
            None,
            &Activation {
                seed: 7,
                intensity: 1.0,
                wind: [0.0, 0.0],
                adv: [0.0, 0.0],
                biome_weight: None,
            },
            &View {
                world: &world,
                cam: CAM,
                time: 1.0,
            },
            &mut ceilings,
            &mut out,
        );
        assert_eq!(
            !out.is_empty(),
            expect_some,
            "biome filter must gate the derive"
        );
    }
}
