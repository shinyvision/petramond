use std::ffi::OsStr;
use std::process::{Command, Output};

#[must_use = "call `assert_passed` — a child that ran no test still exits 0"]
pub struct ChildRun {
    test_path: String,
    output: Output,
}

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
