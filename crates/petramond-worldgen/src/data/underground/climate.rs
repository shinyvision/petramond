use serde::{Deserialize, Serialize};

/// Temperature, humidity, continentality, erosion, variance and terrain depth.
pub type ClimatePoint = [f64; 6];
pub type ClimateBox = [[f64; 2]; 6];

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClimateRange {
    #[serde(default = "full")]
    temperature: [f64; 2],
    #[serde(default = "full")]
    humidity: [f64; 2],
    #[serde(default = "full")]
    continentality: [f64; 2],
    #[serde(default = "full")]
    erosion: [f64; 2],
    #[serde(default = "full")]
    variance: [f64; 2],
    depth: [f64; 2],
}

impl ClimateRange {
    #[inline]
    pub(super) fn axes(self) -> ClimateBox {
        [
            self.temperature,
            self.humidity,
            self.continentality,
            self.erosion,
            self.variance,
            self.depth,
        ]
    }

    pub(super) fn validate(self) -> Result<(), String> {
        for (name, [lo, hi]) in [
            "temperature",
            "humidity",
            "continentality",
            "erosion",
            "variance",
            "depth",
        ]
        .into_iter()
        .zip(self.axes())
        {
            if !lo.is_finite() || !hi.is_finite() || lo > hi || lo < -2.0 || hi > 2.0 {
                return Err(format!(
                    "'climate.{name}' must be an ordered finite range within [-2, 2]"
                ));
            }
        }
        Ok(())
    }

    #[inline]
    pub(super) fn distance_below(self, point: ClimatePoint, limit: f64) -> Option<f64> {
        let mut distance = 0.0;
        for ([lo, hi], v) in self.axes().into_iter().zip(point) {
            distance += (lo - v).max(v - hi).max(0.0).powi(2);
            if distance >= limit {
                return None;
            }
        }
        Some(distance)
    }

    /// Bounds on fitness throughout a climate box, including points between its corners.
    #[inline]
    pub(super) fn distance_bounds(self, bounds: ClimateBox) -> [f64; 2] {
        let mut out = [0.0; 2];
        for ([lo, hi], [a, b]) in self.axes().into_iter().zip(bounds) {
            out[0] += (lo - b).max(a - hi).max(0.0).powi(2);
            out[1] += (lo - a).max(b - hi).max(0.0).powi(2);
        }
        out
    }
}

fn full() -> [f64; 2] {
    [-1.0, 1.0]
}

pub(super) fn ordinary_fitness(depth: f64) -> f64 {
    depth.powi(2).min((depth - 1.0).powi(2))
}

pub(super) fn ordinary_upper([lo, hi]: [f64; 2]) -> f64 {
    let edges = ordinary_fitness(lo).max(ordinary_fitness(hi));
    if lo <= 0.5 && hi >= 0.5 {
        edges.max(0.25)
    } else {
        edges
    }
}
