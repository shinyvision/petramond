use crate::mob::{Mob, MobDamageFeedback, MobId, MobTagValue, SavedMob};
use crate::world::ServerWorld;
use petramond_math::world_pos::WorldPos;
use petramond_world::body::Body;
use petramond_world::chunk::SectionPos;

mod fluid;

fn id_at(mobs: &Mobs, i: usize) -> MobId {
    mobs.instances()[i].id()
}

#[test]
fn mobs_anchor_on_the_nearest_player() {
    use super::PlayerAnchor;
    let a = PlayerAnchor {
        id: crate::player::PlayerId(0),
        pos: WorldPos::new(0.0, 64.0, 0.0),
        ..Default::default()
    };
    let b = PlayerAnchor {
        id: crate::player::PlayerId(1),
        pos: WorldPos::new(10.0, 64.0, 0.0),
        ..Default::default()
    };
    let near_b = WorldPos::new(8.0, 64.0, 0.0);
    assert_eq!(super::nearest_anchor(&[a, b], near_b).id.0, 1);
    assert_eq!(
        super::nearest_anchor(&[b, a], near_b).id.0,
        1,
        "order-independent"
    );
    let near_a = WorldPos::new(1.0, 64.0, 0.0);
    assert_eq!(super::nearest_anchor(&[a, b], near_a).id.0, 0);
}

#[test]
fn a_frozen_tick_discards_its_drive_intent() {
    let world = ServerWorld::new(0, 1);
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.5, 64.0, 8.5), 0.0));
    assert!(mobs.set_mob_drive(
        id_at(&mobs, 0),
        Some([2.0, 0.0]),
        None,
        Some(1.0),
        false,
        false
    ));
    assert!(mobs.instances()[0].drive_pending());

    mobs.tick(
        0.05,
        &world,
        &[PlayerAnchor {
            pos: WorldPos::new(0.0, 64.0, 0.0),
            ..Default::default()
        }],
        true,
    );

    assert!(
        !mobs.instances()[0].drive_pending(),
        "a skipped integration cannot carry this tick's command forward"
    );
}
use super::*;

#[test]
fn take_in_section_harvests_only_that_sections_mobs() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(2.5, 64.0, 2.5), 0.5));
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(20.5, 64.0, 2.5), 1.0));

    let taken = mobs.take_in_section(SectionPos::new(0, 4, 0));
    assert_eq!(taken.len(), 1, "only the (0,4,0) owl is harvested");
    assert_eq!(taken[0].kind, Mob::Owl);
    assert_eq!(taken[0].pos, WorldPos::new(2.5, 64.0, 2.5));
    assert_eq!(taken[0].yaw, 0.5, "facing is captured");
    assert_eq!(mobs.len(), 1, "the (1,4,0) owl stays live");
}

#[test]
fn saved_by_section_groups_live_mobs_without_removing_them() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(2.5, 64.0, 2.5), 0.0));
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(5.5, 64.0, 9.5), 0.0));
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(20.5, 64.0, 2.5), 0.0));

    let map = mobs.saved_by_section();
    assert_eq!(map[&SectionPos::new(0, 4, 0)].len(), 2);
    assert_eq!(map[&SectionPos::new(1, 4, 0)].len(), 1);
    assert_eq!(mobs.len(), 3, "the flush clones; the mobs stay live");
}

#[test]
fn restore_respawns_saved_mobs_with_their_pose() {
    let mut mobs = Mobs::new(0);
    mobs.restore([
        SavedMob {
            kind: Mob::Owl,
            pos: WorldPos::new(8.5, 70.0, 8.5),
            yaw: 1.25,
            tags: Default::default(),
            container: Default::default(),
        },
        SavedMob {
            kind: Mob::Sheep,
            pos: WorldPos::new(9.5, 70.0, 8.5),
            yaw: -0.5,
            tags: std::collections::BTreeMap::from([(
                crate::mob::tags::SHEAR_REGROW.to_owned(),
                MobTagValue::Int(500),
            )]),
            container: Default::default(),
        },
    ]);
    assert_eq!(mobs.len(), 2);
    let poses: Vec<(WorldPos, f32)> = mobs.instances().iter().map(|m| (m.pos, m.yaw)).collect();
    assert!(
        poses.contains(&(WorldPos::new(8.5, 70.0, 8.5), 1.25)),
        "first mob restored in place"
    );
    assert!(
        poses.contains(&(WorldPos::new(9.5, 70.0, 8.5), -0.5)),
        "second mob restored in place"
    );
    let shorn: Vec<bool> = mobs.instances().iter().map(Instance::is_shorn).collect();
    assert!(
        shorn.contains(&true) && shorn.contains(&false),
        "a saved regrow counter carries over on restore: {shorn:?}"
    );
}

