//! The first-person viewmodel: the rigs catalog's viewmodel rig, posed by its
//! animator and baked in VIEW space for the hand pass — the arms in the
//! player's skin, and each hand's held item CARRIED by its fist from the
//! item's first-person rest seat (`hand::rest_seat`): the fist rests in the
//! rig row's hold clip for the item's render kind, where the item sits
//! exactly on its authored seat, and wherever the fist goes the item goes
//! with it. View space is the rig's own: camera at the origin looking down
//! −Z, one rig pixel a sixteenth of a block.
//!
//! The row's camera bone IS the view. The hand pass draws through its
//! inverse and the renderer applies the same inverse to the world camera, so
//! a clip that kicks the camera moves the world and the arms as one.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use petramond::player::rigs::{self, Presenter, Rig};
use petramond_world::animation::{ClipId, LocalPose, MirrorMap};
use petramond_world::item::ItemType;

use super::item_model::ItemVertex;
use super::mob_model::bake_model_cubes;
use crate::views::LocalMotion;
use crate::{AnimatorInputs, HeldItemFrame};

mod driver;

/// One rig pixel in view-space blocks.
pub(crate) const RIG_PX: f32 = 1.0 / 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hand {
    /// The row's main grip.
    Main,
    /// The row's off grip.
    Off,
}

pub(crate) struct FirstPersonRig {
    pub row: &'static Rig,
    /// Per hand: the inverse of the grip frame at the rig's rest.
    rest: [Mat4; 2],
    /// Per hand: the inverse of the grip frame in each hold clip the row
    /// names — the off hand's the main hold mirrored, as the animator plays it.
    holds: [Vec<(ClipId, Mat4)>; 2],
    /// Per hand: where the fist resting in the sprite hold grips a sprite, in
    /// view-space blocks.
    sprite_grips: [Vec3; 2],
}

impl FirstPersonRig {
    pub fn new(row: &'static Rig) -> Self {
        let model = &row.model;
        let map = MirrorMap::for_model(model);
        let bones = model.bones().len();
        let inverse_grip = |pose: &LocalPose, hand: Hand| {
            (to_view() * pose.resolve(model)[row.grips[hand as usize]]).inverse()
        };
        let rest_pose = LocalPose::rest(bones);
        let rest = [Hand::Main, Hand::Off].map(|hand| inverse_grip(&rest_pose, hand));
        let mut holds: [Vec<(ClipId, Mat4)>; 2] = Default::default();
        if let Some(graph) = &row.graph {
            for &(_, clip) in &row.holds {
                let mut main = LocalPose::rest(bones);
                main.set_clip_at(graph.clips().get(clip), 0.0);
                let mut off = LocalPose::rest(bones);
                main.mirror_into(&map, &mut off);
                holds[0].push((clip, inverse_grip(&main, Hand::Main)));
                holds[1].push((clip, inverse_grip(&off, Hand::Off)));
            }
        }
        let mut rig = Self {
            row,
            rest,
            holds,
            sprite_grips: [Vec3::ZERO; 2],
        };
        rig.sprite_grips = [Hand::Main, Hand::Off].map(|hand| {
            let pivot = model.bones()[row.grips[hand as usize]].pivot;
            let sprite = row.holds.iter().find(|(kind, _)| *kind == "sprite").map(|(_, clip)| *clip);
            rig.from_hold(hand, sprite).inverse().transform_point3(pivot)
        });
        rig
    }

    /// The inverse grip frame `hand` rests in holding what `clip` poses (the
    /// rig's rest for `None`).
    fn from_hold(&self, hand: Hand, clip: Option<ClipId>) -> Mat4 {
        clip.and_then(|clip| self.holds[hand as usize].iter().find(|(c, _)| *c == clip))
            .map_or(self.rest[hand as usize], |(_, m)| *m)
    }

    /// Where the rendered view sits in view space: identity at rest.
    pub fn camera(&self, bones: &[Mat4]) -> Mat4 {
        match self.row.camera.and_then(|b| bones.get(b)) {
            Some(bone) => to_view() * *bone * to_view().inverse(),
            None => Mat4::IDENTITY,
        }
    }

    /// A sprite's rest `seat` moved so its `grip` point (unit model space)
    /// sits in `hand`'s resting fist.
    pub fn sprite_in_fist(&self, hand: Hand, seat: Mat4, grip: Vec3) -> Mat4 {
        Mat4::from_translation(self.sprite_grips[hand as usize] - seat.transform_point3(grip))
            * seat
    }

    /// What carries `item` from its first-person rest seat to where `hand`'s
    /// fist has taken it — identity while the fist rests in the item's hold.
    pub fn carry(&self, bones: &[Mat4], hand: Hand, item: ItemType) -> Option<Mat4> {
        let grip = *bones.get(self.row.grips[hand as usize])?;
        let hold: Option<ClipId> = self.row.hold(item.render_kind());
        Some(to_view() * grip * self.from_hold(hand, hold))
    }

    /// Append the rig's cubes — the arms — in view-space blocks.
    pub fn bake(
        &self,
        bones: &[Mat4],
        tint: [f32; 3],
        verts: &mut Vec<ItemVertex>,
        indices: &mut Vec<u32>,
    ) {
        bake_model_cubes(
            &self.row.model,
            bones,
            to_view(),
            tint,
            |_| false,
            verts,
            indices,
        );
    }
}

fn to_view() -> Mat4 {
    Mat4::from_scale(Vec3::splat(RIG_PX))
}

/// The hand pass's first-person state: the rig, its animator, and this
/// frame's posed bones.
pub(crate) struct FirstPersonHand {
    pub rig: FirstPersonRig,
    pub bones: Vec<Mat4>,
    driver: driver::Driver,
}

impl FirstPersonHand {
    /// The catalog's viewmodel rig and its animator; `None` when either
    /// failed to load.
    pub fn shipped() -> Option<Self> {
        let (id, row) = rigs::presented(Presenter::Viewmodel)?;
        let graph = row.graph.as_ref()?;
        if row.model.bones().is_empty() {
            return None;
        }
        Some(Self {
            rig: FirstPersonRig::new(row),
            bones: row.model.resolve_local(&[], &[]),
            driver: driver::Driver::new(id, Arc::clone(graph)),
        })
    }

    pub fn reset(&mut self) {
        self.driver.reset();
        self.rig.row.model.resolve_local_into(&[], &[], &mut self.bones);
    }

    /// Advance the animator one frame, `dt` seconds after the last, and pose
    /// the rig. `inputs` are the local player's resolved animator claims and
    /// the graph events fired on it this frame; only this rig's apply here.
    pub fn advance(
        &mut self,
        frames: &[HeldItemFrame; 2],
        motion: &LocalMotion,
        inputs: AnimatorInputs<'_>,
        dt: f32,
    ) {
        self.driver.update(frames, motion, inputs, dt);
        self.driver
            .animator()
            .pose()
            .resolve_into(&self.rig.row.model, &mut self.bones);
    }

    /// The view-space correction the camera bone asks for; the world and
    /// the hand pass both draw through it.
    pub fn view_offset(&self) -> Mat4 {
        self.rig.camera(&self.bones).inverse()
    }
}

#[cfg(test)]
mod tests;
