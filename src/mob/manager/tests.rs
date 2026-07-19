use crate::body::Body;
use crate::chunk::SectionPos;
use crate::mob::{Mob, MobDamageFeedback, SavedMob};
use crate::world::World;

#[test]
fn mobs_anchor_on_the_nearest_player() {
    use super::PlayerAnchor;
    let a = PlayerAnchor {
        id: crate::server::player::PlayerId(0),
        pos: Vec3::new(0.0, 64.0, 0.0),
        ..Default::default()
    };
    let b = PlayerAnchor {
        id: crate::server::player::PlayerId(1),
        pos: Vec3::new(10.0, 64.0, 0.0),
        ..Default::default()
    };
    let near_b = Vec3::new(8.0, 64.0, 0.0);
    assert_eq!(super::nearest_anchor(&[a, b], near_b).id.0, 1);
    assert_eq!(
        super::nearest_anchor(&[b, a], near_b).id.0,
        1,
        "order-independent"
    );
    let near_a = Vec3::new(1.0, 64.0, 0.0);
    assert_eq!(super::nearest_anchor(&[a, b], near_a).id.0, 0);
}

#[test]
fn a_frozen_tick_discards_its_drive_intent() {
    let world = World::new(0, 1);
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, Vec3::new(8.5, 64.0, 8.5), 0.0));
    assert!(mobs.set_mob_drive(0, 2.0, 0.0, Some(1.0)));
    assert!(mobs.instances()[0].drive_pending());

    mobs.tick(
        0.05,
        &world,
        &[PlayerAnchor {
            pos: Vec3::new(0.0, 64.0, 0.0),
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
    // y=64 → cy 4. x 2.5 → cx 0; x 20.5 → cx 1.
    assert!(mobs.spawn(Mob::Owl, Vec3::new(2.5, 64.0, 2.5), 0.5)); // section (0,4,0)
    assert!(mobs.spawn(Mob::Owl, Vec3::new(20.5, 64.0, 2.5), 1.0)); // section (1,4,0)

    let taken = mobs.take_in_section(SectionPos::new(0, 4, 0));
    assert_eq!(taken.len(), 1, "only the (0,4,0) owl is harvested");
    assert_eq!(taken[0].kind, Mob::Owl);
    assert_eq!(taken[0].pos, Vec3::new(2.5, 64.0, 2.5));
    assert_eq!(taken[0].yaw, 0.5, "facing is captured");
    assert_eq!(mobs.len(), 1, "the (1,4,0) owl stays live");
}

#[test]
fn saved_by_section_groups_live_mobs_without_removing_them() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, Vec3::new(2.5, 64.0, 2.5), 0.0)); // (0,4,0)
    assert!(mobs.spawn(Mob::Owl, Vec3::new(5.5, 64.0, 9.5), 0.0)); // (0,4,0)
    assert!(mobs.spawn(Mob::Owl, Vec3::new(20.5, 64.0, 2.5), 0.0)); // (1,4,0)

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
            pos: Vec3::new(8.5, 70.0, 8.5),
            yaw: 1.25,
            shear_regrow: 0,
            tags: Default::default(),
            kv: Default::default(),
        },
        SavedMob {
            kind: Mob::Sheep,
            pos: Vec3::new(9.5, 70.0, 8.5),
            yaw: -0.5,
            shear_regrow: 500,
            tags: Default::default(),
            kv: Default::default(),
        },
    ]);
    assert_eq!(mobs.len(), 2);
    let poses: Vec<(Vec3, f32)> = mobs.instances().iter().map(|m| (m.pos, m.yaw)).collect();
    assert!(
        poses.contains(&(Vec3::new(8.5, 70.0, 8.5), 1.25)),
        "first mob restored in place"
    );
    assert!(
        poses.contains(&(Vec3::new(9.5, 70.0, 8.5), -0.5)),
        "second mob restored in place"
    );
    let shorn: Vec<bool> = mobs.instances().iter().map(Instance::is_shorn).collect();
    assert!(
        shorn.contains(&true) && shorn.contains(&false),
        "a saved regrow counter carries over on restore: {shorn:?}"
    );
}

