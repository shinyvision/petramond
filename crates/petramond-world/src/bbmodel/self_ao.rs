//! Self ambient occlusion for an animated model, baked once at its rest pose.
//!
//! A creature's parts move, so no bake is exact for every frame — but what
//! gives a body of many cuboids its depth is the darkening where parts MEET
//! (a limb at its socket, a plate on a torso), and parts that meet stay
//! together through any rotation about their joint. The rest pose gets those
//! right for good; a swinging arm carries the soft shade of the flank it
//! hangs beside, which reads as depth rather than error at this reach.

use glam::Mat4;

use super::{euler_quat, Model};
use crate::block_model::{bake_box_ao, AoBox};

/// The response curve for bodies (see `bake_box_ao`): a creature is seen from
/// a few blocks off and in motion, where the linear response tuned on
/// furniture at arm's length reads as nothing. The darkest corner is no
/// darker; the shade reaches further out of it.
const BODY_CURVE: f32 = 0.5;

impl Model {
    /// Per-cube, per-face (`Face::ALL`), per-corner (`face_corners` order)
    /// shade multipliers at the rest pose. `world_px` is one world pixel
    /// (1/16 block) in model units — `1 / (16 × the species' scale)`.
    /// `casts` = whether the cube at an index may darken the others: parts that can be
    /// hidden (a fleece) must not leave their shade on what they covered.
    pub fn rest_self_ao(&self, world_px: f32, casts: impl Fn(usize) -> bool) -> Vec<[[f32; 4]; 6]> {
        let rest = self.rest_pose();
        let boxes: Vec<AoBox> = self
            .cubes
            .iter()
            .enumerate()
            .map(|(index, cube)| AoBox {
                from: cube.from,
                to: cube.to,
                pose: rest.get(cube.bone).copied().unwrap_or(Mat4::IDENTITY)
                    * Mat4::from_translation(cube.origin)
                    * Mat4::from_quat(euler_quat(cube.rotation))
                    * Mat4::from_translation(-cube.origin),
                faces: cube.faces.map(|f| f.is_some()),
                casts: casts(index),
            })
            .collect();
        bake_box_ao(&boxes, world_px, BODY_CURVE, |_, _, _, _, _| true)
    }
}
