//! Re-running one `#[ignore]`d test in a fresh copy of the current test
//! binary.
//!
//! Tests that need process-global state set before first touch (a fixture pack
//! in `PETRAMOND_MODS` read by the registries, a watchdog alone on the machine)
//! spawn `<test binary> <path> --exact --ignored` and check the result. libtest
//! exits 0 when the filter matches nothing, so a status check alone turns the
//! whole test into a silent no-op the moment the inner test is renamed or its
//! module moves. [`ChildRun::assert_passed`] therefore also requires that
//! exactly one test ran and passed.

use std::ffi::OsStr;
use std::process::{Command, Output};

/// The finished child process of [`run_ignored`].
#[must_use = "call `assert_passed` — a child that ran no test still exits 0"]
pub struct ChildRun {
    test_path: String,
    output: Output,
}

/// Re-spawn the current test binary on `test_path` (the full module path of
/// an `#[ignore]`d test, e.g. `mob::load::tests::dynamic_pack_mob_inner`) with
/// `envs` added to the environment, and wait for it. Clean any fixture up
/// between this and [`ChildRun::assert_passed`] so a failure does not leak it.
pub fn run_ignored<I, K, V>(test_path: &str, envs: I) -> ChildRun
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let exe = std::env::current_exe().expect("test binary path");
    let output = Command::new(exe)
        .args([test_path, "--exact", "--ignored", "--nocapture"])
        .envs(envs)
        .output()
        .expect("spawn test binary");
    ChildRun {
        test_path: test_path.to_owned(),
        output,
    }
}

impl ChildRun {
    /// Panic unless the child exited successfully AND ran exactly one test,
    /// which passed.
    pub fn assert_passed(&self) {
        let stdout = String::from_utf8_lossy(&self.output.stdout);
        let verdict = if self.output.status.success() {
            ran_exactly_one_passing_test(&stdout)
        } else {
            Err(format!("exited with {}", self.output.status))
        };
        if let Err(why) = verdict {
            panic!(
                "child test `{}` {why}\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
                self.test_path,
                String::from_utf8_lossy(&self.output.stderr),
            );
        }
    }
}

/// Checks libtest's own summary lines: `running 1 test` before the run and
/// `test result: ok. 1 passed;` after it. The inner test's `--nocapture`
/// output sits between them, so the header is the FIRST `running ` line
/// (libtest prints it before any test starts) and the summary the LAST
/// `test result: ` line.
fn ran_exactly_one_passing_test(stdout: &str) -> Result<(), String> {
    let running = stdout
        .lines()
        .find_map(|line| line.strip_prefix("running "));
    let passed = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("test result: "));
    match running {
        Some("1 test") => {}
        Some(other) => {
            return Err(format!(
                "ran {other} instead of exactly 1 — the path no longer names one test"
            ));
        }
        None => return Err("printed no libtest header".to_owned()),
    }
    match passed {
        Some(summary) if summary.starts_with("ok. 1 passed;") => Ok(()),
        Some(summary) => Err(format!("did not pass exactly 1 test: {summary}")),
        None => Err("printed no libtest summary".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::ran_exactly_one_passing_test;

    #[test]
    fn one_passing_test_with_interleaved_output_is_accepted() {
        // "running late" and the fake result line are the inner test's own
        // `--nocapture` output, not libtest's.
        let stdout = "\nrunning 1 test\nrunning late\ntest result: made up\n\
                      test a::b ... ok\n\ntest result: ok. 1 passed; 0 failed; \
                      0 ignored; 0 measured; 41 filtered out; finished in 0.01s\n\n";
        assert_eq!(ran_exactly_one_passing_test(stdout), Ok(()));
    }

    #[test]
    fn a_filter_that_matches_nothing_is_rejected() {
        let stdout = "\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; \
                      0 ignored; 0 measured; 42 filtered out; finished in 0.00s\n\n";
        let why = ran_exactly_one_passing_test(stdout).unwrap_err();
        assert!(why.contains("0 tests"), "{why}");
    }

    #[test]
    fn missing_libtest_output_is_rejected() {
        assert!(ran_exactly_one_passing_test("").is_err());
        assert!(ran_exactly_one_passing_test("running 1 test\n").is_err());
    }
}
