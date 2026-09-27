use serde::{Deserialize, Serialize};

use super::ClientSettings;

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
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::Msaa4x,
        Self::Msaa8x,
        Self::Ssaa4x,
        Self::Ssaa16x,
    ];

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|mode| *mode == self)
            .expect("every mode is listed in ALL")
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    pub fn sample_count(self) -> u32 {
        match self {
            Self::Msaa4x => 4,
            Self::Msaa8x => 8,
            _ => 1,
        }
    }

    pub fn resolution_multiplier(self) -> u32 {
        match self {
            Self::Off | Self::Msaa4x | Self::Msaa8x => 1,
            Self::Ssaa4x => 2,
            Self::Ssaa16x => 4,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GraphicsSettings {
    pub render_dist: i32,
    pub render_scale: f32,
    pub grade: bool,
    pub anti_aliasing: AntiAliasing,
    pub particle_density: f32,
    pub screen_shake: bool,
}

impl ClientSettings {
    pub fn graphics(&self) -> GraphicsSettings {
        GraphicsSettings {
            render_dist: self.render_dist,
            render_scale: self.render_scale,
            grade: self.grade,
            anti_aliasing: self.anti_aliasing,
            particle_density: self.particles.density(),
            screen_shake: self.screen_shake,
        }
    }
}
