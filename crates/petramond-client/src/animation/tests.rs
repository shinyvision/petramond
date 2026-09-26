//! The animation stage driven headlessly, the way the presentation gather and
//! the app drive it — no renderer, no GPU.

use glam::IVec3;
use petramond::player::AnimatorClaims;
use petramond_math::view_volume::{Frustum, ViewVolume};
use petramond_math::world_pos::WorldPos;
use petramond_world::light::BlockLight6;

use super::*;

fn body_at(pos: WorldPos) -> BodyInput {
    BodyInput {
        pos,
        emitter_tint: [1.0; 3],
        emitter_self_lit: 0.0,
        skylight: 15,
        blocklight: BlockLight6::DARK,
        state: BodyState::default(),
        bones: ArenaRange::default(),
    }
}

fn remote(key: u32, pos: WorldPos) -> RemoteBody {
    RemoteBody {
        key,
        body: body_at(pos),
        held: [HeldItemView::default(); 2],
        frames: [HeldItemFrame::default(); 2],
        animator: AnimatorRanges::default(),
    }
}

/// A view that sees everything within `reach` blocks of the origin.
fn view_within(reach: f32) -> ViewVolume {
    ViewVolume::new(
        Frustum::permissive(),
        IVec3::ZERO,
        WorldPos::ZERO,
        reach,
        f32::MAX,
    )
}

fn bone_count() -> usize {
    rigs::presented(Presenter::Body)
        .expect("the body rig ships")
        .1
        .model
        .bones()
        .len()
}

fn open_frame(animation: &mut PlayerAnimation) {
    let claims = AnimatorClaims::default();
    animation.begin_frame(
        LocalInput {
            hands: [HeldItemFrame::default(); 2],
            claims: &claims,
            events: &[],
            motion: LocalMotion::default(),
            hurt_flash: 0.0,
        },
        1.0 / 60.0,
    );
}

#[test]
fn only_bodies_the_view_sees_are_posed_each_into_its_own_range() {
    let mut animation = PlayerAnimation::default();
    open_frame(&mut animation);
    animation.frame.clear();
    animation
        .frame
        .remotes
        .push(remote(1, WorldPos::new(4.0, 0.0, 4.0)));
    animation
        .frame
        .remotes
        .push(remote(2, WorldPos::new(200.0, 0.0, 0.0)));
    animation
        .frame
        .remotes
        .push(remote(3, WorldPos::new(-3.0, 0.0, 2.0)));
    animation.pose_bodies(&view_within(16.0));

    let bodies = animation.bodies();
    assert_eq!(bodies.len(), 2, "the far body advances unposed");
    assert_eq!(bodies[0].body.pos, WorldPos::new(4.0, 0.0, 4.0));
    assert_eq!(bodies[1].body.pos, WorldPos::new(-3.0, 0.0, 2.0));
    let bones = bone_count();
    assert_eq!(animation.poses().len(), bones * 2);
    for body in bodies {
        assert_eq!(body.body.pose.of(animation.poses()).len(), bones);
    }
    assert_ne!(bodies[0].body.pose, bodies[1].body.pose);
}

#[test]
fn the_local_body_is_posed_first_and_carries_the_frames_hurt_flash() {
    let mut animation = PlayerAnimation::default();
    let claims = AnimatorClaims::default();
    animation.begin_frame(
        LocalInput {
            hands: [HeldItemFrame::default(); 2],
            claims: &claims,
            events: &[],
            motion: LocalMotion::default(),
            hurt_flash: 0.75,
        },
        1.0 / 60.0,
    );
    animation.frame.clear();
    animation
        .frame
        .remotes
        .push(remote(9, WorldPos::new(1.0, 0.0, 0.0)));
    animation.frame.local = Some(body_at(WorldPos::new(0.0, 0.0, 1.0)));
    animation.pose_bodies(&ViewVolume::unbounded());

    let bodies = animation.bodies();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0].body.pos, WorldPos::new(0.0, 0.0, 1.0));
    assert_eq!(
        bodies[0].body.hurt, 0.75,
        "the app's envelope flashes the body"
    );
    assert_eq!(bodies[1].body.hurt, 0.0);
}

#[test]
fn clearing_for_a_new_world_drops_every_posed_row() {
    let mut animation = PlayerAnimation::default();
    open_frame(&mut animation);
    animation
        .frame
        .remotes
        .push(remote(1, WorldPos::new(2.0, 0.0, 2.0)));
    animation.pose_bodies(&ViewVolume::unbounded());
    assert!(!animation.bodies().is_empty());

    animation.clear();
    assert!(animation.bodies().is_empty());
    assert!(animation.poses().is_empty());
    assert!(animation.frame.remotes.is_empty());
}
