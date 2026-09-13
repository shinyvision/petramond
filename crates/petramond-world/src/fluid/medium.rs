//! How a fluid looks from inside it and at its surface: row data a renderer reads.

use std::sync::LazyLock;

use crate::block::Block;

/// A grey level the surface albedo is pulled toward, so painted body colour
/// survives the flipbook detail and the vertex tint.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct AlbedoMix {
    pub toward: f32,
    pub amount: f32,
}

/// Presentation of a fluid as a medium. Colours are linear RGB `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct FluidMedium {
    /// Fog and clear colour while the camera eye is inside this fluid.
    pub fog_color: [f32; 3],
    /// Linear fog band (blocks from the eye) while inside.
    pub fog_start: f32,
    pub fog_end: f32,
    /// Multiply over everything drawn (terrain, bodies, particles) while the
    /// eye is inside this fluid — every surface except a fluid's own faces.
    pub volume_tint: [f32; 3],
    /// Multiply on this fluid's faces while the eye is inside ANY fluid, in
    /// place of the eye medium's `volume_tint`.
    pub surface_tint: [f32; 3],
    /// Face alpha; `1.0` is an opaque body that draws and writes depth with
    /// the opaque terrain.
    pub surface_alpha: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub albedo_mix: Option<AlbedoMix>,
    /// Whether the top surface gets the reflective sheen response.
    #[serde(default)]
    pub sheen: bool,
    /// How much of its row's light emission a face provides itself, `0..=1`:
    /// the face's block light is raised toward the emission and its contact
    /// AO lifted by this fraction. A face is lit from the cell in front of it,
    /// which for a glowing body under a ceiling or in a trench is dark rock.
    #[serde(default)]
    pub self_lit: f32,
    /// How far below the open surface the eye must be to count as inside;
    /// `None` counts any eye in one of this fluid's cells.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eye_margin: Option<f32>,
}

impl FluidMedium {
    /// Whether faces of this fluid are opaque (draw with solid terrain).
    #[inline]
    pub fn is_opaque(&self) -> bool {
        self.surface_alpha >= 1.0
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        let unit = |name: &str, v: f32| {
            if v.is_finite() && (0.0..=1.0).contains(&v) {
                Ok(())
            } else {
                Err(format!("fluid medium {name} must be finite and in 0..=1"))
            }
        };
        for (name, rgb) in [
            ("fog_color", self.fog_color),
            ("volume_tint", self.volume_tint),
            ("surface_tint", self.surface_tint),
        ] {
            for v in rgb {
                unit(name, v)?;
            }
        }
        if !(self.fog_start.is_finite()
            && self.fog_end.is_finite()
            && 0.0 <= self.fog_start
            && self.fog_start < self.fog_end
            && self.fog_end <= 1024.0)
        {
            return Err(
                "fluid medium fog band must satisfy 0 <= fog_start < fog_end <= 1024".into(),
            );
        }
        if !(self.surface_alpha.is_finite()
            && self.surface_alpha > 0.0
            && self.surface_alpha <= 1.0)
        {
            return Err("fluid medium surface_alpha must be in (0, 1]".into());
        }
        if let Some(mix) = self.albedo_mix {
            unit("albedo_mix.toward", mix.toward)?;
            unit("albedo_mix.amount", mix.amount)?;
        }
        unit("self_lit", self.self_lit)?;
        if let Some(margin) = self.eye_margin {
            unit("eye_margin", margin)?;
        }
        Ok(())
    }
}

struct MediaTable {
    blocks: Vec<Block>,
    by_id: Box<[u32]>,
}

static MEDIA: LazyLock<MediaTable> = LazyLock::new(|| {
    let blocks: Vec<Block> = Block::all()
        .iter()
        .copied()
        .filter(|b| b.fluid_def().is_some())
        .collect();
    let mut by_id = vec![u32::MAX; Block::all().len()].into_boxed_slice();
    for (i, b) in blocks.iter().enumerate() {
        by_id[b.id() as usize] = i as u32;
    }
    MediaTable { blocks, by_id }
});

/// Every fluid block, in id order: position `i` is medium index `i`, the dense
/// handle a renderer addresses a fluid's [`FluidMedium`] by.
pub fn media() -> &'static [Block] {
    &MEDIA.blocks
}

/// The dense medium index of a fluid block (not of a host holding one).
#[inline]
pub fn medium_index(block: Block) -> Option<u32> {
    MEDIA
        .by_id
        .get(block.id() as usize)
        .copied()
        .filter(|&i| i != u32::MAX)
}
