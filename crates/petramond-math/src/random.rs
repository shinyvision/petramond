//! Reproducible pseudo-random streams for data and spatial algorithms.

/// A 48-bit linear congruential stream with unbiased bounded sampling.
pub struct Lcg48(u64);

impl Lcg48 {
    pub fn new(seed: u64) -> Self {
        Self((seed ^ 0x5deece66d) & ((1 << 48) - 1))
    }

    fn bits(&mut self, n: u32) -> u32 {
        self.0 = self.0.wrapping_mul(0x5deece66d).wrapping_add(11) & ((1 << 48) - 1);
        (self.0 >> (48 - n)) as u32
    }

    pub fn unit(&mut self) -> f64 {
        ((u64::from(self.bits(26)) << 27) + u64::from(self.bits(27))) as f64 / (1u64 << 53) as f64
    }

    /// An integer in `0..bound`; rejects biased high residues.
    pub fn bounded(&mut self, bound: u32) -> u32 {
        assert!(bound > 0 && bound <= i32::MAX as u32);
        if bound.is_power_of_two() {
            return ((u64::from(bound) * u64::from(self.bits(31))) >> 31) as u32;
        }
        loop {
            let bits = self.bits(31);
            let value = bits % bound;
            if bits - value + (bound - 1) < 1 << 31 {
                return value;
            }
        }
    }
}

#[cfg(test)]
mod tests;
