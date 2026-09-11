use crate::{data::excavations::Connections, rng::FeatureRng};

const SEGMENTS: usize = 8;

#[derive(Clone, Copy)]
pub(super) struct Passage {
    points: [[f64; 3]; SEGMENTS + 1],
    radii: [f64; SEGMENTS + 1],
    flatten: f64,
    feather: f64,
    lo: [f64; 3],
    hi: [f64; 3],
}

impl Passage {
    #[cfg(test)]
    pub(super) fn points(&self) -> &[[f64; 3]] {
        &self.points
    }

    pub(super) fn between(
        a: [i32; 3],
        b: [i32; 3],
        shape: Connections,
        rng: &mut FeatureRng,
    ) -> Self {
        let a = a.map(f64::from);
        let b = b.map(f64::from);
        let offset = shape.bend * rng.next_i32(-1000, 1000) as f64 / 1000.0;
        let mut control = std::array::from_fn::<_, 3, _>(|i| (a[i] + b[i]) * 0.5);
        control[0] -= (b[2] - a[2]) * offset;
        control[2] += (b[0] - a[0]) * offset;
        let narrow = shape.radius[0];
        let wide = shape.radius[1];
        let mut points = [[0.0; 3]; SEGMENTS + 1];
        let mut radii = [0.0; SEGMENTS + 1];
        let phase = rng.next_i32(0, 65535) as f64 * std::f64::consts::TAU / 65536.0;
        for i in 0..=SEGMENTS {
            let t = i as f64 / SEGMENTS as f64;
            points[i] = std::array::from_fn(|axis| {
                (1.0 - t).powi(2) * a[axis] + 2.0 * t * (1.0 - t) * control[axis] + t * t * b[axis]
            });
            radii[i] =
                narrow + (wide - narrow) * (0.5 + 0.5 * (phase + t * std::f64::consts::TAU).sin());
        }
        let pad = [
            wide + shape.feather,
            wide * shape.flatten + shape.feather,
            wide + shape.feather,
        ];
        let lo = std::array::from_fn(|axis| {
            points.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min) - pad[axis]
        });
        let hi = std::array::from_fn(|axis| {
            points
                .iter()
                .map(|p| p[axis])
                .fold(f64::NEG_INFINITY, f64::max)
                + pad[axis]
        });
        Self {
            points,
            radii,
            flatten: shape.flatten,
            feather: shape.feather,
            lo,
            hi,
        }
    }

    pub(super) fn reaches(&self, lo: [i32; 3], hi: [i32; 3]) -> bool {
        (0..3).all(|i| self.hi[i] >= lo[i] as f64 && self.lo[i] <= hi[i] as f64)
    }

    pub(super) fn at(&self, p: [i32; 3], knead: f64) -> f64 {
        if !self.reaches(p, p) {
            return 0.0;
        }
        let scaled = |p: [f64; 3]| [p[0], p[1] / self.flatten, p[2]];
        let p = scaled(p.map(f64::from));
        let mut value: f64 = 0.0;
        for i in 0..SEGMENTS {
            let a = scaled(self.points[i]);
            let b = scaled(self.points[i + 1]);
            let d = std::array::from_fn::<_, 3, _>(|axis| b[axis] - a[axis]);
            let length2: f64 = d.iter().map(|v| v * v).sum();
            let t = if length2 > 0.0 {
                ((0..3)
                    .map(|axis| (p[axis] - a[axis]) * d[axis])
                    .sum::<f64>()
                    / length2)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            };
            let distance = (0..3)
                .map(|axis| (p[axis] - a[axis] - t * d[axis]).powi(2))
                .sum::<f64>()
                .sqrt();
            let radius = self.radii[i] + t * (self.radii[i + 1] - self.radii[i]);
            // Scale the vertical feather with the metric so its world reach
            // remains bounded by the same box for flattened and tall passages.
            let feather = self.feather / self.flatten.max(1.0);
            let ramp = ((1.0 - (distance - radius).max(0.0) / feather) * (1.0 + 0.4 * knead))
                .clamp(0.0, 1.0);
            value = value.max(ramp * ramp * (3.0 - 2.0 * ramp));
        }
        value
    }
}