#[test]
fn mob_tags_survive_section_unload_and_reload() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(2.5, 64.0, 2.5), 0.5));
    assert!(mobs.set_mob_tag(
        id_at(&mobs, 0),
        "zombies:anger".into(),
        MobTagValue::Int(31)
    ));

    let taken = mobs.take_in_section(SectionPos::new(0, 4, 0));
    assert_eq!(taken.len(), 1);
    assert_eq!(
        taken[0].tags.get("zombies:anger"),
        Some(&MobTagValue::Int(31))
    );
    assert_eq!(mobs.len(), 0, "harvested out of the live set");

    mobs.restore(taken);
    assert_eq!(
        mobs.mob_tag(id_at(&mobs, 0), "zombies:anger"),
        Some(&MobTagValue::Int(31)),
        "the tag is back on the restored mob"
    );
    assert!(mobs.remove_mob_tag(id_at(&mobs, 0), "zombies:anger"));
    assert!(!mobs.remove_mob_tag(id_at(&mobs, 0), "zombies:anger"));
    assert!(!mobs.set_mob_tag(999, "zombies:anger".into(), MobTagValue::Int(1)));
}

#[test]
fn a_wounded_mob_saves_and_restores_wounded() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Sheep, WorldPos::new(2.5, 64.0, 2.5), 0.0));
    let spawn_health = crate::mob::def(Mob::Sheep).spawn_health();
    assert_eq!(mobs.instances()[0].health(), spawn_health);
    let drop = mobs.damage_mob(
        id_at(&mobs, 0),
        spawn_health - 1.0,
        None,
        true,
        None,
        &crate::mob::MobDamageFeedback::default(),
    );
    assert!(drop.is_none(), "the hit is not lethal");
    assert_eq!(mobs.instances()[0].health(), 1.0);

    let taken = mobs.take_in_section(SectionPos::new(0, 4, 0));
    assert_eq!(taken.len(), 1);
    mobs.restore(taken);
    assert_eq!(
        mobs.instances()[0].health(),
        1.0,
        "the health tag rides the save record"
    );
}

#[test]
fn shearing_a_sheep_yields_wool_once_until_the_coat_regrows() {
    let world = ServerWorld::new(0, 1);
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Sheep, WorldPos::new(8.5, 64.0, 8.5), 0.0));
    let spec = crate::mob::def(Mob::Sheep)
        .shear
        .expect("sheep are shearable");

    let drop = mobs
        .shear_mob(id_at(&mobs, 0))
        .expect("a coated sheep shears");
    assert_eq!(drop.item, spec.drop);
    assert!(
        (spec.min..=spec.max).contains(&drop.count),
        "count rolled inside the spec range: {}",
        drop.count
    );
    assert!(mobs.instances()[0].is_shorn());
    assert!(
        mobs.shear_mob(id_at(&mobs, 0)).is_none(),
        "no double-shear while shorn"
    );

    let mut ticks: u32 = 0;
    while mobs.instances()[0].is_shorn() {
        mobs.tick(
            0.05,
            &world,
            &[crate::mob::PlayerAnchor {
                pos: far(),
                ..Default::default()
            }],
            false,
        );
        ticks += 1;
        assert!(
            ticks <= spec.regrow_max,
            "the coat must be back within the max regrow duration"
        );
    }
    assert!(
        ticks >= spec.regrow_min,
        "the coat can't regrow before the min duration: {ticks}"
    );
    assert!(
        mobs.shear_mob(id_at(&mobs, 0)).is_some(),
        "a regrown sheep can be shorn again"
    );
}

#[test]
fn a_species_without_a_shear_spec_cannot_be_shorn() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.5, 64.0, 8.5), 0.0));
    assert!(mobs.shear_mob(id_at(&mobs, 0)).is_none());
    assert!(!mobs.instances()[0].is_shorn());
}

