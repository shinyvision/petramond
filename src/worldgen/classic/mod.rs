//! **Classic generator** — a deterministic, layered world generator built to be a
//! stable, exactly-reproducible foundation for llamacraft's worldgen.
//!
//! It is intentionally a faithful implementation of a well-understood, proven
//! generation pipeline so the output is correct and stable from day one; we own it
//! and intend to evolve it from here, not freeze it as a copy.
//!
//! Pipeline (each stage a pure function of the seed + coordinate):
//!   - [`lcg`] — the 48-bit LCG random source every stage draws from.
//!   - layered land-biome generation (island → zoom → refine → smooth cascade).
//!   - octave-noise base terrain shaped by those land biomes.
//!   - the classic river overlay remains available for parity/reference tests,
//!     while active generation carves rivers later via `worldgen::river`.
//!
//! **Verification contract.** Correctness is *measured*, not asserted: the LCG is
//! pinned by known-answer vectors (see [`lcg`]); each higher stage is diffed
//! against independent reference output over a large, multi-seed sample so "this
//! is a faithful, stable basis" is a tested claim, not a hope.

pub mod biome;
pub mod layer_rng;
pub mod lcg;
pub mod noise;
pub mod terrain;
pub mod world;
