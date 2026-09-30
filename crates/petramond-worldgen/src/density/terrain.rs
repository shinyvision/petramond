use crate::data::terrain::TerrainRecipe;
use crate::graph::ScalarGraph;

pub mod channels {
    pub const TEMPERATURE: &str = "temperature";
    pub const HUMIDITY: &str = "humidity";
    pub const CONTINENTALITY: &str = "continentality";
    pub const EROSION: &str = "erosion";
    pub const VARIANCE: &str = "variance";
    pub const RIDGE: &str = "ridge";
    pub const BASE_HEIGHT: &str = "base_height";
    pub const MASTER_DENSITY: &str = "master_density";
    pub const SURFACE_DETECTION: &str = "surface_detection";
}

#[derive(Clone, Copy)]
pub struct TerrainDensitySpec {
    recipe: &'static TerrainRecipe,
}

impl TerrainDensitySpec {
    pub fn default_surface() -> Self {
        Self {
            recipe: crate::data::terrain::recipe(),
        }
    }

    pub fn build_graph(&self, seed: u32) -> TerrainDensityGraph {
        TerrainDensityGraph {
            graph: self.recipe.build(seed),
        }
    }
}

#[derive(Debug)]
pub struct TerrainDensityGraph {
    graph: ScalarGraph,
}

impl Clone for TerrainDensityGraph {
    fn clone(&self) -> Self {
        Self {
            graph: self.graph.clone(),
        }
    }
}

impl TerrainDensityGraph {
    pub fn graph(&self) -> &ScalarGraph {
        &self.graph
    }

    #[cfg(test)]
    pub fn graph_mut(&mut self) -> &mut ScalarGraph {
        &mut self.graph
    }
}

#[cfg(test)]
pub(crate) mod reference;

#[cfg(test)]
mod tests {
    use super::reference::{
        FloorDensitySpec, ReferenceTerrainSpec, ShapingSplineSpecs, DEPTH_OFFSET_BIAS, HEIGHT_SCALE,
    };
    use super::*;
    use crate::density::shaper;
    use crate::graph::spline::CubicSpline;
    use crate::graph::SamplePoint;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1.0e-10,
            "expected {expected}, got {actual}"
        );
    }

    fn flat_spec(base_height: f64) -> ReferenceTerrainSpec {
        ReferenceTerrainSpec {
            shaping: ShapingSplineSpecs {
                offset: CubicSpline::constant(
                    shaper::axes::CONTINENTALITY,
                    (base_height - HEIGHT_SCALE * (1.0 - DEPTH_OFFSET_BIAS)) / HEIGHT_SCALE,
                ),
            },
            floor: FloorDensitySpec::new(0.0, 8.0, 64.0),
        }
    }

    #[test]
    fn climate_and_shaping_channels_are_horizontal_only() {
        let density = TerrainDensitySpec::default_surface().build_graph(0x1234_5678);
        let low = SamplePoint::new(137.25, 24.0, -291.75);
        let high = SamplePoint::new(137.25, 128.0, -291.75);

        for channel in [
            channels::TEMPERATURE,
            channels::HUMIDITY,
            channels::CONTINENTALITY,
            channels::EROSION,
            channels::VARIANCE,
            channels::RIDGE,
            channels::BASE_HEIGHT,
        ] {
            assert_eq!(
                density.graph().channel_depends_on_y(channel),
                Some(false),
                "{channel} should be marked Y-invariant"
            );
            assert_close(
                density.graph().evaluate_channel(channel, low).unwrap(),
                density.graph().evaluate_channel(channel, high).unwrap(),
            );
        }

        assert_eq!(
            density
                .graph()
                .channel_depends_on_y(channels::MASTER_DENSITY),
            Some(true)
        );
        let low_density = density
            .graph()
            .evaluate_channel(channels::MASTER_DENSITY, low)
            .unwrap();
        let high_density = density
            .graph()
            .evaluate_channel(channels::MASTER_DENSITY, high)
            .unwrap();
        assert!(
            (low_density - high_density).abs() > 1.0e-6,
            "master density must still depend on sample Y"
        );
    }

    #[test]
    fn master_density_sign_tracks_base_height() {
        let density = flat_spec(64.0).build_graph(99);

        let below = density
            .graph()
            .evaluate_channel(channels::MASTER_DENSITY, SamplePoint::new(0.0, 63.0, 0.0))
            .unwrap();
        let at = density
            .graph()
            .evaluate_channel(channels::MASTER_DENSITY, SamplePoint::new(0.0, 64.0, 0.0))
            .unwrap();
        let above = density
            .graph()
            .evaluate_channel(channels::MASTER_DENSITY, SamplePoint::new(0.0, 65.0, 0.0))
            .unwrap();

        assert!(below > 0.0, "density below base height should be solid");
        assert_close(at, 0.0);
        assert!(above < 0.0, "density above base height should be air");
    }

    #[test]
    fn floor_clamp_fades_bottom_levels_toward_fixed_solid_density() {
        let density = flat_spec(32.0).build_graph(7);

        assert_close(
            density
                .graph()
                .evaluate_channel(channels::MASTER_DENSITY, SamplePoint::new(0.0, 0.0, 0.0))
                .unwrap(),
            64.0,
        );
        let faded = density
            .graph()
            .evaluate_channel(channels::MASTER_DENSITY, SamplePoint::new(0.0, 4.0, 0.0))
            .unwrap();
        let unfaded = density
            .graph()
            .evaluate_channel(channels::MASTER_DENSITY, SamplePoint::new(0.0, 8.0, 0.0))
            .unwrap();
        assert!(faded > unfaded);
        assert_close(unfaded, 24.0);
    }
}
