use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bbmodel::display_euler_quat;

use super::{models, BlockModelKind};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayTransform {
    pub rotation: [f32; 3],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
    pub rotation_pivot: [f32; 3],
    pub scale_pivot: [f32; 3],
}

impl Default for DisplayTransform {
    fn default() -> Self {
        DisplayTransform {
            rotation: [0.0; 3],
            translation: [0.0; 3],
            scale: [1.0; 3],
            rotation_pivot: [0.0; 3],
            scale_pivot: [0.0; 3],
        }
    }
}

impl DisplayTransform {
    pub fn base_matrix(&self) -> Mat4 {
        let rot = display_euler_quat(Vec3::from(self.rotation));
        let s = Vec3::from(self.scale);
        let guarded = |v: f32| if v == 0.0 { 0.001 } else { v };
        let scale = Vec3::new(guarded(s.x), guarded(s.y), guarded(s.z));
        let mut pos = Vec3::from(self.translation) / 16.0;
        let rp = Vec3::from(self.rotation_pivot);
        if rp != Vec3::ZERO {
            pos -= rot * rp - rp;
        }
        let sp = Vec3::from(self.scale_pivot);
        if sp != Vec3::ZERO {
            // Blockbench rotates the pivot first, then damps it componentwise by
            // (1 - scale). Copied verbatim, quirks and all.
            pos += (rot * sp) * (Vec3::ONE - s);
        }
        Mat4::from_translation(pos) * Mat4::from_quat(rot) * Mat4::from_scale(scale)
    }

    pub fn left_hand(&self) -> DisplayTransform {
        DisplayTransform {
            rotation: [self.rotation[0], -self.rotation[1], -self.rotation[2]],
            translation: [
                -self.translation[0],
                self.translation[1],
                self.translation[2],
            ],
            ..*self
        }
    }

    fn parse(v: &Value) -> Self {
        let read = |key: &str, default: [f32; 3]| -> [f32; 3] {
            match v.get(key).and_then(Value::as_array) {
                Some(a) if a.len() == 3 => [
                    a[0].as_f64().unwrap_or(default[0] as f64) as f32,
                    a[1].as_f64().unwrap_or(default[1] as f64) as f32,
                    a[2].as_f64().unwrap_or(default[2] as f64) as f32,
                ],
                _ => default,
            }
        };
        DisplayTransform {
            rotation: read("rotation", [0.0; 3]),
            translation: read("translation", [0.0; 3]),
            scale: read("scale", [1.0; 3]),
            rotation_pivot: read("rotation_pivot", [0.0; 3]),
            scale_pivot: read("scale_pivot", [0.0; 3]),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BlockDisplay {
    pub firstperson_righthand: DisplayTransform,
    pub firstperson_lefthand: Option<DisplayTransform>,
    pub gui: DisplayTransform,
    pub thirdperson_righthand: DisplayTransform,
    pub thirdperson_lefthand: Option<DisplayTransform>,
    pub ground: DisplayTransform,
}

impl BlockDisplay {
    pub(super) fn parse(root: &Value) -> Self {
        let d = root.get("display");
        let ctx = |name: &str| {
            d.and_then(|d| d.get(name))
                .map(DisplayTransform::parse)
                .unwrap_or_default()
        };
        let opt_ctx = |name: &str| d.and_then(|d| d.get(name)).map(DisplayTransform::parse);
        BlockDisplay {
            firstperson_righthand: ctx("firstperson_righthand"),
            firstperson_lefthand: opt_ctx("firstperson_lefthand"),
            gui: ctx("gui"),
            thirdperson_righthand: ctx("thirdperson_righthand"),
            thirdperson_lefthand: opt_ctx("thirdperson_lefthand"),
            ground: ctx("ground"),
        }
    }
}

#[inline]
pub fn display(kind: BlockModelKind) -> &'static BlockDisplay {
    &models()[kind.0 as usize].display
}
