use super::*;

/// An authored vertical profile repeated around every branch attachment.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayeredFoliage {
    layers: Vec<Layer>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Layer {
    y: i32,
    radius: i32,
    ragged: f32,
}

impl LayeredFoliage {
    pub fn validate(&self) -> Result<(), String> {
        use crate::data::bounds::{unit, within};
        if self.layers.is_empty() || self.layers.len() > 8 {
            return Err("foliage profile requires 1..=8 layers".into());
        }
        for layer in &self.layers {
            within("layer y", layer.y, -3..=4)?;
            within("layer radius", layer.radius, 1..=5)?;
            unit("layer ragged", layer.ragged)?;
        }
        Ok(())
    }
}

impl FoliagePlacer for LayeredFoliage {
    fn place(
        &self,
        ctx: &mut FeatureCtx,
        open: &mut dyn FnMut(IVec3) -> bool,
        trunk: &TrunkPlan,
        leaf: Block,
        rng: &mut FeatureRng,
    ) {
        let mut canopy = Canopy::new();
        for attach in &trunk.attach {
            for layer in &self.layers {
                leaf_layer(
                    &mut canopy,
                    attach.x,
                    attach.y + layer.y,
                    attach.z,
                    layer.radius,
                    layer.ragged,
                    rng,
                );
            }
        }
        canopy.commit(ctx, open, &trunk.logs, leaf);
    }

    fn horizontal_reach(&self) -> i32 {
        self.layers.iter().map(|l| l.radius).max().unwrap_or(0)
    }
}
