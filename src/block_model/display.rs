use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bbmodel::euler_quat;

use super::{BlockModelKind, MODELS};

/// One Blockbench `display` transform (rotation degrees, translation in 1/16-block
/// units, scale multiplier, plus the optional rotation/scale pivots in block units) —
/// how the author wants the model posed in a given context (in-hand, in the GUI, …).
/// Cached in the `.llblock` so the held item + inventory icon can pose the model
/// exactly as designed. Default = identity (no display entry). A NEGATIVE scale
/// component is Blockbench's "mirror" checkbox (the sign is saved into the scale).
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
    /// The COMPLETE display transform `T(translation) · R(rotation) · S(scale)` with the
    /// pivot position-corrections, in BLOCK units — element-for-element the matrix
    /// Blockbench's display preview builds in `updateDisplayBase` (display_mode.js) for
    /// the right-hand / unmirrored contexts, so a pose renders exactly as authored.
    /// Model points are expected relative to the authored display pivot (the block
    /// centre — see [`BlockModel::display_pivot`] / [`ModelInstance::display_from_unit`]).
    /// Translation is authored in pixels (16 per block); the pivots in blocks. A zero
    /// scale component degrades to 0.001 exactly as Blockbench does; a negative one
    /// mirrors that axis (the authored "mirror" flag).
    pub fn base_matrix(&self) -> Mat4 {
        let rot = euler_quat(Vec3::from(self.rotation));
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
            // Blockbench rotates the pivot FIRST, then damps it componentwise by
            // (1 - scale) — replicated verbatim, quirks included.
            pos += (rot * sp) * (Vec3::ONE - s);
        }
        Mat4::from_translation(pos) * Mat4::from_quat(rot) * Mat4::from_scale(scale)
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

/// The Blockbench `display` block: the per-context poses we use. Each defaults to
/// identity when the source omits it, so a model with no `display` still renders.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BlockDisplay {
    /// First-person right-hand pose — the HELD item.
    pub firstperson_righthand: DisplayTransform,
    /// Inventory / GUI pose — the slot ICON.
    pub gui: DisplayTransform,
    /// Third-person right-hand pose (cached for completeness; not yet wired).
    pub thirdperson_righthand: DisplayTransform,
    /// On-the-ground pose (cached for completeness; the dropped item keeps its own pose).
    pub ground: DisplayTransform,
}

impl BlockDisplay {
    /// Parse the `.bbmodel`'s `display` object (any context absent → identity).
    pub(super) fn parse(root: &Value) -> Self {
        let d = root.get("display");
        let ctx = |name: &str| {
            d.and_then(|d| d.get(name))
                .map(DisplayTransform::parse)
                .unwrap_or_default()
        };
        BlockDisplay {
            firstperson_righthand: ctx("firstperson_righthand"),
            gui: ctx("gui"),
            thirdperson_righthand: ctx("thirdperson_righthand"),
            ground: ctx("ground"),
        }
    }
}

/// The Blockbench `display` poses for `kind` (cached in the `.llblock`) — the held item
/// reads `firstperson_righthand`, the inventory icon reads `gui`.
#[inline]
pub fn display(kind: BlockModelKind) -> &'static BlockDisplay {
    &MODELS[kind.0 as usize].display
}