#[test]
fn mob_mod_kv_survives_section_unload_and_reload() {
    // The unload → save-record → reload cycle at the manager level: a mod
    // KV entry set on a live mob rides its SavedMob projection and is back
    // on the restored instance (the on-disk byte layer is covered by
    // `save::mobs`).
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, Vec3::new(2.5, 64.0, 2.5), 0.5));
    assert!(mobs.mod_kv_set(0, "zombies:anger".into(), vec![3, 1]));
    assert_eq!(mobs.mod_kv_get(0, "zombies:anger"), Some(&[3u8, 1][..]));

    let taken = mobs.take_in_section(SectionPos::new(0, 4, 0));
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].kv.get("zombies:anger"), Some(&vec![3, 1]));
    assert_eq!(mobs.len(), 0, "harvested out of the live set");

    mobs.restore(taken);
    assert_eq!(
        mobs.mod_kv_get(0, "zombies:anger"),
        Some(&[3u8, 1][..]),
        "the KV is back on the restored mob"
    );
    // Removal reports presence honestly; out-of-range indices are inert.
    assert!(mobs.mod_kv_remove(0, "zombies:anger"));
    assert!(!mobs.mod_kv_remove(0, "zombies:anger"));
    assert!(!mobs.mod_kv_set(9, "zombies:anger".into(), vec![1]));
}

#[test]
fn shearing_a_sheep_yields_wool_once_until_the_coat_regrows() {
    let world = World::new(0, 1);
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Sheep, Vec3::new(8.5, 64.0, 8.5), 0.0));
    let spec = crate::mob::def(Mob::Sheep)
        .shear
        .expect("sheep are shearable");

    let drop = mobs.shear_mob(0).expect("a coated sheep shears");
    assert_eq!(drop.item, spec.drop);
    assert!(
        (spec.min..=spec.max).contains(&drop.count),
        "count rolled inside the spec range: {}",
        drop.count
    );
    assert!(mobs.instances()[0].is_shorn());
    assert!(mobs.shear_mob(0).is_none(), "no double-shear while shorn");

    // The coat regrows on the tick, within the spec's rolled range.
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
        mobs.shear_mob(0).is_some(),
        "a regrown sheep can be shorn again"
    );
}

#[test]
fn a_species_without_a_shear_spec_cannot_be_shorn() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, Vec3::new(8.5, 64.0, 8.5), 0.0));
    assert!(mobs.shear_mob(0).is_none());
    assert!(!mobs.instances()[0].is_shorn());
}

#[test]
fn a_corpse_cannot_be_shorn() {
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Sheep, Vec3::new(8.5, 64.0, 8.5), 0.0));
    assert!(mobs
        .damage_mob(
            0,
            100.0,
            Some(Vec3::new(5.0, 64.0, 8.5)),
            true,
            None,
            &MobDamageFeedback::default()
        )
        .is_some());
    assert!(
        mobs.shear_mob(0).is_none(),
        "a ragdolling corpse keeps its coat"
    );
}

