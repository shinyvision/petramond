//! Natural cave density. Negative values remove rock; positive values retain it.

use crate::density::noise::{build_climate_field, ClimateFieldParams, ReferenceDoublePerlin};

const CAVERN_DENSITY_BIAS: f64 = 0.35;

#[derive(Clone)]
struct Noise(ReferenceDoublePerlin);

impl Noise {
    fn new(seed: u32, salt: u64, octave: i32, amplitudes: &'static [f64]) -> Self {
        Self(build_climate_field(
            seed as u64,
            &ClimateFieldParams {
                salt: (salt, salt.rotate_left(29) ^ 0x39a2_f76d_810c_b5e4),
                omin: octave,
                amplitudes,
            },
        ))
    }

    fn at(&self, p: [f64; 3], xz: f64, y: f64) -> f64 {
        self.0.sample(p[0] * xz, p[1] * y, p[2] * xz)
    }
}

pub(super) struct CaveDensity {
    tunnel: [Noise; 2],
    tunnel_rarity: Noise,
    tunnel_width: Noise,
    horizontal: Noise,
    horizontal_rarity: Noise,
    horizontal_height: Noise,
    horizontal_width: Noise,
    roughness: Noise,
    roughness_gain: Noise,
    entrance: Noise,
    cheese: Noise,
    layer: Noise,
    pillar: Noise,
    pillar_rarity: Noise,
    pillar_width: Noise,
    noodle: [Noise; 2],
    noodle_toggle: Noise,
    noodle_width: Noise,
}

#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub entrance: f64,
    pub interior: f64,
    pub noodle: [f64; 4],
    #[cfg(test)]
    pub chamber_live: bool,
}

impl CaveDensity {
    pub(super) fn new(seed: u32) -> Self {
        let noise = |salt, octave, amplitudes| Noise::new(seed, salt, octave, amplitudes);
        Self {
            tunnel: [noise(0xCA01, -7, &[1.0]), noise(0xCA02, -7, &[1.0])],
            tunnel_rarity: noise(0xCA03, -11, &[1.0]),
            tunnel_width: noise(0xCA04, -8, &[1.0]),
            horizontal: noise(0xCA05, -7, &[1.0]),
            horizontal_rarity: noise(0xCA06, -11, &[1.0]),
            horizontal_height: noise(0xCA07, -8, &[1.0]),
            horizontal_width: noise(0xCA08, -11, &[1.0]),
            roughness: noise(0xCA09, -5, &[1.0]),
            roughness_gain: noise(0xCA0A, -8, &[1.0]),
            entrance: noise(0xCA0B, -7, &[0.4, 0.5, 1.0]),
            cheese: noise(0xCA0C, -8, &[0.5, 1.0, 2.0, 1.0, 2.0, 1.0, 0.0, 2.0, 0.0]),
            layer: noise(0xCA0D, -8, &[1.0]),
            pillar: noise(0xCA0E, -7, &[1.0, 1.0]),
            pillar_rarity: noise(0xCA0F, -8, &[1.0]),
            pillar_width: noise(0xCA10, -8, &[1.0]),
            noodle: [noise(0xCA11, -7, &[1.0]), noise(0xCA12, -7, &[1.0])],
            noodle_toggle: noise(0xCA13, -8, &[1.0]),
            noodle_width: noise(0xCA14, -8, &[1.0]),
        }
    }

