//! Deterministic river path routing.
//!
//! Builds the explicit path objects that columns later query: source-cell trace
//! (`compute_path_from_cell`), the terrain-aware turn-limited heading
//! (`flow_dir`/`turn_limited`), per-step channel width/depth, the terminal pond,
//! and the region path-enumeration (`paths_for_bounds`/`path_from_cell`). Every
//! result is a pure function of `(seed, cell)`; see the module-level contract.

use noise::NoiseFn;

use super::*;

/// A path's identity is its source cell; `points` are the traced centerline
/// samples with their per-point width/depth. The bbox is padded by the query
/// radius for cheap region culling.
#[derive(Clone)]
pub(super) struct RiverPath {
    /// Stable identity = source cell. Distinct paths have distinct keys; used for
    /// the deterministic two-nearest tiebreak (order-independent).
    pub(super) key: (i32, i32),
    pub(super) points: Vec<RiverPoint>,
    pub(super) min_x: f32,
    pub(super) min_z: f32,
    pub(super) max_x: f32,
    pub(super) max_z: f32,
}

impl RiverPath {
    pub(super) fn intersects(&self, x0: f32, z0: f32, x1: f32, z1: f32) -> bool {
        self.max_x >= x0 && self.min_x <= x1 && self.max_z >= z0 && self.min_z <= z1
    }
}

#[derive(Copy, Clone)]
pub(super) struct RiverPoint {
    pub(super) x: f32,
    pub(super) z: f32,
    pub(super) width: f32,
    pub(super) depth: f32,
}

