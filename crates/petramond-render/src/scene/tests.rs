use super::*;
use petramond_math::world_pos::WorldPos;
use petramond_world::item::ItemType;

fn fresh_drop(pos: WorldPos, item: ItemType) -> DroppedItemPresentation {
    DroppedItemPresentation {
        variant: petramond_world::item::VariantId::NONE,
        prev_pos: pos,
        pos,
        item,
        count: 1,
        prev_spin: 0.0,
        spin: 0.0,
        prev_flight: None,
        flight: None,
        skylight: 0,
        blocklight: petramond_world::light::BlockLight6::DARK,
    }
}

fn particle_row(atlas: ParticleAtlas) -> ParticlePresentation {
    ParticlePresentation {
        quad_axes: None,
        atlas,
        pos: WorldPos::new(0.0, 64.0, 0.0),
        uv_min: [0.0, 0.0],
        uv_size: [0.0625; 2],
        tint: [1.0, 1.0, 1.0],
        alpha: 1.0,
        size: 0.1,
        stretch: 1.0,
        skylight: 0,
        blocklight: petramond_world::light::BlockLight6::DARK,
    }
}

#[test]
fn bake_item_entities_one_instance_per_drop() {
    let drops = vec![
        fresh_drop(WorldPos::new(1.0, 2.0, 3.0), ItemType::Dirt),
        fresh_drop(WorldPos::new(4.0, 5.0, 6.0), ItemType::Stone),
    ];
    let mut out = Vec::new();
    bake_item_entities(&drops, 1.0, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].pos, drops[0].pos);
    assert_eq!(out[0].item, ItemType::Dirt);
    assert_eq!(out[1].item, ItemType::Stone);
}

#[test]
fn bake_item_entities_interpolates_between_ticks() {
    let drop = DroppedItemPresentation {
        prev_pos: WorldPos::new(0.0, 64.0, 0.0),
        pos: WorldPos::new(2.0, 64.0, 0.0),
        ..fresh_drop(WorldPos::ZERO, ItemType::Dirt)
    };
    let mut out = Vec::new();
    bake_item_entities(std::slice::from_ref(&drop), 0.5, &mut out);
    assert_eq!(
        out[0].pos,
        WorldPos::new(1.0, 64.0, 0.0),
        "halfway between prev and current"
    );
}

#[test]
fn bake_item_entities_reuses_the_vec_without_growth() {
    let drops: Vec<_> = (0..8)
        .map(|i| {
            fresh_drop(
                petramond_math::world_pos::WorldPos::new(i as f64, i as f64, i as f64),
                ItemType::Dirt,
            )
        })
        .collect();
    let mut out = Vec::new();
    bake_item_entities(&drops, 1.0, &mut out);
    let cap = out.capacity();
    bake_item_entities(&drops[..2], 1.0, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out.capacity(), cap, "instance buffer reused");
}

#[test]
fn bake_particles_splits_rows_by_atlas() {
    let particles = vec![
        particle_row(ParticleAtlas::Block),
        particle_row(ParticleAtlas::Model),
        particle_row(ParticleAtlas::Block),
        particle_row(ParticleAtlas::Solid),
    ];
    let mut block_out = Vec::new();
    let mut model_out = Vec::new();
    let mut solid_out = Vec::new();
    bake_particles(&particles, &mut block_out, &mut model_out, &mut solid_out);
    assert_eq!(block_out.len(), 2, "block rows route to the block list");
    assert_eq!(model_out.len(), 1, "model rows route to the model list");
    assert_eq!(solid_out.len(), 1, "solid rows route to the blended list");
    assert_eq!(
        solid_out[0].color, block_out[0].tint,
        "a solid particle's tint IS its color"
    );
}