#[test]
fn a_corpse_cannot_be_shorn() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Sheep, WorldPos::new(8.5, 64.0, 8.5), 0.0));
    assert!(mobs
        .damage_mob(
            id_at(&mobs, 0),
            100.0,
            Some(WorldPos::new(5.0, 64.0, 8.5)),
            true,
            None,
            &MobDamageFeedback::default()
        )
        .is_some());
    assert!(
        mobs.shear_mob(id_at(&mobs, 0)).is_none(),
        "a ragdolling corpse keeps its coat"
    );
}

fn horizontal_gap(mobs: &Mobs) -> f32 {
    let p = mobs.instances();
    let (a, b) = (p[0].pos, p[1].pos);
    (((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()) as f32
}

fn far() -> WorldPos {
    WorldPos::new(1000.0, 64.0, 1000.0)
}

#[test]
fn overlapping_mobs_drift_apart_smoothly() {
    let world = ServerWorld::new(0, 1);
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.0, 64.0, 8.0), 0.0));
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.05, 64.0, 8.0), 0.0));
    let reach = 2.0 * crate::mob::def(Mob::Owl).size.half_width;

    let gap0 = horizontal_gap(&mobs);
    let mut gap = gap0;
    let mut last_step = f32::INFINITY;
    for _ in 0..40 {
        mobs.tick(
            0.05,
            &world,
            &[crate::mob::PlayerAnchor {
                pos: far(),
                ..Default::default()
            }],
            false,
        );
        let next = horizontal_gap(&mobs);
        assert!(
            next >= gap - 1e-4,
            "the gap never shrinks (no snap-back): {gap} -> {next}"
        );
        last_step = next - gap;
        gap = next;
    }
    assert!(
        gap > gap0 + 0.2,
        "the overlapping owls clearly separated: {gap0} -> {gap}"
    );
    assert!(
        gap > 0.9 * reach,
        "they ended up cleanly apart: gap {gap}, reach {reach}"
    );
    assert!(
        gap < 1.3 * reach,
        "they settled at contact, not flung apart: gap {gap}, reach {reach}"
    );
    assert!(
        last_step < 0.005,
        "the push eases off as they part: final tick step {last_step}"
    );
}

#[test]
fn the_push_pass_records_touch_contacts_both_ways() {
    let world = ServerWorld::new(0, 1);
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.0, 64.0, 8.0), 0.0));
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.1, 64.0, 8.0), 0.0));
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(20.0, 64.0, 8.0), 0.0));
    let ids: Vec<u64> = mobs.instances().iter().map(Instance::id).collect();

    let player = crate::mob::PlayerAnchor {
        id: crate::player::PlayerId(3),
        pos: WorldPos::new(8.0, 64.9, 8.1),
        body: Some(Body::new(WorldPos::new(8.0, 64.0, 8.1), 0.3, 1.8)),
        sneaking: true,
        ..Default::default()
    };
    mobs.tick(0.05, &world, &[player], false);

    let contacts: Vec<&[crate::mob::EntityRef]> =
        mobs.instances().iter().map(Instance::contacts).collect();
    assert!(
        contacts[0].contains(&crate::mob::EntityRef::Mob(ids[1]))
            && contacts[1].contains(&crate::mob::EntityRef::Mob(ids[0])),
        "overlapping mobs record each other: {contacts:?}"
    );
    assert!(
        contacts[0].contains(&crate::mob::EntityRef::Player(crate::player::PlayerId(3))),
        "the touching (sneaking) player is felt: {contacts:?}"
    );
    assert!(
        contacts[2].is_empty(),
        "a distant mob touches nothing: {contacts:?}"
    );
}

#[test]
fn a_mob_overlapping_the_player_pushes_it_away() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.2, 64.0, 8.0), 0.0));
    let player_body = Body::new(WorldPos::new(8.0, 64.0, 8.0), 0.3, 1.8);
    let push = mobs.push_on_player(player_body);
    assert!(
        push.x < 0.0,
        "the player is pushed -X, away from the owl: {push:?}"
    );
    assert_eq!(push.y, 0.0, "the push is horizontal");
}

