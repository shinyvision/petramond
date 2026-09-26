//! User data directory resolution and optional shared test deadlines.
//!
//! `test_time` is compiled only with the `test-support` feature, which
//! dependents enable from their `[dev-dependencies]`, so test policy never
//! ships in a release build.

pub mod paths;
#[cfg(feature = "test-support")]
pub mod test_time;
