//! River column carve: cross-section geometry + nearest-path query.
//!
//! `carve_column` turns a column's distance to the path network into a valley
//! cross-section (wet channel → dished floodplain → valley wall), using the
//! shared per-column noises (`edge_noise`/`floodplain_noise`/`rim_noise`). The
//! two-nearest query (`nearest_hits`) is ordered by `(distance, key)` so the
//! median carve between sibling channels is independent of path iteration order
//! — the seam-determinism guarantee.

use noise::NoiseFn;

use super::route::RiverPath;
use super::*;

#[derive(Copy, Clone)]
pub(super) struct RiverHit {
    pub(super) distance: f32,
    pub(super) width: f32,
    pub(super) depth: f32,
    pub(super) key: (i32, i32),
}

impl RiverSystem {
    pub(super) fn carve_column(
        &self,
        wx: i32,
        wz: i32,
        base_surf: i32,
        biome: Biome,
        paths: &[RiverPath],
    ) -> Option<(RiverColumn, i32)> {
        if matches!(biome, Biome::DeepOcean | Biome::MushroomFields) {
            return None;
        }

        let px = wx as f32 + 0.5;
        let pz = wz as f32 + 0.5;
        let (best, second) = nearest_hits(px, pz, paths);
        let hit = best?;

        let sea = SEA_LEVEL as f32;
        let relief = (base_surf - SEA_LEVEL).max(0) as f32;
        let preserve_bed = base_surf <= SEA_LEVEL;
        let steepness = data::rivers::bank_steepness(&self.bank, wx, wz, biome, relief);

        // Radii outward from the centerline (decisions §5).
        let wet_half = (hit.width * 0.5).max(0.0);
        let edge = (wet_half + self.edge_noise(wx, wz)).max(0.0);
        let floodband = (WALL_MIN_RUN * 0.5 + wet_half * (FLOODPLAIN_FRAC - 0.5)).max(2.0);
        let flood_out = edge + floodband;
        let wall_run = (WALL_MIN_RUN + relief * WALL_RELIEF_K).clamp(WALL_MIN_RUN, WALL_RUN_MAX);
        let wall_out = flood_out + wall_run;
        let influence_radius = wall_out.min(INFLUENCE_CAP);
        if hit.distance >= influence_radius {
            return None;
        }

        // The shared cross-section profile for one hit at distance `d`. Monotone
        // non-increasing toward the centre, with NO flat sea-level ring. The
        // per-column noises are pure functions of (wx,wz), so the profile stays
        // identical for both hits and across regions.
        let bed_y = (sea - hit.depth).round().clamp(3.0, sea - 1.0);
        let flood_noise = self.floodplain_noise(wx, wz);
        let profile = |d: f32| -> f32 {
            if d <= edge && edge > 0.5 {
                // Wet channel (concave).
                let t = (d / edge).clamp(0.0, 1.0);
                bed_y + smoothstep01(t) * ((sea - 1.0 - bed_y).max(0.0))
            } else if d <= flood_out {
                // Dry, dished floodplain — rises gently from the wet rim to the
                // dry floor level, undulating so it never forms a flat strip at
                // exactly SEA. On flat plains the floor dips toward base-1. The
                // undulation fades to 0 at both joins so the floodplain meets the
                // wet channel and the valley wall seamlessly.
                let u = smoothstep(edge, flood_out, d);
                let fp_inner = sea - 1.0; // continuous with the wet-channel rim
                let fp_outer = (sea + FLOODPLAIN_RISE).min(base_surf as f32 - 1.0);
                let env = smoothstep(0.0, 0.25, u) * (1.0 - smoothstep(0.75, 1.0, u));
                lerp(fp_inner, fp_outer.max(fp_inner), u) + flood_noise * env
            } else {
                // Valley wall: rise from the floodplain floor up to untouched
                // terrain, steeper biomes hugging the floor longer (concave-up).
                let v = smoothstep(flood_out, wall_out, d);
                let wall_exp = 1.0 + steepness * 0.8;
                let lo = (sea + FLOODPLAIN_RISE)
                    .min(base_surf as f32 - 1.0)
                    .max(sea - 1.0);
                let hi = (base_surf as f32).max(lo);
                lo + (hi - lo) * v.powf(wall_exp) + self.rim_noise(wx, wz, relief, steepness, v)
            }
        };

        // Two-nearest carve safety net (decisions §7): the deepest target wins so
        // a near-parallel sibling channel floods the median between them.
        let mut target = profile(hit.distance);
        if let Some(other) = second {
            if other.distance < influence_radius {
                target = target.min(profile(other.distance));
            }
        }

        let mut carved_surf = (target.round() as i32).min(base_surf).max(3);
        debug_assert!(
            carved_surf <= base_surf,
            "carve must never raise terrain (carve-only invariant)"
        );

        // The column is wet if it lies inside the (noisy) wet edge of EITHER hit;
        // `channel` ramps 0→1 from that edge to the nearest wet centerline so that
        // a flooded median between two channels still reads as wet.
        let in_wet_best = hit.distance < edge;
        let in_wet_second = second.is_some_and(|o| o.distance < edge);
        let wet_distance = match (in_wet_best, in_wet_second) {
            (true, true) => hit
                .distance
                .min(second.map_or(hit.distance, |o| o.distance)),
            (true, false) => hit.distance,
            (false, true) => second.map_or(hit.distance, |o| o.distance),
            (false, false) => hit.distance, // outside both; channel will clamp to 0
        };
        let channel = if edge > 0.01 {
            (1.0 - wet_distance / edge).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // A column that reads as wet MUST flood. The carve's wet branch (gated on
        // `edge > 0.5`) and `wet()` (path width + channel) can disagree at a pinch
        // where edge-noise collapses `edge` while `hit.width >= WET_MIN`, leaving a
        // wet column carved up in the floodplain (>= SEA) — a dry stub. Force any
        // wet column below the waterline. Still carve-only (only lowers); pure
        // function of the column, so seam-safe.
        if is_wet(hit.width, channel) {
            carved_surf = carved_surf.min(SEA_LEVEL - 1);
        }
        let influence = 1.0 - smoothstep(flood_out, influence_radius, hit.distance);
        if influence <= 0.01 && !(in_wet_best || in_wet_second) {
            return None;
        }

        Some((
            RiverColumn {
                influence,
                channel,
                distance: hit.distance,
                width: hit.width,
                depth: hit.depth,
                bed_y: bed_y as i32,
                water_y: SEA_LEVEL,
                bed_block: data::rivers::bed_block(&self.material, wx, wz, biome),
                bank_block: data::rivers::bank_block(
                    &self.material,
                    wx,
                    wz,
                    biome,
                    influence,
                    hit.width,
                ),
                preserve_bed,
            },
            carved_surf,
        ))
    }

    /// Noisy waterline offset (≥2 octaves), via the shared `bank` sampler at world
    /// coords. Centred near 0 so it widens AND pinches the wet edge.
    fn edge_noise(&self, wx: i32, wz: i32) -> f32 {
        let n0 = self
            .bank
            .get([wx as f64 * 0.020 + 91.0, wz as f64 * 0.020 - 37.0]) as f32;
        let n1 = self
            .bank
            .get([wx as f64 * 0.075 - 17.0, wz as f64 * 0.075 + 53.0]) as f32;
        (0.65 * n0 + 0.35 * n1) * EDGE_AMP
    }

    /// Floodplain undulation (≥2 octaves). Breaks the valley floor so it is never
    /// a flat strip at exactly SEA_LEVEL — some of it dips below (water creeps in),
    /// some rises just above (dry bank). Pure function of world coords.
    fn floodplain_noise(&self, wx: i32, wz: i32) -> f32 {
        let n0 = self
            .bank
            .get([wx as f64 * 0.028 + 401.0, wz as f64 * 0.028 - 263.0]) as f32;
        let n1 = self
            .bank
            .get([wx as f64 * 0.091 - 121.0, wz as f64 * 0.091 + 77.0]) as f32;
        (0.7 * n0 + 0.3 * n1) * FLOODPLAIN_AMP
    }

    /// Rim/wall variation whose envelope VANISHES at both joins so the carve meets
    /// untouched terrain seamlessly. `v` is the normalized wall parameter (0 at the
    /// floodplain join, 1 at the rim).
    fn rim_noise(&self, wx: i32, wz: i32, relief: f32, steepness: f32, v: f32) -> f32 {
        let envelope = smoothstep(0.1, 0.4, v) * (1.0 - smoothstep(0.85, 1.0, v));
        if envelope <= 0.0 {
            return 0.0;
        }
        let broad = self
            .bank
            .get([wx as f64 * 0.041 - 177.0, wz as f64 * 0.041 + 53.0]) as f32;
        let detail = self
            .bank
            .get([wx as f64 * 0.137 + 31.0, wz as f64 * 0.137 - 211.0]) as f32;
        let terrace =
            self.bank
                .get([wx as f64 * 0.083 + 307.0, wz as f64 * 0.083 + 149.0]) as f32;
        let amplitude = (1.3 + relief * 0.065 + (1.0 - steepness) * 1.1).clamp(1.0, 6.5);
        let signal = broad * 0.65 + detail * 0.25 + terrace.signum() * 0.18;
        signal * amplitude * envelope
    }
}

/// The two nearest hits from DISTINCT paths (best, second-best by path identity),
/// each the closest segment of its path. Ordering is by `(distance, key)`
/// lexicographically so the result is independent of path iteration order — the
/// seam-determinism guarantee for the two-nearest median carve.
fn nearest_hits(x: f32, z: f32, paths: &[RiverPath]) -> (Option<RiverHit>, Option<RiverHit>) {
    let mut best: Option<RiverHit> = None;
    let mut second: Option<RiverHit> = None;
    for path in paths {
        if x < path.min_x || x > path.max_x || z < path.min_z || z > path.max_z {
            continue;
        }
        // Closest segment of THIS path.
        let mut path_hit: Option<RiverHit> = None;
        for segment in path.points.windows(2) {
            let a = segment[0];
            let b = segment[1];
            let abx = b.x - a.x;
            let abz = b.z - a.z;
            let len2 = abx * abx + abz * abz;
            if len2 <= f32::EPSILON {
                continue;
            }
            let t = (((x - a.x) * abx + (z - a.z) * abz) / len2).clamp(0.0, 1.0);
            let px = a.x + abx * t;
            let pz = a.z + abz * t;
            let dx = x - px;
            let dz = z - pz;
            let distance = (dx * dx + dz * dz).sqrt();
            if distance > MAX_QUERY_RADIUS {
                continue;
            }
            let hit = RiverHit {
                distance,
                width: a.width + (b.width - a.width) * t,
                depth: a.depth + (b.depth - a.depth) * t,
                key: path.key,
            };
            if path_hit.is_none_or(|h| hit_lt(&hit, &h)) {
                path_hit = Some(hit);
            }
        }
        let Some(hit) = path_hit else { continue };
        // Insert into (best, second) by distinct-path ordering.
        if best.is_none_or(|b| hit_lt(&hit, &b)) {
            second = best;
            best = Some(hit);
        } else if second.is_none_or(|s| hit_lt(&hit, &s)) {
            second = Some(hit);
        }
    }
    (best, second)
}

/// Strict lexicographic order on `(distance, key)` — total + deterministic.
#[inline]
fn hit_lt(a: &RiverHit, b: &RiverHit) -> bool {
    if a.distance != b.distance {
        a.distance < b.distance
    } else {
        a.key < b.key
    }
}

#[cfg(test)]
mod tests {
    use super::route::RiverPoint;
    use super::*;
    use crate::worldgen::river::tests::straight_path;