#[test]
fn a_distant_mob_does_not_push_the_player() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.0, 64.0, 8.0), 0.0));
    let player_body = Body::new(far(), 0.3, 1.8);
    assert_eq!(
        mobs.push_on_player(player_body),
        Vec3::ZERO,
        "an out-of-reach mob imparts no push"
    );
}

#[test]
fn a_bodiless_player_does_not_shove_mobs() {
    let world = ServerWorld::new(0, 1);
    let mut mobs = Mobs::new(0);
    let spot = WorldPos::new(8.0, 64.0, 8.0);
    assert!(mobs.spawn(Mob::Owl, spot, 0.0));
    let before = mobs.instances()[0].pos;
    mobs.tick(
        0.05,
        &world,
        &[crate::mob::PlayerAnchor {
            pos: spot,
            ..Default::default()
        }],
        false,
    );
    let after = mobs.instances()[0].pos;
    assert_eq!(
        (before.x, before.z),
        (after.x, after.z),
        "a player with no body doesn't shove the mob sideways"
    );
}

#[test]
fn a_harvested_corpse_is_dropped_not_saved() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(2.5, 64.0, 2.5), 0.0));
    assert!(mobs
        .damage_mob(
            id_at(&mobs, 0),
            100.0,
            Some(WorldPos::new(5.0, 64.0, 2.5)),
            true,
            None,
            &MobDamageFeedback::default()
        )
        .is_some());
    let taken = mobs.take_in_section(SectionPos::new(0, 4, 0));
    assert!(taken.is_empty(), "a corpse is not persisted");
    assert_eq!(mobs.len(), 0, "but it is removed from the live set");
}

#[test]
fn placement_is_blocked_only_where_a_solid_block_clips_a_live_mob() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.5, 64.0, 8.5), 0.0));
    let here = IVec3::new(8, 64, 8);
    let away = IVec3::new(20, 64, 8);

    assert!(
        mobs.any_overlapping_placement(here, Block::Dirt),
        "a solid block in the owl's cell is blocked"
    );
    assert!(
        !mobs.any_overlapping_placement(away, Block::Dirt),
        "a cell away from the owl is clear"
    );
    assert!(
        !mobs.any_overlapping_placement(here, Block::Torch),
        "a no-collision block is always placeable"
    );

    assert!(mobs
        .damage_mob(
            id_at(&mobs, 0),
            100.0,
            Some(WorldPos::new(9.0, 64.0, 8.5)),
            true,
            None,
            &MobDamageFeedback::default()
        )
        .is_some());
    assert!(
        !mobs.any_overlapping_placement(here, Block::Dirt),
        "a corpse doesn't block placement"
    );
}

#[test]
fn the_tag_cap_refuses_new_keys_but_never_replacements() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, WorldPos::new(8.5, 64.0, 8.5), 0.0));
    let spawn_tags = mobs.instances()[0].tags().len();
    assert!(spawn_tags >= 1, "spawn tags include petramond:health");
    for i in 0..crate::mob::MAX_MOB_TAGS - spawn_tags {
        assert!(
            mobs.set_mob_tag(
                id_at(&mobs, 0),
                format!("farm:k{i}"),
                MobTagValue::Int(i as i64)
            ),
            "key {i} fits under the cap"
        );
    }
    assert!(
        !mobs.set_mob_tag(
            id_at(&mobs, 0),
            "farm:one_too_many".into(),
            MobTagValue::Int(0)
        ),
        "a NEW key past the cap is refused"
    );
    assert!(
        mobs.set_mob_tag(id_at(&mobs, 0), "farm:k0".into(), MobTagValue::Int(-1)),
        "replacing an existing key is always allowed"
    );
    assert_eq!(
        mobs.mob_tag(id_at(&mobs, 0), "farm:k0"),
        Some(&MobTagValue::Int(-1))
    );
    assert!(mobs.remove_mob_tag(id_at(&mobs, 0), "farm:k1"));
    assert!(
        mobs.set_mob_tag(
            id_at(&mobs, 0),
            "farm:back_under".into(),
            MobTagValue::Int(0)
        ),
        "a deletion frees a slot again"
    );
}

