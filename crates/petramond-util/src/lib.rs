//! Persistence and platform plumbing with no game-domain knowledge: crash-safe
//! file replacement, the little-endian byte codec the save and region formats
//! share, and the user data directories.
//!
//! `test_time` is compiled only with the `test-support` feature, which
//! dependents enable from their `[dev-dependencies]`, so test policy never
//! ships in a release build.

pub mod atomic_file;
pub mod bytecodec;
pub mod paths;
#[cfg(feature = "test-support")]
pub mod test_time;
