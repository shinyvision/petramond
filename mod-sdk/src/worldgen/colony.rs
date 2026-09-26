//! Seam-safe COLONY density fields: the "things grow in patches" rule every
//! worldgen feature that scatters by density needs, as one pure function of
//! `(seed, column)` so every section that can see a column derives the same
//! answer for it.

use super::GenRng;

/// A per-mille density field made of colonies: each lattice cell of the
/// column grid may seed one colony at a jittered centre inside it, owning a
/// disc whose density falls LINEARLY in distance from `core` at the centre to
/// `rim` at the edge. Columns in no colony keep `stray`, and where colonies
/// overlap the densest one owns the column — two patches meeting read as two
/// patches, not a bloom.
///
/// The field is positional through [`GenRng::positional`] on the lattice
/// cell, so it never depends on which section asks, in what order, or on
/// anything but the seed and the column.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ColonyField {
    /// The feature's own positional salt.
    pub salt: u64,
    /// Lattice cell edge, in blocks.
    pub lattice: i32,
    /// One lattice cell in `one_in` seeds a colony. `1` seeds every cell and
    /// draws no rarity roll at all.
    pub one_in: i32,
    /// Colony radius in blocks, rolled per colony (inclusive).
    pub radius: (i32, i32),
    /// Per-mille density at a colony's centre.
    pub core: i32,
    /// Per-mille density at a colony's rim.
    pub rim: i32,
    /// Per-mille density outside every colony.
    pub stray: i32,
}

/// The colony that owns a column.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Colony<T> {
    /// Whole-block distance from the colony's centre to the column.
    pub distance: i32,
    /// The colony's rolled radius.
    pub radius: i32,
    /// What the feature rolled for this colony (its kind, its colour...).
    pub traits: T,
}

impl ColonyField {
    /// The density at column `(wx, wz)` and the colony that owns it (`None`
    /// = a stray column). `traits` draws the owning colony's own properties
    /// from its stream, right after the centre and radius rolls, so a
    /// colony's traits are the same from every column it covers.
    pub fn densest<T>(
        &self,
        seed: u32,
        wx: i32,
        wz: i32,
        mut traits: impl FnMut(&mut GenRng) -> T,
    ) -> (i32, Option<Colony<T>>) {
        let mut best = self.stray;
        let mut owner = None;
        let cell = |v: i32| v.div_euclid(self.lattice);
        // A colony reaches at most `radius.1` blocks, so only the lattice
        // cells within that of the column can own it.
        let reach = self.radius.1;
        for lz in cell(wz - reach)..=cell(wz + reach) {
            for lx in cell(wx - reach)..=cell(wx + reach) {
                let mut rng = GenRng::positional(seed, self.salt, lx, 0, lz);
                if self.one_in > 1 && rng.next_i32(0, self.one_in - 1) != 0 {
                    continue;
                }
                let cx = lx * self.lattice + rng.next_i32(0, self.lattice - 1);
                let cz = lz * self.lattice + rng.next_i32(0, self.lattice - 1);
                let r = rng.next_i32(self.radius.0, self.radius.1);
                let (dx, dz) = (wx - cx, wz - cz);
                let d2 = dx * dx + dz * dz;
                if d2 > r * r {
                    continue;
                }
                // Linear in DISTANCE, not in distance squared: squared falls
                // off far too slowly near the centre and gives a flat disc.
                let d = isqrt(d2);
                let dens = self.core + (self.rim - self.core) * d / r.max(1);
                if dens > best {
                    best = dens;
                    owner = Some(Colony {
                        distance: d,
                        radius: r,
                        traits: traits(&mut rng),
                    });
                }
            }
        }
        (best, owner)
    }
}

/// Integer square root (floor). Shape decisions in worldgen stay in integer
/// arithmetic: mixing in `f64::sqrt` invites a rounding difference between
/// two derivations of one cell.
pub fn isqrt(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// The cubic ease `3t² − 2t³` over `t` in `[0, 1]`: the blend weight for
/// interpolating a value field between lattice corners without visible
/// creases at the cell edges.
pub fn smoothstep01(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothstep01_eases_from_zero_to_one() {
        assert_eq!((smoothstep01(0.0), smoothstep01(0.5), smoothstep01(1.0)), (0.0, 0.5, 1.0));
        assert!(smoothstep01(0.1) < 0.1 && smoothstep01(0.9) > 0.9, "flat at both ends");
    }

    const FIELD: ColonyField = ColonyField {
        salt: 0x00C0_10E7,
        lattice: 16,
        one_in: 1,
        radius: (4, 9),
        core: 500,
        rim: 100,
        stray: 5,
    };

    #[test]
    fn isqrt_is_the_floor_root() {
        for n in 0..10_000 {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "isqrt({n}) = {r}");
        }
        assert_eq!(isqrt(-4), 0);
    }

    #[test]
    fn every_column_agrees_with_its_owner_and_the_falloff() {
        let (mut owned, mut stray) = (0, 0);
        for wz in -40..40 {
            for wx in -40..40 {
                let (dens, owner) = FIELD.densest(7, wx, wz, |_| ());
                match owner {
                    Some(c) => {
                        owned += 1;
                        assert!(c.distance <= c.radius);
                        assert_eq!(dens, 500 - 400 * c.distance / c.radius);
                    }
                    None => {
                        stray += 1;
                        assert_eq!(dens, FIELD.stray);
                    }
                }
                assert_eq!(FIELD.densest(7, wx, wz, |_| ()).0, dens, "not pure");
            }
        }
        assert!(owned > 0 && stray > 0, "owned {owned}, stray {stray}");
    }

    #[test]
    fn traits_are_the_colonys_own_draws_after_its_rolls() {
        // Replay lattice cell (0, 0)'s stream by hand and ask about its own
        // centre column: its colony owns it at full density.
        for seed in 0..64 {
            let mut rng = GenRng::positional(seed, FIELD.salt, 0, 0, 0);
            let cx = rng.next_i32(0, FIELD.lattice - 1);
            let cz = rng.next_i32(0, FIELD.lattice - 1);
            let radius = rng.next_i32(FIELD.radius.0, FIELD.radius.1);
            let expected = rng.next_u64();
            let (dens, owner) = FIELD.densest(seed, cx, cz, GenRng::next_u64);
            let owner = owner.expect("a centre column is owned");
            assert_eq!(dens, FIELD.core);
            assert_eq!(owner.distance, 0);
            // Another centre on the very same column would win only if met
            // first; skip that (rare) seed rather than guess.
            if owner.radius == radius && owner.traits == expected {
                return;
            }
        }
        panic!("no seed handed the colony its own draws");
    }

    #[test]
    fn a_rare_field_rolls_before_placing() {
        let rare = ColonyField { one_in: 1_000_000, ..FIELD };
        let owned = (-40..40)
            .flat_map(|wz| (-40..40).map(move |wx| (wx, wz)))
            .filter(|&(wx, wz)| rare.densest(3, wx, wz, |_| ()).1.is_some())
            .count();
        assert_eq!(owned, 0, "a one-in-a-million field seeded a colony here");
    }
}