/// The horizontal distance between the first two live mobs.
fn horizontal_gap(mobs: &Mobs) -> f32 {
    let p = mobs.instances();
    let (a, b) = (p[0].pos, p[1].pos);
    ((a.x - b.x).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

/// A point far from the origin — used as a parked player anchor / body so a tick
/// exercises only mob↔mob pushing.
fn far() -> Vec3 {
    Vec3::new(1000.0, 64.0, 1000.0)
}

#[test]
fn overlapping_mobs_drift_apart_smoothly() {
    // Two owls spawned almost on top of each other must ease apart *gradually* and
    // monotonically — never snapping back (the jitter we're avoiding) — and settle
    // just clear of each other (≈ their combined half-widths), not blow past. The
    // empty world has no floor, so they also fall; the gap checked is horizontal. No
    // player body this tick.
    let world = World::new(0, 1);
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
    assert!(mobs.spawn(Mob::Owl, Vec3::new(8.05, 64.0, 8.0), 0.0));
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
        // No snap-back: the gap only ever grows — the jitter we were getting was the
        // gap oscillating as positions were snapped each tick.
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
    // Eased to rest: the push fades out as they separate (proportional to the
    // shrinking overlap), so by the end they've coasted to a stop — a gradual drift
    // that converges, not a constant ram.
    assert!(
        last_step < 0.005,
        "the push eases off as they part: final tick step {last_step}"
    );
}

#[test]
fn the_push_pass_records_touch_contacts_both_ways() {
    // The touch perception channel: overlapping bodies land in each
    // other's contact lists (and the player in the mob's), while a
    // distant mob records nothing.
    let world = World::new(0, 1);
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
    assert!(mobs.spawn(Mob::Owl, Vec3::new(8.1, 64.0, 8.0), 0.0)); // overlapping
    assert!(mobs.spawn(Mob::Owl, Vec3::new(20.0, 64.0, 8.0), 0.0)); // far away
    let ids: Vec<u64> = mobs.instances().iter().map(Instance::id).collect();

    let player = crate::mob::PlayerAnchor {
        id: crate::server::player::PlayerId(3),
        pos: Vec3::new(8.0, 64.9, 8.1),
        body: Some(Body::new(Vec3::new(8.0, 64.0, 8.1), 0.3, 1.8)),
        sneaking: true, // touch is felt, not heard — sneak is irrelevant
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
        contacts[0].contains(&crate::mob::EntityRef::Player(
            crate::server::player::PlayerId(3)
        )),
        "the touching (sneaking) player is felt: {contacts:?}"
    );
    assert!(
        contacts[2].is_empty(),
        "a distant mob touches nothing: {contacts:?}"
    );
}

#[test]
fn a_mob_overlapping_the_player_pushes_it_away() {
    // The mobs push the player too — but that's a per-frame query (`push_on_player`),
    // not the tick, so the player drifts out smoothly. It points away from the owl.
    let mut mobs = Mobs::new(0);
    // Owl just east (+X) of the player's column, footprints overlapping.
    assert!(mobs.spawn(Mob::Owl, Vec3::new(8.2, 64.0, 8.0), 0.0));
    let player_body = Body::new(Vec3::new(8.0, 64.0, 8.0), 0.3, 1.8);
    let push = mobs.push_on_player(player_body);
    assert!(
        push.x < 0.0,
        "the player is pushed -X, away from the owl: {push:?}"
    );
    assert_eq!(push.y, 0.0, "the push is horizontal");
}

#[test]
fn a_distant_mob_does_not_push_the_player() {
    // No overlap, no push — a mob across the world leaves the player be.
    let mut mobs = Mobs::new(0);
    assert!(mobs.spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
    let player_body = Body::new(far(), 0.3, 1.8);
    assert_eq!(
        mobs.push_on_player(player_body),
        Vec3::ZERO,
        "an out-of-reach mob imparts no push"
    );
}

#[test]
fn a_bodiless_player_does_not_shove_mobs() {
    // A noclip spectator (no push body) overlapping a mob leaves it be — the tick's
    // player→mob shove is skipped when there's no body (the caller likewise skips the
    // per-frame mob→player push for a spectator).
    let world = World::new(0, 1);
    let mut mobs = Mobs::new(0);
    let spot = Vec3::new(8.0, 64.0, 8.0);
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
    assert!(mobs.spawn(Mob::Owl, Vec3::new(2.5, 64.0, 2.5), 0.0));
    // Kill it: now a ragdolling corpse. Harvesting its section removes it but does not
    // persist it (its loot already fell when it died).
    assert!(mobs
        .damage_mob(
            0,
            100.0,
            Some(Vec3::new(5.0, 64.0, 2.5)),
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
    assert!(mobs.spawn(Mob::Owl, Vec3::new(8.5, 64.0, 8.5), 0.0)); // body in cell (8,64,8)
    let here = IVec3::new(8, 64, 8);
    let away = IVec3::new(20, 64, 8);

    // A solid full cube dropped into the owl's cell clips its body.
    assert!(
        mobs.any_overlapping_placement(here, Block::Dirt),
        "a solid block in the owl's cell is blocked"
    );
    // The same cube well clear of the owl is fine.
    assert!(
        !mobs.any_overlapping_placement(away, Block::Dirt),
        "a cell away from the owl is clear"
    );
    // A no-collision block (a torch) never clips anything, even right on the owl.
    assert!(
        !mobs.any_overlapping_placement(here, Block::Torch),
        "a no-collision block is always placeable"
    );

    // A ragdolling corpse doesn't block placement (it's about to vanish).
    assert!(mobs
        .damage_mob(
            0,
            100.0,
            Some(Vec3::new(9.0, 64.0, 8.5)),
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
