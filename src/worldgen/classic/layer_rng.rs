//! The 64-bit layer generator — the deterministic random source for the layered
//! biome cascade. Separate from the 48-bit [`super::lcg`]: its own constants,
//! state, and seeding convention, used only by the biome layers.
//!
//! Seeding has three derived quantities, all wrapping `i64`:
//!   - **layer salt** — the layer's integer salt expanded by [`step`] three times
//!     ([`layer_salt`]).
//!   - **start salt** — the world seed mixed with the layer salt three times
//!     ([`start_salt`]); this is the value used to *advance* the cell seed between
//!     draws.
//!   - **start seed** — `step(start_salt, 0)` ([`start_seed`]); the base a cell's
//!     seed is derived from.
//!
//! A cell's seed is `start_seed + x` then `step` by `z, x, z` ([`cell_seed`]); a
//! bounded draw is `(seed >> 24) mod n` with a floor-mod sign-fix ([`first_int`]).
//!
//! The known-answer vectors in the tests were probed directly from the reference
//! layer library, so they lock this implementation to the reference bit-for-bit
//! (not merely to a paraphrased formula). Reference-equality of the whole stack is
//! then re-confirmed by the per-layer biome diff.

/// LCG multiplier for the layer step.
const MULT_64: i64 = 6364136223846793005;
/// LCG increment for the layer step.
const ADD_64: i64 = 1442695040888963407;

/// One salted advance: `s * (s * MULT_64 + ADD_64) + salt`, wrapping i64.
#[inline]
pub fn step(s: i64, salt: i64) -> i64 {
    s.wrapping_mul(s.wrapping_mul(MULT_64).wrapping_add(ADD_64))
        .wrapping_add(salt)
}

/// Expand a layer's integer salt into its layer salt (the salt stepped into
/// itself three times).
#[inline]
pub fn layer_salt(salt: i64) -> i64 {
    let mut ls = step(salt, salt);
    ls = step(ls, salt);
    ls = step(ls, salt);
    ls
}

/// Mix the world seed with a layer salt three times → the layer's start salt
/// (used to advance a cell seed between draws).
#[inline]
pub fn start_salt(world_seed: i64, layer_salt: i64) -> i64 {
    let mut st = step(world_seed, layer_salt);
    st = step(st, layer_salt);
    st = step(st, layer_salt);
    st
}

/// The layer's start seed: `step(start_salt, 0)`.
#[inline]
pub fn start_seed(world_seed: i64, layer_salt: i64) -> i64 {
    step(start_salt(world_seed, layer_salt), 0)
}

/// A cell's seed from the layer's start seed and the cell coordinate: `ss + x`
/// (plain add), then `step` by `z, x, z`.
#[inline]
pub fn cell_seed(start_seed: i64, x: i64, z: i64) -> i64 {
    let mut cs = start_seed.wrapping_add(x);
    cs = step(cs, z);
    cs = step(cs, x);
    cs = step(cs, z);
    cs
}

/// Non-advancing bounded draw from a raw cell seed: `(s >> 24) mod n` (arithmetic
/// shift, floor-mod sign-fix).
#[inline]
pub fn first_int(s: i64, n: i32) -> i32 {
    let mut r = ((s >> 24) % (n as i64)) as i32;
    if r < 0 {
        r += n;
    }
    r
}

/// `first_int(s, n) == 0`.
#[inline]
pub fn first_is_zero(s: i64, n: i32) -> bool {
    first_int(s, n) == 0
}

/// Convenience driver: holds a layer's start salt + start seed and a current cell
/// seed. `next_int` advances the cell seed with the **start salt** (not the bound).
#[derive(Clone, Debug)]
pub struct LayerRng {
    start_salt: i64,
    start_seed: i64,
    cell_seed: i64,
}

impl LayerRng {
    /// Build from a world seed and the layer's integer salt. Salt 0 is the
    /// zero-init special case (some branch layers): start salt and seed are 0.
    #[inline]
    pub fn new(world_seed: i64, salt: i64) -> Self {
        let ls = layer_salt(salt);
        let (start_salt, start_seed) = if ls == 0 {
            (0, 0)
        } else {
            let ss = start_salt(world_seed, ls);
            (ss, step(ss, 0))
        };
        Self {
            start_salt,
            start_seed,
            cell_seed: 0,
        }
    }

    #[inline]
    pub fn start_salt(&self) -> i64 {
        self.start_salt
    }

    #[inline]
    pub fn start_seed(&self) -> i64 {
        self.start_seed
    }

    #[inline]
    pub fn cell_seed(&self) -> i64 {
        self.cell_seed
    }

    /// Seed the current cell.
    #[inline]
    pub fn set_cell(&mut self, x: i64, z: i64) {
        self.cell_seed = cell_seed(self.start_seed, x, z);
    }

    /// Advancing bounded draw in `[0, n)`.
    #[inline]
    pub fn next_int(&mut self, n: i32) -> i32 {
        let i = first_int(self.cell_seed, n);
        self.cell_seed = step(self.cell_seed, self.start_salt);
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known-answer vectors probed directly from the reference layer library for
    // the continent layer (integer salt 1) at world seed 12345. They lock the
    // seeding chain bit-for-bit to the reference.

    #[test]
    fn step_known_answers() {
        assert_eq!(step(1, 0), 7_806_831_264_735_756_412);
        assert_eq!(step(1, 1), 7_806_831_264_735_756_413);
        assert_eq!(step(0, 0), 0);
        assert_eq!(step(0, 1), 1);
    }

    #[test]
    fn seeding_chain_matches_reference() {
        let ls = layer_salt(1);
        assert_eq!(ls, 3_107_951_898_966_440_229);
        let ss_salt = start_salt(12345, ls);
        assert_eq!(ss_salt, -2_202_151_823_110_491_623);
        assert_eq!(start_seed(12345, ls), 5_693_180_511_283_642_260);
    }

    #[test]
    fn cell_seed_and_draw_match_reference() {
        let seed = start_seed(12345, layer_salt(1));
        let cs = cell_seed(seed, 10, -7);
        assert_eq!(cs, -1_234_243_271_805_336_287);
        assert_eq!(first_int(cs, 10), 7);
    }

    #[test]
    fn advancing_draw_sequence_matches_reference() {
        let mut r = LayerRng::new(12345, 1);
        r.set_cell(10, -7);
        assert_eq!(r.next_int(6), 5);
        assert_eq!(r.next_int(4), 2);
    }

    #[test]
    fn next_int_is_always_in_range() {
        let mut r = LayerRng::new(0xABCD_1234, 2000);
        for x in -50..50 {
            r.set_cell(x, x * 7 - 3);
            for n in [2, 3, 4, 5, 6, 10, 13, 100, 256, 1024] {
                let v = r.next_int(n);
                assert!((0..n).contains(&v), "out of range n={n} v={v}");
            }
        }
    }

    #[test]
    fn determinism_same_inputs_same_outputs() {
        let mut a = LayerRng::new(777, 3);
        let mut b = LayerRng::new(777, 3);
        for (x, z) in [(0, 0), (13, -7), (-100, 250)] {
            a.set_cell(x, z);
            b.set_cell(x, z);
            for _ in 0..20 {
                assert_eq!(a.next_int(7), b.next_int(7));
            }
        }
    }
}