    fn compute(&self, p: [f64; 3], depth: f64, interior: bool) -> Sample {
        let width = map(self.tunnel_width.at(p, 1.0, 1.0), 0.065, 0.088);
        let rarity = select(
            self.tunnel_rarity.at(p, 2.0, 1.0),
            &[-0.5, 0.0, 0.5],
            &[0.75, 1.0, 1.5, 2.0],
        );
        let ridge = self
            .tunnel
            .iter()
            .map(|n| n.at(p, rarity.recip(), rarity.recip()).abs() * rarity)
            .fold(0.0, f64::max);
        let rough = (self.roughness.at(p, 1.0, 1.0).abs() - 0.4)
            * map(self.roughness_gain.at(p, 1.0, 1.0), 0.0, -0.1);
        let tunnel = ridge - width + rough;
        let mouth = self.entrance.at(p, 0.75, 0.5)
            + 0.37
            + 0.3 * (1.0 - ((p[1] + 10.0) / 40.0).clamp(0.0, 1.0));
        let entrance = mouth.min(tunnel);
        if !interior {
            return Sample {
                entrance,
                interior: 1.0,
                noodle: [0.0, 0.0, -1.0, 0.0],
                #[cfg(test)]
                chamber_live: false,
            };
        }

        let horizontal_width = map(self.horizontal_width.at(p, 2.0, 1.0), 0.6, 1.3);
        let horizontal_rarity = select(
            self.horizontal_rarity.at(p, 2.0, 1.0),
            &[-0.75, -0.5, 0.5, 0.75],
            &[0.5, 0.75, 1.0, 2.0, 3.0],
        );
        let horizontal_ridge = self
            .horizontal
            .at(p, horizontal_rarity.recip(), horizontal_rarity.recip())
            .abs()
            * horizontal_rarity;
        let elevation = self.horizontal_height.at(p, 1.0, 0.0) * 64.0;
        let horizontal = (horizontal_ridge - 0.083 * horizontal_width)
            .max(((p[1] - elevation).abs() / 8.0 - horizontal_width).powi(3))
            + rough;

        // Layer density retains shelves between large voids. Pillars retain rock
        // through those voids; small tunnels remain a separate subtraction.
        let layers = self.layer.at(p, 1.0, 8.0).powi(2) * 4.0;
        let roof = (1.5 - depth * 12.8).clamp(0.0, 0.5);
        let cavern = (self.cheese.at(p, 1.0, 2.0 / 3.0) + CAVERN_DENSITY_BIAS).clamp(-1.0, 1.0)
            + layers
            + roof;
        let pillar = (2.0 * self.pillar.at(p, 25.0, 0.3)
            + map(self.pillar_rarity.at(p, 1.0, 1.0), 0.0, -2.0))
            * map(self.pillar_width.at(p, 1.0, 1.0), 0.0, 1.1).powi(3);
        let mut interior = cavern.min(entrance).min(horizontal);
        if pillar >= 0.03 {
            interior = interior.max(pillar);
        }
        Sample {
            entrance,
            interior,
            #[cfg(test)]
            chamber_live: false,
            noodle: [
                self.noodle[0].at(p, 8.0 / 3.0, 8.0 / 3.0),
                self.noodle[1].at(p, 8.0 / 3.0, 8.0 / 3.0),
                self.noodle_toggle.at(p, 1.0, 1.0),
                map(self.noodle_width.at(p, 1.0, 1.0), 0.05, 0.1),
            ],
        }
    }

    pub(super) fn sample(
        &self,
        p: [f64; 3],
        depth: f64,
        excavation: (f64, f64),
        interior: bool,
    ) -> Sample {
        let mut sample = self.compute(p, depth, interior);
        #[cfg(test)]
        {
            sample.chamber_live = excavation.0 > 0.0;
        }
        if excavation.1 > 0.0 {
            sample.interior = sample.interior.min(sample.entrance - 0.035 * excavation.1);
        }
        let room = excavation.0.clamp(0.0, 1.0);
        sample.interior = sample
            .interior
            .min(sample.interior + (-0.6 - sample.interior) * room);
        let floor = ((p[1] - super::settings::CAVE_MIN_Y as f64)
            / super::settings::CAVE_FLOOR_FADE)
            .clamp(0.0, 1.0);
        sample.interior = 0.12 + (sample.interior - 0.12) * floor;
        sample
    }

    pub(super) fn knead(&self, p: [f64; 3]) -> f64 {
        self.roughness.at(p, 1.0, 1.0)
    }
}

fn map(value: f64, lo: f64, hi: f64) -> f64 {
    lo + (hi - lo) * (value + 1.0) * 0.5
}

fn select(value: f64, edges: &[f64], values: &[f64]) -> f64 {
    values[edges.partition_point(|&edge| value >= edge)]
}