#[test]
fn with_tag_filters_by_presence_and_value_and_skips_the_dead() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Sheep, WorldPos::new(8.5, 64.0, 8.5), 0.0));
    assert!(mobs.spawn(Mob::Sheep, WorldPos::new(9.5, 64.0, 8.5), 0.0));
    assert!(mobs.spawn(Mob::Sheep, WorldPos::new(10.5, 64.0, 8.5), 0.0));
    let (a, b) = (id_at(&mobs, 0), id_at(&mobs, 1));
    assert!(mobs.set_mob_tag(a, "farm:quality".into(), MobTagValue::Int(1)));
    assert!(mobs.set_mob_tag(b, "farm:quality".into(), MobTagValue::Int(2)));
    let carriers = |mobs: &Mobs, want: Option<&MobTagValue>, key: &str| -> Vec<MobId> {
        mobs.with_tag(key, want).map(|(_, m)| m.id()).collect()
    };

    assert_eq!(
        carriers(&mobs, None, "farm:quality"),
        vec![a, b],
        "presence matches every carrier"
    );
    assert_eq!(
        carriers(&mobs, Some(&MobTagValue::Int(2)), "farm:quality"),
        vec![b],
        "a value filter matches only equal values"
    );
    assert_eq!(
        carriers(&mobs, Some(&MobTagValue::Int(1)), "farm:quality"),
        vec![a],
        "and the other stored value"
    );
    assert_eq!(
        carriers(&mobs, None, "farm:missing"),
        Vec::<MobId>::new(),
        "an uncarried key matches nothing"
    );
    assert!(
        mobs.with_tag("farm:quality", None)
            .all(|(position, m)| mobs.position_of(m.id()) == Some(position)),
        "each carrier comes with its position in the listing"
    );

    assert!(mobs
        .damage_mob(
            a,
            100.0,
            Some(WorldPos::new(5.0, 64.0, 8.5)),
            true,
            None,
            &MobDamageFeedback::default()
        )
        .is_some());
    assert_eq!(
        carriers(&mobs, None, "farm:quality"),
        vec![b],
        "a ragdolling corpse is gone to the query, exactly like MobsInRadius"
    );
}

#[test]
fn a_penned_mob_becomes_confined_and_a_broken_fence_frees_it_within_ticks() {
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    let mut world = ServerWorld::new(0, 1);
    for cx in 0..3 {
        for cz in 0..3 {
            let mut chunk = Chunk::new(cx, cz);
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    chunk.set_block(x, 63, z, Block::Grass);
                }
            }
            world.insert_chunk_for_test(ChunkPos::new(cx, cz), chunk);
        }
    }
    for i in 21..=27 {
        for (x, z) in [(21, i), (27, i), (i, 21), (i, 27)] {
            assert!(world.set_block_world(x, 64, z, Block::OakFence));
        }
    }
    let id = world
        .spawn_mob(Mob::Sheep, WorldPos::new(24.5, 64.0, 24.5), 0.0)
        .expect("spawned");
    let anchors = [PlayerAnchor {
        pos: WorldPos::new(24.5, 64.0, 30.5),
        ..Default::default()
    }];
    let confined = |world: &ServerWorld| world.mobs().get(id).expect("alive").is_confined();

    for _ in 0..=super::super::confined::CHECK_INTERVAL as usize {
        world.tick_mobs(0.05, &anchors);
    }
    assert!(confined(&world), "an enclosed fence pen must read confined");

    assert!(world.set_block_world(27, 64, 24, Block::Air));
    world.tick_mobs(0.05, &anchors);
    world.tick_mobs(0.05, &anchors);
    assert!(!confined(&world), "a gap in the fence frees the pen-mate");
}

