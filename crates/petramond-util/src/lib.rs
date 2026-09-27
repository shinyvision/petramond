//! User data directory resolution, handing paths and relaunches to the
//! operating system, and optional shared test support.
//!
//! `test_time` and `test_dirs` are compiled only with the `test-support`
//! feature, which dependents enable from their `[dev-dependencies]`, so test
//! policy never ships in a release build.

pub mod paths;
pub mod process;
#[cfg(feature = "test-support")]
pub mod test_dirs;
#[cfg(feature = "test-support")]
pub mod test_time;
