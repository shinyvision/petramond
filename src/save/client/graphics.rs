use serde::{Deserialize, Serialize};

use super::ClientSettings;

/// Scene sampling quality; screen-space UI always renders at native resolution.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AntiAliasing {
    Off,
    #[default]
    Msaa4x,
    Msaa8x,
    Ssaa4x,
    Ssaa16x,
}

impl AntiAliasing {
    /// Every mode, in the order a control that steps through them presents
    /// them: cheapest first, then the two sample-count modes, then the two
    /// supersampling modes. [`Self::index`] and [`Self::from_index`] are the
    /// only way in and out of this order.
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Msaa4x,
        Self::Msaa8x,
        Self::Ssaa4x,
        Self::Ssaa16x,
    ];

    /// This mode's position in [`Self::ALL`].
    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|mode| *mode == self)
            .expect("every mode is listed in ALL")
    }

    /// The mode at `index` in [`Self::ALL`], clamped to the last one.
    pub fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    /// Raster coverage samples per scene pixel; supersampling uses a larger scene.
    pub fn sample_count(self) -> u32 {
        match self {
            Self::Msaa4x => 4,
            Self::Msaa8x => 8,
            _ => 1,
        }
    }

    /// Scene dimensions relative to the native viewport.
    pub fn resolution_multiplier(self) -> u32 {
        match self {
            Self::Off | Self::Msaa4x | Self::Msaa8x => 1,
            Self::Ssaa4x => 2,
            Self::Ssaa16x => 4,
        }
    }
}

/// The renderer-owned knobs of [`ClientSettings`], as ONE value the renderer
/// applies in one call. Every graphics setting reaches the GPU through this
/// and nothing else, so each is live-toggleable and none can be applied at
/// window creation only.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GraphicsSettings {
    /// Streaming radius in chunks; the fog band derives from it.
    pub render_dist: i32,
    /// World-resolution scale with anti-aliasing Off (`0.5..=1.0`).
    pub render_scale: f32,
    /// The colour grade.
    pub grade: bool,
    pub anti_aliasing: AntiAliasing,
    /// Emitter particle density (`0` off, `0.5` reduced, `1` full).
    pub particle_density: f32,
}

impl ClientSettings {
    pub fn graphics(&self) -> GraphicsSettings {
        GraphicsSettings {
            render_dist: self.render_dist,
            render_scale: self.render_scale,
            grade: self.grade,
            anti_aliasing: self.anti_aliasing,
            particle_density: self.particles.density(),
        }
    }
}
