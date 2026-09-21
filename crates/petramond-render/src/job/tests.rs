use super::*;
use std::time::{Duration, Instant};

fn settle<T: Send + 'static>(job: &Job<T>) -> Result<T, JobLost> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(outcome) = job.poll() {
            return outcome;
        }
        assert!(Instant::now() < deadline, "job never ended");
        std::thread::yield_now();
    }
}

#[test]
fn a_job_yields_its_value_once_and_a_panic_is_a_loss() {
    let pool = JobPool::new(1);
    assert_eq!(settle(&Job::spawn(&pool, || 7)), Ok(7));
    let lost: Job<u8> = Job::spawn(&pool, || panic!("expected by the test"));
    assert_eq!(settle(&lost), Err(JobLost));
    let mut slot = Some(Job::spawn(&JobPool::inline(), || "now"));
    assert_eq!(Job::finish(&mut slot), Some(Ok("now")));
    assert!(slot.is_none());
}
