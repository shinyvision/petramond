use std::time::Instant;

#[derive(Clone, Debug)]
pub struct TokenBucket {
    capacity: f64,
    per_second: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    pub fn new(capacity: f64, per_second: f64, now: Instant) -> Self {
        Self {
            capacity,
            per_second,
            tokens: capacity,
            last: now,
        }
    }

    pub fn try_take(&mut self, cost: f64, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now.max(self.last);
        self.tokens = (self.tokens + elapsed * self.per_second).min(self.capacity);
        if self.tokens < cost {
            return false;
        }
        self.tokens -= cost;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bursts_drain_and_time_refills_up_to_capacity() {
        let t0 = Instant::now();
        let mut bucket = TokenBucket::new(3.0, 1.0, t0);
        assert!((0..3).all(|_| bucket.try_take(1.0, t0)));
        assert!(!bucket.try_take(1.0, t0), "the burst is spent");
        assert!(!bucket.try_take(1.0, t0 + Duration::from_millis(500)));
        assert!(bucket.try_take(1.0, t0 + Duration::from_millis(1600)));

        let later = t0 + Duration::from_secs(3600);
        assert!((0..3).all(|_| bucket.try_take(1.0, later)));
        assert!(
            !bucket.try_take(1.0, later),
            "an idle hour banks only the capacity"
        );
    }
}
