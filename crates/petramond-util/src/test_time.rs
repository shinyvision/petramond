//! Shared wall-clock budgets for tests.
//!
//! Endeavor: every default-suite test should pass or fail in ≤0.5 s.
//! Hard rule: any test that takes >10 s must be optimized or removed.

use std::time::Duration;

/// Prefer this as a give-up bound when the condition is already wrong.
/// New pump/drain loops should start here and only widen to
/// [`TEST_HARD_DEADLINE`] when a threaded/TCP edge needs it.
#[allow(dead_code)] // preferred bound for new tests; many call sites still use hard
pub const TEST_ENDEAVOR_DEADLINE: Duration = Duration::from_millis(500);

/// Hard cap for any test wait / give-up deadline in the default suite.
pub const TEST_HARD_DEADLINE: Duration = Duration::from_secs(10);