impl RiverSystem {
    pub(super) fn paths_for_bounds(&self, x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<RiverPath> {
        let cx0 = (x0 - PATH_REACH).div_euclid(CELL_BLOCKS);
        let cz0 = (z0 - PATH_REACH).div_euclid(CELL_BLOCKS);
        let cx1 = (x1 + PATH_REACH).div_euclid(CELL_BLOCKS);
        let cz1 = (z1 + PATH_REACH).div_euclid(CELL_BLOCKS);
        let mut paths = Vec::new();
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                if let Some(path) = self.path_from_cell(cx, cz) {
                    if path.intersects(x0 as f32, z0 as f32, x1 as f32, z1 as f32) {
                        paths.push(path);
                    }
                }
            }
        }
        paths
    }

    /// Memoized wrapper over [`Self::compute_path_from_cell`]. The cache is a pure
    /// memo of a pure function, so results are identical to computing every time.
    pub(super) fn path_from_cell(&self, cx: i32, cz: i32) -> Option<RiverPath> {
        if let Some(cached) = self.path_cache.read().unwrap().get(&(cx, cz)) {
            return cached.clone();
        }
        let path = self.compute_path_from_cell(cx, cz);
        self.path_cache
            .write()
            .unwrap()
            .insert((cx, cz), path.clone());
        path
    }

    fn compute_path_from_cell(&self, cx: i32, cz: i32) -> Option<RiverPath> {
        let gate = self.source_gate(cx, cz)?;
        if self.source_suppressed(cx, cz, gate) {
            return None;
        }

        let mut rng = FeatureRng::positional(self.seed, SOURCE_SALT, cx, 0, cz);
        // Re-consume the 3 gate draws (roll, jitter x, jitter z) so subsequent
        // draws (phase, dir) stay on the same stream the gate established.
        let _roll = rng.next_f32();
        let _jit_x = rng.next_f32();
        let _jit_z = rng.next_f32();
        let phase0 = rng.next_f32() * std::f32::consts::TAU;

        let mut x = gate.x;
        let mut z = gate.z;
        let mut dir = unit_from_angle(rng.next_f32() * std::f32::consts::TAU);
        let mut points = Vec::with_capacity(PATH_STEPS + 1);
        let mut min_x = x;
        let mut min_z = z;
        let mut max_x = x;
        let mut max_z = z;
        let mut reached_ocean = false;
        let mut ocean_extra = 0usize;

        for step in 0..=PATH_STEPS {
            let downstream = step as f32 / PATH_STEPS as f32;
            let w = self.channel_width(x, z, downstream);
            let depth = self.channel_depth(x, z, w, downstream);
            points.push(RiverPoint {
                x,
                z,
                width: w,
                depth,
            });
            min_x = min_x.min(x);
            min_z = min_z.min(z);
            max_x = max_x.max(x);
            max_z = max_z.max(z);

            // Ocean termination: on first ocean hit, extend a few steps into the
            // sea then stop with the mouth at full width (decisions §3).
            if !reached_ocean && self.is_ocean_at(x.round() as i32, z.round() as i32) {
                reached_ocean = true;
            }
            if reached_ocean {
                ocean_extra += 1;
                if ocean_extra > OCEAN_OVERSHOOT_STEPS {
                    break;
                }
            }

            let s = step as f32 * STEP_BLOCKS;
            dir = self.flow_dir(x, z, dir, s, w, phase0);
            x += dir.0 * STEP_BLOCKS;
            z += dir.1 * STEP_BLOCKS;
        }

        // Cap hit without reaching ocean → terminal pond so it never ends in a
        // dry wide stub (decisions §3).
        if !reached_ocean {
            self.apply_terminal_pond(&mut points);
            for p in points.iter().rev().take(3) {
                min_x = min_x.min(p.x);
                min_z = min_z.min(p.z);
                max_x = max_x.max(p.x);
                max_z = max_z.max(p.z);
            }
        }

        Some(RiverPath {
            key: (cx, cz),
            points,
            min_x: min_x - MAX_QUERY_RADIUS,
            min_z: min_z - MAX_QUERY_RADIUS,
            max_x: max_x + MAX_QUERY_RADIUS,
            max_z: max_z + MAX_QUERY_RADIUS,
        })
    }

    /// Flatten the meander over the last few points and seat a small basin so a
    /// non-ocean-reaching river ends in water.
    fn apply_terminal_pond(&self, points: &mut [RiverPoint]) {
        let n = points.len();
        if n < 2 {
            return;
        }
        let last = points[n - 1];
        let pond_w = (2.0 * WET_MIN).max(0.8 * last.width).min(WET_MAX);
        let pond_d = (BED_MIN_DEPTH + 2.0).min(BED_MAX_DEPTH);
        let span = n.min(3);
        for k in 0..span {
            let idx = n - 1 - k;
            let blend = 1.0 - k as f32 / span as f32; // 1 at terminus, fades inward
            let p = &mut points[idx];
            // Floor at WET_MIN (not a blend-scaled floor) so EVERY pond point is
            // wet — otherwise the upstream-most pond point could drop below WET_MIN
            // and leave a 1-point dry gap between the pond and the river.
            p.width = lerp(p.width, pond_w, blend).max(WET_MIN);
            p.depth = lerp(p.depth, pond_d, blend);
        }
    }

    fn channel_width(&self, x: f32, z: f32, downstream: f32) -> f32 {
        let grow = WET_HEADWATER + smoothstep01(downstream) * (WET_MAX - WET_HEADWATER);
        let fluct = (0.65 * self.width.get([x as f64 * 0.004, z as f64 * 0.004]) as f32
            + 0.35 * self.width.get([x as f64 * 0.013, z as f64 * 0.013]) as f32)
            * 4.0;
        let head = smoothstep(0.0, 0.10, downstream); // source fade only
        ((grow + fluct).max(0.0) * head).clamp(0.0, WET_MAX)
    }

    fn channel_depth(&self, x: f32, z: f32, width: f32, downstream: f32) -> f32 {
        let wob = self.depth.get([x as f64 * 0.007, z as f64 * 0.007]) as f32;
        let base = BED_MIN_DEPTH
            + (width / WET_MAX) * (BED_MAX_DEPTH - BED_MIN_DEPTH)
            + downstream * 1.5
            + wob * 1.0;
        (base * smoothstep(0.0, 0.10, downstream)).clamp(0.0, BED_MAX_DEPTH)
    }

    /// Terrain-aware flow direction: a seaward target (downhill gradient + global
    /// tilt) perturbed by a bounded arc-length meander, then the turn from the
    /// previous heading is CLAMPED to `MAX_TURN`. Clamping is the key fix over a
    /// raw weighted sum, whose net could reverse/curl and tie the path in knots —
    /// a clamped turn can only ever bend the river gently forward.
    fn flow_dir(
        &self,
        x: f32,
        z: f32,
        prev: (f32, f32),
        s: f32,
        width: f32,
        phase0: f32,
    ) -> (f32, f32) {
        // Wide central difference of the coarse elevation (downhill = -gradient).
        // ~0 inside one biome; nonzero and seaward across a biome boundary.
        let gx = self.coarse_elevation((x + GRAD_OFFS).round() as i32, z.round() as i32)
            - self.coarse_elevation((x - GRAD_OFFS).round() as i32, z.round() as i32);
        let gz = self.coarse_elevation(x.round() as i32, (z + GRAD_OFFS).round() as i32)
            - self.coarse_elevation(x.round() as i32, (z - GRAD_OFFS).round() as i32);
        let downhill = normalize((-gx, -gz)).unwrap_or((0.0, 0.0));

        // Lateral meander: a bounded perpendicular swing about the forward target,
        // arc-length phased so its wavelength scales with width. Because it is a
        // perpendicular COMPONENT (not a free vector) it can never point backward.
        let l = (MEANDER_BASE + MEANDER_K * width).max(1.0);
        let m = (phase0 + std::f32::consts::TAU * s / l).sin();
        let perp = (-prev.1, prev.0);

        // Seaward target heading.
        let desired = normalize((
            downhill.0 * W_DOWN + self.tilt_x * W_TILT + perp.0 * m * W_MEANDER,
            downhill.1 * W_DOWN + self.tilt_z * W_TILT + perp.1 * m * W_MEANDER,
        ))
        .unwrap_or(prev);

        // Clamp the turn from `prev` to `desired` to ±MAX_TURN — guarantees no knots.
        turn_limited(prev, desired, self.cos_max_turn, self.sin_max_turn)
    }
}