#[test]
fn the_push_broadphase_keeps_every_genuinely_overlapping_pair() {
    use crate::mob::{def, Mob};

    let kinds: Vec<Mob> = crate::mob::defs().iter().map(|d| d.mob).collect();
    let mut rng = 0x1234_5678_9abc_def0u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f32 / (1u64 << 53) as f32
    };
    let bodies: Vec<Option<super::push::PushBody>> = (0..160)
        .map(|i| {
            let kind = kinds[i % kinds.len()];
            let size = def(kind).size;
            let spread = if i % 3 == 0 { 3.0 } else { 40.0 };
            let pos = WorldPos::new(
                f64::from((next() - 0.5) * spread),
                f64::from((next() - 0.5) * 4.0),
                f64::from((next() - 0.5) * spread),
            );
            Some(super::push::push_body_for_test(
                pos,
                next() * std::f32::consts::TAU,
                size,
            ))
        })
        .collect();
    let order: Vec<usize> = (0..bodies.len()).collect();
    let (mut sweep, mut pairs) = (Vec::new(), Vec::new());
    super::push::overlap_pairs(&bodies, &order, &mut sweep, &mut pairs);

    assert!(pairs.windows(2).all(|w| w[0] < w[1]), "sorted and deduped");
    let mut kept = 0usize;
    for ra in 0..order.len() {
        for rb in ra + 1..order.len() {
            let a = bodies[order[ra]].unwrap();
            let b = bodies[order[rb]].unwrap();
            if super::body_separation(a.pos(), a.yaw(), a.size(), b.pos(), b.yaw(), b.size())
                .is_some()
            {
                kept += 1;
                assert!(
                    pairs.binary_search(&(ra as u32, rb as u32)).is_ok(),
                    "broadphase dropped overlapping pair {ra}/{rb}"
                );
            }
        }
    }
    assert!(kept > 20, "fixture must actually produce overlaps: {kept}");
    assert!(
        pairs.len() < order.len() * (order.len() - 1) / 4,
        "broadphase should reject most pairs, kept {}",
        pairs.len()
    );
}

#[test]
fn handles_stay_stable_across_every_live_set_mutation() {
    let assert_consistent = |mobs: &Mobs| {
        for (i, m) in mobs.instances().iter().enumerate() {
            assert_eq!(mobs.slot(m.id()), Some(i), "mob {} at {i}", m.id());
            assert_eq!(mobs.get(m.id()).map(|g| g.id()), Some(m.id()));
        }
    };
    let mut mobs = Mobs::new(0);
    let mut spawned = Vec::new();
    for x in 0..6 {
        let pos = WorldPos::new(f64::from(x) * 20.0 + 8.0, 64.0, 8.0);
        let light = petramond_world::light::BlockLight6::DARK;
        spawned.push((mobs.spawn_lit(Mob::Owl, pos, 0.0, 63, light).unwrap(), pos));
    }
    assert_consistent(&mobs);
    for &(id, pos) in &spawned {
        assert!(mobs.set_mob_tag(id, "test:x".into(), MobTagValue::Float(pos.x)));
    }
    let names_itself =
        |mobs: &Mobs, id: MobId, x: f64| mobs.mob_tag(id, "test:x") == Some(&MobTagValue::Float(x));

    let removed = spawned[1].0;
    assert!(mobs.remove(removed));
    assert!(!mobs.remove(removed), "a second removal finds nothing");
    assert!(!mobs.contains(removed));
    assert!(mobs.get(removed).is_none());
    assert!(!mobs.set_mob_tag(removed, "test:y".into(), MobTagValue::Int(0)));
    assert_consistent(&mobs);
    for &(id, pos) in spawned.iter().filter(|(id, _)| *id != removed) {
        assert!(
            names_itself(&mobs, id, pos.x),
            "handle {id} still names its mob"
        );
    }

    let harvested = mobs.take_in_section(SectionPos::new(0, 4, 0));
    assert_eq!(
        harvested.len(),
        1,
        "only the first owl sits in section (0, 4, 0)"
    );
    assert!(!mobs.contains(spawned[0].0));
    assert_consistent(&mobs);

    let victim = spawned[2].0;
    let lethal = MobDamageFeedback {
        components: vec![crate::mob::MobDamageFeedbackComponent::DecreaseHealth],
    };
    assert!(mobs
        .damage_mob(victim, 1000.0, None, true, None, &lethal)
        .is_some());
    assert!(mobs.contains(victim), "a corpse stays until the cull");
    assert!(mobs.live(victim).is_none(), "but it is not live");
    let world = ServerWorld::new(0, 1);
    let anchor = PlayerAnchor {
        pos: WorldPos::new(8.0, 64.0, 8.0),
        ..Default::default()
    };
    mobs.tick(0.05, &world, &[anchor], false);
    assert!(!mobs.contains(victim), "the cull drops the handle");
    assert_eq!(mobs.len(), 3);
    assert_consistent(&mobs);
    for &(id, pos) in &spawned[3..] {
        assert!(
            names_itself(&mobs, id, pos.x),
            "survivor {id} keeps its handle"
        );
    }
}
