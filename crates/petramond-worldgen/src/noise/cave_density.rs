use crate::density::noise::{build_climate_field, ClimateFieldParams, ReferenceDoublePerlin};

const CAVERN_DENSITY_BIAS: f64 = 0.35;

pub(super) const BATCH: usize = 64;

/// The noise fields [`CaveDensity::sample_batch`] reads per point.
const FIELDS: usize = 20;

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

#[derive(Clone, Copy, Default)]
pub(super) struct Sample {
    pub entrance: f64,
    pub interior: f64,
    pub noodle: [f64; 4],
    #[cfg(test)]
    pub chamber_live: bool,
}

impl Sample {
    /// A sample read only through its entrance lane: what a point above the
    /// interior band holds.
    pub(super) fn entrance_only(entrance: f64) -> Self {
        Self {
            entrance,
            interior: 1.0,
            noodle: [0.0, 0.0, -1.0, 0.0],
            #[cfg(test)]
            chamber_live: false,
        }
    }

    /// Applies rooms and the floor fade at height `y`.
    pub(super) fn finish(&mut self, y: f64, excavation: (f64, f64)) {
        #[cfg(test)]
        {
            self.chamber_live = excavation.0 > 0.0;
        }
        if excavation.1 > 0.0 {
            self.interior = self.interior.min(self.entrance - 0.035 * excavation.1);
        }
        let room = excavation.0.clamp(0.0, 1.0);
        self.interior = self
            .interior
            .min(self.interior + (-0.6 - self.interior) * room);
        let floor = ((y - super::settings::CAVE_MIN_Y as f64) / super::settings::CAVE_FLOOR_FADE)
            .clamp(0.0, 1.0);
        self.interior = 0.12 + (self.interior - 0.12) * floor;
    }
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

    /// Samples every point of a batch (at most [`BATCH`]), each exactly as a
    /// lone point would be: one field at a time over the whole batch, so a
    /// field's octave tables stay hot and neighbouring points share cells.
    pub(super) fn sample_batch(
        &self,
        p: &[[f64; 3]],
        depth: &[f64],
        excavation: &[(f64, f64)],
        out: &mut [Sample],
    ) {
        let n = p.len();
        assert!(n <= BATCH && depth.len() == n && excavation.len() == n && out.len() == n);
        let mut coords = [[0.0; BATCH]; 3];
        let mut field = |noise: &Noise,
                         xz: &dyn Fn(usize) -> f64,
                         y: &dyn Fn(usize) -> f64,
                         values: &mut [f64; BATCH]| {
            let [xs, ys, zs] = &mut coords;
            for i in 0..n {
                xs[i] = p[i][0] * xz(i);
                ys[i] = p[i][1] * y(i);
                zs[i] = p[i][2] * xz(i);
            }
            noise
                .0
                .sample_many(&xs[..n], &ys[..n], &zs[..n], &mut values[..n]);
        };
        let mut v = [[0.0; BATCH]; FIELDS];
        let [width, rarity, ridge0, ridge1, roughness, gain, mouth, horizontal_width, horizontal_rarity, horizontal, elevation, layer, cheese, pillar, pillar_rarity, pillar_width, noodle_a, noodle_b, noodle_toggle, noodle_width] =
            &mut v;
        field(&self.tunnel_width, &|_| 1.0, &|_| 1.0, width);
        field(&self.tunnel_rarity, &|_| 2.0, &|_| 1.0, rarity);
        for r in rarity.iter_mut().take(n) {
            *r = select(*r, &[-0.5, 0.0, 0.5], &[0.75, 1.0, 1.5, 2.0]);
        }
        let scale = |i: usize| rarity[i].recip();
        field(&self.tunnel[0], &scale, &scale, ridge0);
        field(&self.tunnel[1], &scale, &scale, ridge1);
        field(&self.roughness, &|_| 1.0, &|_| 1.0, roughness);
        field(&self.roughness_gain, &|_| 1.0, &|_| 1.0, gain);
        field(&self.entrance, &|_| 0.75, &|_| 0.5, mouth);
        let mut rough = [0.0; BATCH];
        for i in 0..n {
            let width = map(width[i], 0.065, 0.088);
            let ridge = [ridge0[i], ridge1[i]]
                .iter()
                .map(|r| r.abs() * rarity[i])
                .fold(0.0, f64::max);
            rough[i] = (roughness[i].abs() - 0.4) * map(gain[i], 0.0, -0.1);
            let tunnel = ridge - width + rough[i];
            let mouth = mouth[i] + 0.37 + 0.3 * (1.0 - ((p[i][1] + 10.0) / 40.0).clamp(0.0, 1.0));
            out[i] = Sample::entrance_only(mouth.min(tunnel));
        }
        field(&self.horizontal_width, &|_| 2.0, &|_| 1.0, horizontal_width);
        field(
            &self.horizontal_rarity,
            &|_| 2.0,
            &|_| 1.0,
            horizontal_rarity,
        );
        for i in 0..n {
            horizontal_width[i] = map(horizontal_width[i], 0.6, 1.3);
            horizontal_rarity[i] = select(
                horizontal_rarity[i],
                &[-0.75, -0.5, 0.5, 0.75],
                &[0.5, 0.75, 1.0, 2.0, 3.0],
            );
        }
        let scale = |i: usize| horizontal_rarity[i].recip();
        field(&self.horizontal, &scale, &scale, horizontal);
        field(&self.horizontal_height, &|_| 1.0, &|_| 0.0, elevation);
        field(&self.layer, &|_| 1.0, &|_| 8.0, layer);
        field(&self.cheese, &|_| 1.0, &|_| 2.0 / 3.0, cheese);
        field(&self.pillar, &|_| 25.0, &|_| 0.3, pillar);
        field(&self.pillar_rarity, &|_| 1.0, &|_| 1.0, pillar_rarity);
        field(&self.pillar_width, &|_| 1.0, &|_| 1.0, pillar_width);
        field(&self.noodle[0], &|_| 8.0 / 3.0, &|_| 8.0 / 3.0, noodle_a);
        field(&self.noodle[1], &|_| 8.0 / 3.0, &|_| 8.0 / 3.0, noodle_b);
        field(&self.noodle_toggle, &|_| 1.0, &|_| 1.0, noodle_toggle);
        field(&self.noodle_width, &|_| 1.0, &|_| 1.0, noodle_width);
        for i in 0..n {
            let horizontal_ridge = horizontal[i].abs() * horizontal_rarity[i];
            let elevation = elevation[i] * 64.0;
            let horizontal = (horizontal_ridge - 0.083 * horizontal_width[i])
                .max(((p[i][1] - elevation).abs() / 8.0 - horizontal_width[i]).powi(3))
                + rough[i];
            let layers = layer[i].powi(2) * 4.0;
            let roof = (1.5 - depth[i] * 12.8).clamp(0.0, 0.5);
            let cavern = (cheese[i] + CAVERN_DENSITY_BIAS).clamp(-1.0, 1.0) + layers + roof;
            let pillar = (2.0 * pillar[i] + map(pillar_rarity[i], 0.0, -2.0))
                * map(pillar_width[i], 0.0, 1.1).powi(3);
            let mut value = cavern.min(out[i].entrance).min(horizontal);
            if pillar >= 0.03 {
                value = value.max(pillar);
            }
            out[i].interior = value;
            out[i].noodle = [
                noodle_a[i],
                noodle_b[i],
                noodle_toggle[i],
                map(noodle_width[i], 0.05, 0.1),
            ];
        }
        for i in 0..n {
            out[i].finish(p[i][1], excavation[i]);
        }
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