/// Rotate unit vector `prev` toward unit vector `desired` by at most `MAX_TURN`,
/// whose cos/sin are passed in. Pure 2-D rotation by a constant angle — no
/// per-step `atan2`, keeping the trig surface (and determinism) minimal.
fn turn_limited(prev: (f32, f32), desired: (f32, f32), cos_max: f32, sin_max: f32) -> (f32, f32) {
    let dot = (prev.0 * desired.0 + prev.1 * desired.1).clamp(-1.0, 1.0);
    if dot >= cos_max {
        return desired; // already within the per-step turn limit
    }
    // Rotate `prev` by ±MAX_TURN, sign chosen to turn toward `desired`.
    let s = if prev.0 * desired.1 - prev.1 * desired.0 >= 0.0 {
        sin_max
    } else {
        -sin_max
    };
    normalize((prev.0 * cos_max - prev.1 * s, prev.0 * s + prev.1 * cos_max)).unwrap_or(prev)
}

#[inline]
fn unit_from_angle(angle: f32) -> (f32, f32) {
    (angle.cos(), angle.sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_limited_clamps_and_stays_unit() {
        // The headline no-knot guarantee: the heading can never change by more than
        // MAX_TURN in a step, and stays a unit vector.
        let (c, s) = (MAX_TURN.cos(), MAX_TURN.sin());
        let prev = (1.0f32, 0.0);
        for &desired in &[(-1.0f32, 0.0f32), (0.0, 1.0), (0.0, -1.0)] {
            let out = turn_limited(prev, desired, c, s);
            let len = (out.0 * out.0 + out.1 * out.1).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "heading must stay unit (len {len})");
            let ang = (prev.0 * out.0 + prev.1 * out.1).clamp(-1.0, 1.0).acos();
            assert!(
                ang <= MAX_TURN + 1e-3,
                "turn {ang} exceeded MAX_TURN {MAX_TURN}"
            );
        }
        // A small desired turn (within the limit) is applied directly.
        let small = normalize((1.0, 0.1)).unwrap();
        let out = turn_limited(prev, small, c, s);
        assert!((out.0 - small.0).abs() < 1e-6 && (out.1 - small.1).abs() < 1e-6);
    }

    #[test]
    fn generated_paths_meander() {
        // At least one generated river should visibly wander (sinuosity = arc length
        // / straight-line distance well above 1), proving the meander actually bends
        // the course rather than running straight.
        let rivers = RiverSystem::new(12_345);
        let mut best_sinuosity = 1.0f32;
        for cz in -8..=8 {
            for cx in -8..=8 {
                let Some(path) = rivers.path_from_cell(cx, cz) else {
                    continue;
                };
                if path.points.len() < 10 {
                    continue;
                }
                let arc: f32 = path
                    .points
                    .windows(2)
                    .map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].z - w[0].z).powi(2)).sqrt())
                    .sum();
                let f = path.points.first().unwrap();
                let l = path.points.last().unwrap();
                let straight = ((l.x - f.x).powi(2) + (l.z - f.z).powi(2)).sqrt();
                if straight < 80.0 {
                    continue; // skip short/pond-terminated degenerate paths
                }
                best_sinuosity = best_sinuosity.max(arc / straight);
            }
        }
        assert!(
            best_sinuosity >= 1.1,
            "at least one river should visibly meander (best sinuosity {best_sinuosity})"
        );
    }

    #[test]
    fn wet_width_within_band_and_fluctuates() {
        // Build real paths and confirm wet widths land in band and vary per path.
        let rivers = RiverSystem::new(12_345);
        let mut found = 0usize;
        'cells: for cz in -6..=6 {
            for cx in -6..=6 {
                let Some(path) = rivers.path_from_cell(cx, cz) else {
                    continue;
                };
                let widths: Vec<f32> = path
                    .points
                    .iter()
                    .map(|p| p.width)
                    .filter(|&w| w >= WET_MIN)
                    .collect();
                if widths.len() < 6 {
                    continue;
                }
                let max = widths.iter().cloned().fold(f32::MIN, f32::max);
                let min = widths.iter().cloned().fold(f32::MAX, f32::min);
                assert!(max <= WET_MAX + 0.001, "wet width {max} exceeds band");
                assert!(min >= 4.0, "wet width {min} below band floor");
                assert!(
                    max - min >= 3.0,
                    "wet width should fluctuate along a path (spread {:.2})",
                    max - min
                );
                found += 1;
                if found >= 3 {
                    break 'cells;
                }
            }
        }
        assert!(found >= 3, "should find several wet paths to measure");
    }

    #[test]
    fn headwater_fades_but_mouth_stays_wide() {
        let rivers = RiverSystem::new(12_345);
        let mut path = None;
        'search: for cz in -10..=10 {
            for cx in -10..=10 {
                if let Some(found) = rivers.path_from_cell(cx, cz) {
                    // Want a sufficiently long, wide river to assert on.
                    if found.points.iter().any(|p| p.width > 12.0) {
                        path = Some(found);
                        break 'search;
                    }
                }
            }
        }
        let path = path.expect("search area should contain a wide generated river path");

        let first = path.points.first().unwrap();
        let last = path.points.last().unwrap();
        // Widest point in the downstream half (robust to per-point pinch noise).
        let half = path.points.len() / 2;
        let mid_max = path.points[half..]
            .iter()
            .map(|p| p.width)
            .fold(0.0f32, f32::max);
        assert!(
            first.width < WET_MIN,
            "source end should start as a sub-WET_MIN trickle (was {})",
            first.width
        );
        assert!(
            mid_max > 12.0,
            "downstream half of a generated river should be visibly wide (was {mid_max})"
        );
        assert!(
            last.width >= WET_MIN,
            "mouth/terminus must stay wide, not taper to nothing (was {})",
            last.width
        );
    }
}