fn mob_row(prev_x: f64, x: f64) -> MobPresentation {
    MobPresentation {
        id: 1,
        kind: petramond::mob::Mob::Owl,
        prev_pos: WorldPos::new(prev_x, 64.0, 0.0),
        pos: WorldPos::new(x, 64.0, 0.0),
        prev_yaw: 0.0,
        yaw: 0.0,
        prev_tilt: petramond_math::math::Tilt::LEVEL,
        tilt: petramond_math::math::Tilt::LEVEL,
        prev_anim_time: 0.0,
        anim_time: 1.0,
        moving: true,
        idle_anim: None,
        gait_weight: 1.0,
        gait_fades: crate::ArenaRange::default(),
        prev_head_yaw: 0.0,
        head_yaw: 0.0,
        prev_head_pitch: 0.0,
        head_pitch: 0.0,
        skylight: 0,
        blocklight: petramond_world::light::BlockLight6::DARK,
        hurt_flash: 0.0,
        dead: false,
        shorn: false,
        anims: crate::ArenaRange::default(),
        emitter_tint: [1.0; 3],
        emitter_self_lit: 0.0,
        ragdoll_pose: None,
        held: [Some(ItemType::Stone), None],
    }
}

#[test]
fn bake_mobs_interpolates_and_keeps_each_rows_arena_ranges() {
    let mut first = mob_row(0.0, 2.0);
    first.anims = crate::ArenaRange { start: 0, len: 2 };
    first.gait_fades = crate::ArenaRange { start: 1, len: 1 };
    let mut second = mob_row(4.0, 4.0);
    second.anims = crate::ArenaRange { start: 2, len: 1 };
    second.ragdoll_pose = Some(crate::ArenaRange { start: 0, len: 5 });
    second.dead = true;
    let mut out = Vec::new();
    bake_mobs(&[first, second], 0.5, &mut out);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].pos, WorldPos::new(1.0, 64.0, 0.0));
    assert_eq!(out[0].anim_time, 0.5);
    assert_eq!(out[0].anims, first.anims);
    assert_eq!(out[0].gait_fades, first.gait_fades);
    assert_eq!(out[0].ragdoll, None);
    assert_eq!(out[0].held, [Some(ItemType::Stone), None]);
    assert_eq!(out[1].anims, second.anims);
    assert_eq!(out[1].ragdoll, second.ragdoll_pose);
    assert_eq!(out[1].held, [None; 2], "a dead body holds nothing");
}

#[test]
fn a_copied_mob_arena_answers_the_gathers_ranges() {
    let mut interner = crate::AnimInterner::default();
    let oar = interner.intern("oar");
    let gather = crate::MobArena {
        gait_fades: vec![crate::GaitFade {
            clip: crate::GaitClip::Idle(2),
            phase: 0.5,
            weight: 0.25,
        }],
        anims: vec![crate::AnimLayer {
            anim: oar,
            phase: 1.5,
            weight: 1.0,
        }],
        ragdoll: vec![(glam::Vec3::X, glam::Quat::IDENTITY)],
    };
    let mut scene = crate::MobArena {
        anims: vec![
            crate::AnimLayer {
                anim: crate::AnimId(7),
                phase: 0.0,
                weight: 0.0,
            };
            4
        ],
        ..Default::default()
    };
    scene.copy_from(&gather);
    assert_eq!(
        scene, gather,
        "stale rows are gone, the gather's are in place"
    );
    let range = crate::ArenaRange { start: 0, len: 1 };
    assert_eq!(range.of(&scene.anims)[0].anim, oar);
    assert_eq!(
        interner.names().get(oar).map(|n| &**n),
        Some("oar"),
        "the id resolves through the session's table"
    );
}

#[test]
fn an_adopted_name_table_tracks_the_interner_without_copying_names() {
    let mut interner = crate::AnimInterner::default();
    let row = interner.intern("row");
    let mut held = crate::AnimNames::default();
    held.adopt(interner.names());
    let first = held.get(row).cloned().expect("adopted");
    held.adopt(interner.names());
    assert!(std::sync::Arc::ptr_eq(&first, held.get(row).unwrap()));

    let snapshot = held.clone();
    let steer = interner.intern("steer");
    assert_eq!(steer, crate::AnimId(1));
    assert_eq!(snapshot.len(), 1, "an earlier table is never mutated");
    held.adopt(interner.names());
    assert_eq!(held.len(), 2);
    assert!(
        std::sync::Arc::ptr_eq(&first, held.get(row).unwrap()),
        "growth shares the existing names"
    );
    assert_eq!(held.get(steer).map(|n| &**n), Some("steer"));
}
