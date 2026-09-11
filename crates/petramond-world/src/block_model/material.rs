//! Texture-region appearance, resolved once when model templates are baked.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameStrip {
    pub frames: u16,
    pub frame_ticks: u16,
    #[serde(default)]
    pub interpolate: bool,
}

/// A region in the model's combined texture sheet, using normalized UVs.
/// Animated faces address the first frame; the region includes the entire strip.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceMaterial {
    pub rect: [f32; 4],
    #[serde(default = "white")]
    pub tint: [u8; 3],
    #[serde(default)]
    pub animation: Option<FrameStrip>,
    #[serde(default)]
    pub unlit: bool,
    #[serde(default = "enabled")]
    pub shade: bool,
    #[serde(default = "enabled")]
    pub ambient_occlusion: bool,
}

fn white() -> [u8; 3] {
    [255; 3]
}
fn enabled() -> bool {
    true
}

pub(super) fn validate(rows: &[SurfaceMaterial]) -> Result<(), String> {
    for (i, row) in rows.iter().enumerate() {
        let [x0, y0, x1, y1] = row.rect;
        if row
            .rect
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || x0 >= x1
            || y0 >= y1
        {
            return Err("model surface needs a nonempty normalized texture rectangle".into());
        }
        if row
            .animation
            .is_some_and(|a| a.frames < 2 || a.frame_ticks == 0)
        {
            return Err(
                "model animation needs at least two frames and positive frame_ticks".into(),
            );
        }
        if rows[..i]
            .iter()
            .any(|p| x0 < p.rect[2] && x1 > p.rect[0] && y0 < p.rect[3] && y1 > p.rect[1])
        {
            return Err("model surface rectangles overlap".into());
        }
    }
    Ok(())
}

/// An atlas-space flipbook. Slot zero is the static identity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureAnimation {
    pub stride: f32,
    pub frames: u16,
    pub frame_ticks: u16,
    pub interpolate: bool,
}

impl Default for TextureAnimation {
    fn default() -> Self {
        Self {
            stride: 0.0,
            frames: 1,
            frame_ticks: 1,
            interpolate: false,
        }
    }
}

/// Static albedo and the atlas's animation slot; neither changes mesh geometry.
#[derive(Clone, Copy, Debug)]
pub struct FaceAppearance {
    pub tint: [u8; 3],
    pub animation: u8,
    pub unlit: bool,
    pub shade: bool,
    pub ambient_occlusion: bool,
}

impl Default for FaceAppearance {
    fn default() -> Self {
        Self {
            tint: white(),
            animation: 0,
            unlit: false,
            shade: true,
            ambient_occlusion: true,
        }
    }
}

impl FaceAppearance {
    pub fn shading(self, directional: f32, occlusion: f32) -> f32 {
        if self.unlit {
            return 1.0;
        }
        let directional = if self.shade { directional } else { 1.0 };
        directional
            * if self.ambient_occlusion {
                occlusion
            } else {
                1.0
            }
    }

    /// RGB multiply with an optional per-cell tint, packed below the animation byte.
    pub fn packed(self, cell_tint: Option<u32>) -> u32 {
        let color = self.tint.map(u32::from);
        let [r, g, b] = match cell_tint {
            Some(t) => [
                color[0] * ((t >> 16) & 255) / 255,
                color[1] * ((t >> 8) & 255) / 255,
                color[2] * (t & 255) / 255,
            ],
            None => color,
        };
        (u32::from(self.animation) << 24) | (r << 16) | (g << 8) | b
    }
}

#[cfg(test)]
mod tests;
