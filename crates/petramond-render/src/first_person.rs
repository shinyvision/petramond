//! The first-person viewmodel: the rigs catalog's viewmodel rig, posed by the
//! client's animation and baked in VIEW space for the hand pass — the arms in the
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

use glam::{Mat4, Vec3};
use petramond::player::rigs::{self, Presenter, Rig};
use petramond_anim::{ClipId, LocalPose, MirrorMap};
use petramond_world::item::ItemType;

use super::item_model::ItemVertex;
use super::mob_model::bake_model_cubes;

pub(crate) const RIG_PX: f32 = 1.0 / 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hand {
    Main,
    Off,
}

pub(crate) struct FirstPersonRig {
    pub row: &'static Rig,
    rest: [Mat4; 2],
    holds: [Vec<(ClipId, Mat4)>; 2],
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
            let sprite = row
                .holds
                .iter()
                .find(|(kind, _)| *kind == "sprite")
                .map(|(_, clip)| *clip);
            rig.hold_inverse(hand, sprite)
                .inverse()
                .transform_point3(pivot)
        });
        rig
    }

    fn hold_inverse(&self, hand: Hand, clip: Option<ClipId>) -> Mat4 {
        clip.and_then(|clip| self.holds[hand as usize].iter().find(|(c, _)| *c == clip))
            .map_or(self.rest[hand as usize], |(_, m)| *m)
    }

    pub fn camera(&self, bones: &[Mat4]) -> Mat4 {
        match self.row.camera.and_then(|b| bones.get(b)) {
            Some(bone) => to_view() * *bone * to_view().inverse(),
            None => Mat4::IDENTITY,
        }
    }

    pub fn sprite_in_fist(&self, hand: Hand, seat: Mat4, grip: Vec3) -> Mat4 {
        Mat4::from_translation(self.sprite_grips[hand as usize] - seat.transform_point3(grip))
            * seat
    }

    pub fn carry(&self, bones: &[Mat4], hand: Hand, item: ItemType) -> Option<Mat4> {
        let grip = *bones.get(self.row.grips[hand as usize])?;
        let hold: Option<ClipId> = self.row.hold(item.render_kind());
        Some(to_view() * grip * self.hold_inverse(hand, hold))
    }

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
            None,
            verts,
            indices,
        );
    }
}

fn to_view() -> Mat4 {
    Mat4::from_scale(Vec3::splat(RIG_PX))
}

pub(crate) struct FirstPersonHand {
    pub rig: FirstPersonRig,
    pub bones: Vec<Mat4>,
}

impl FirstPersonHand {
    pub fn shipped() -> Option<Self> {
        let (_, row) = rigs::presented(Presenter::Viewmodel)?;
        row.graph.as_ref()?;
        if row.model.bones().is_empty() {
            return None;
        }
        Some(Self {
            rig: FirstPersonRig::new(row),
            bones: row.model.resolve_local(&[], &[]),
        })
    }

    pub fn reset(&mut self) {
        self.rig
            .row
            .model
            .resolve_local_into(&[], &[], &mut self.bones);
    }

    pub fn set_bones(&mut self, bones: &[Mat4]) {
        if bones.len() == self.bones.len() {
            self.bones.copy_from_slice(bones);
        }
    }

    pub fn view_offset(&self) -> Mat4 {
        self.rig.camera(&self.bones).inverse()
    }
}

#[cfg(test)]
mod tests;