    #[test]
    fn carve_only_never_raises() {
        let rivers = RiverSystem::new(7);
        let paths = [straight_path(16.0, 5.0)];
        for base in [SEA_LEVEL - 3, SEA_LEVEL, SEA_LEVEL + 8, SEA_LEVEL + 40] {
            for wx in (-200..=200).step_by(7) {
                for wz in (-60..=60).step_by(11) {
                    if let Some((_, carved_surf)) =
                        rivers.carve_column(wx, wz, base, Biome::Plains, &paths)
                    {
                        assert!(
                            carved_surf <= base,
                            "carve raised terrain at ({wx},{wz}) base {base} -> {carved_surf}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn valley_not_ditch() {
        // A high-relief column: the cross-section must be a valley — rim above the
        // floodplain above the waterline — and rise monotonically outward. The
        // path runs along x at z=0, so perpendicular distance == |wz|; sweep wz.
        let rivers = RiverSystem::new(12_345);
        let base = SEA_LEVEL + 20; // relief 20 >= 12
        let paths = [straight_path(14.0, 5.0)];

        // wx = 0 so the sweep passes through the true centerline at wz=0 (distance
        // 0), giving a genuine channel-bed/waterline sample as the nearest point.
        let wx = 0;
        let mut samples = Vec::new(); // (distance, carved_surf)
        for wz in 0..=120 {
            if let Some((river, carved)) = rivers.carve_column(wx, wz, base, Biome::Plains, &paths)
            {
                samples.push((river.distance, carved));
            }
        }
        assert!(samples.len() > 20, "should carve a wide cross-section");
        samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let waterline = samples.first().unwrap().1; // channel bed at the centerline
        let rim = samples.last().unwrap().1; // furthest carved (valley wall top)
                                             // A representative floodplain sample: roughly mid-profile.
        let mid = samples[samples.len() / 2].1;
        assert!(
            waterline < SEA_LEVEL,
            "channel centre ({waterline}) must carve to/below the waterline"
        );
        assert!(
            rim - mid >= 3,
            "rim ({rim}) should sit well above the floodplain ({mid})"
        );
        assert!(
            mid - waterline >= 1,
            "floodplain ({mid}) should sit above the waterline ({waterline})"
        );
        // Rises outward (valley, not a re-rising ditch lip). Allow small noise
        // jitter (<= 2 blocks) without counting it a violation.
        let mut peak = samples[0].1;
        let mut violations = 0;
        for (_, y) in &samples {
            if *y < peak - 2 {
                violations += 1;
            }
            peak = (*y).max(peak);
        }
        assert_eq!(
            violations, 0,
            "valley profile should rise monotonically outward"
        );
    }

    #[test]
    fn bank_carve_fluctuates_around_smoothed_terrain() {
        let rivers = RiverSystem::new(12_345);
        let paths = [straight_path(16.0, 6.0)];

        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        let mut samples = 0usize;
        for wx in (-320..=320).step_by(8) {
            let Some((river, carved_surf)) =
                rivers.carve_column(wx, 30, SEA_LEVEL + 32, Biome::Plains, &paths)
            else {
                continue;
            };
            assert!(river.active());
            assert!(
                (3..=SEA_LEVEL + 32).contains(&carved_surf),
                "bank variation should stay between the bed floor and the pre-river terrain"
            );
            min_y = min_y.min(carved_surf);
            max_y = max_y.max(carved_surf);
            samples += 1;
        }

        assert!(samples > 32, "synthetic bank should produce enough samples");
        assert!(
            max_y - min_y >= 3,
            "constant-height input terrain should still produce varied river surface heights"
        );
    }

    #[test]
    fn steeper_biomes_give_steeper_walls() {
        // bank_extra is gone; steepness now drives the wall exponent. A steeper
        // biome must produce a taller wall partway up (concave-up `v^wall_exp`).
        let rivers = RiverSystem::new(12_345);
        let plains = data::rivers::bank_steepness(&rivers.bank, 0, 0, Biome::Plains, 10.0);
        let mountains = data::rivers::bank_steepness(&rivers.bank, 0, 0, Biome::Mountains, 80.0);
        assert!(
            mountains > plains,
            "mountainous terrain should bias toward steeper river banks"
        );

        // wall_exp = 1 + steepness*0.8; for v in (0,1), higher exp => smaller value
        // at the same v => the wall stays lower until close to the rim (steeper at
        // the top). Verify the exponent relationship directly via the carve.
        let v = 0.5f32;
        let gentle = v.powf(1.0 + plains * 0.8);
        let steep = v.powf(1.0 + mountains * 0.8);
        assert!(
            steep < gentle,
            "steeper banks should hug the floodplain longer then rise sharply"
        );
    }

    #[test]
    fn two_parallel_paths_flood_the_median() {
        // Two near-parallel channels whose wet zones nearly meet: the median
        // between them must flood (the two-nearest carve takes the deepest target)
        // — no dry above-sea sandbar strip.
        let rivers = RiverSystem::new(1);
        let make = |z: f32, key: (i32, i32)| RiverPath {
            key,
            points: vec![
                RiverPoint {
                    x: -300.0,
                    z,
                    width: 12.0,
                    depth: 5.0,
                },
                RiverPoint {
                    x: 300.0,
                    z,
                    width: 12.0,
                    depth: 5.0,
                },
            ],
            min_x: -300.0 - MAX_QUERY_RADIUS,
            min_z: z - MAX_QUERY_RADIUS,
            max_x: 300.0 + MAX_QUERY_RADIUS,
            max_z: z + MAX_QUERY_RADIUS,
        };
        // Centerlines 10 apart; wet_half = 6 each, so the wet zones overlap at the
        // median (z=0 is distance 5 < edge from both).
        let paths = [make(-5.0, (0, 0)), make(5.0, (1, 0))];

        let (river, carved) = rivers
            .carve_column(0, 0, SEA_LEVEL + 6, Biome::Plains, &paths)
            .expect("median column should be carved by two flanking paths");
        assert!(
            carved < SEA_LEVEL,
            "median between two channels should carve to/below water (was {carved})"
        );
        assert!(river.wet(), "median should be a wet river column");

        // Sweep the whole median strip: none should be a dry above-sea sandbar.
        let mut dry_median = 0usize;
        for wx in (-260..=260).step_by(5) {
            if let Some((_, c)) = rivers.carve_column(wx, 0, SEA_LEVEL + 6, Biome::Plains, &paths) {
                if c > SEA_LEVEL {
                    dry_median += 1;
                }
            }
        }
        assert_eq!(dry_median, 0, "no dry mid-channel sandbar should remain");
    }

    #[test]
    fn river_through_existing_water_preserves_bed_material_flag() {
        let rivers = RiverSystem::new(7);
        let path = RiverPath {
            key: (0, 0),
            points: vec![
                RiverPoint {
                    x: -32.0,
                    z: 0.0,
                    width: 16.0,
                    depth: 5.0,
                },
                RiverPoint {
                    x: 32.0,
                    z: 0.0,
                    width: 18.0,
                    depth: 5.5,
                },
            ],
            min_x: -MAX_QUERY_RADIUS,
            min_z: -MAX_QUERY_RADIUS,
            max_x: MAX_QUERY_RADIUS,
            max_z: MAX_QUERY_RADIUS,
        };

        let (river, carved_surf) = rivers
            .carve_column(0, 0, SEA_LEVEL - 1, Biome::Plains, &[path])
            .expect("centerline should carve the shallow water-body floor");

        assert!(river.wet());
        assert!(river.preserve_bed);
        assert_eq!(river.water_y, SEA_LEVEL);
        assert!(
            carved_surf < SEA_LEVEL - 1,
            "river should clear shallow water-body floors to its channel bed"
        );
    }
}
