use crate::block::Block;

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct AlbedoMix {
    pub toward: f32,
    pub amount: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct FluidMedium {
    pub fog_color: [f32; 3],
    pub fog_start: f32,
    pub fog_end: f32,
    pub volume_tint: [f32; 3],
    pub surface_tint: [f32; 3],
    pub surface_alpha: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub albedo_mix: Option<AlbedoMix>,
    #[serde(default)]
    pub sheen: bool,
    #[serde(default)]
    pub self_lit: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eye_margin: Option<f32>,
}

impl FluidMedium {
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

pub(crate) struct MediaTable {
    blocks: Vec<Block>,
    by_id: Box<[u32]>,
}

pub(crate) static MEDIA: crate::content::Slot<MediaTable> = crate::content::Slot::new(
    crate::content::stage::FLUID_MEDIA,
    &[crate::content::stage::BLOCKS],
    derive_media,
);

fn derive_media(_: &crate::content::ContentRegistry) -> Result<MediaTable, String> {
    let blocks: Vec<Block> = Block::all()
        .iter()
        .copied()
        .filter(|b| b.fluid_def().is_some())
        .collect();
    let mut by_id = vec![u32::MAX; Block::all().len()].into_boxed_slice();
    for (i, b) in blocks.iter().enumerate() {
        by_id[b.id() as usize] = i as u32;
    }
    Ok(MediaTable { blocks, by_id })
}

pub fn media() -> &'static [Block] {
    &MEDIA.current().blocks
}

#[inline]
pub fn medium_index(block: Block) -> Option<u32> {
    MEDIA
        .current()
        .by_id
        .get(block.id() as usize)
        .copied()
        .filter(|&i| i != u32::MAX)
}
