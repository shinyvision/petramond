use std::io::Read;
use std::time::Duration;

use super::http::Stream;

/// A body that hands out its bytes, then stalls without ending.
struct Stalls(Vec<u8>);

impl Read for Stalls {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.0.is_empty() {
            std::thread::sleep(Duration::from_secs(30));
            return Ok(0);
        }
        let n = out.len().min(self.0.len());
        out[..n].copy_from_slice(&self.0[..n]);
        self.0.drain(..n);
        Ok(n)
    }
}

/// A stalled connection fails the read once nothing has arrived for the idle
/// budget, after everything that did arrive was read.
#[test]
fn a_stalled_body_fails_the_read_after_the_idle_budget() {
    let mut stream = Stream::for_test(
        200,
        &[],
        Stalls(b"partial".to_vec()),
        Duration::from_millis(50),
    );
    let mut got = [0u8; 7];
    stream.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"partial");
    let started = std::time::Instant::now();
    let e = stream.read(&mut [0u8; 8]).expect_err("a stall is an error");
    assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_whole_body_reads_to_its_end() {
    let body: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
    let mut stream = Stream::for_test(
        200,
        &[("content-length", "200000"), ("X-Content-Sha256", "ABC")],
        std::io::Cursor::new(body.clone()),
        Duration::from_secs(5),
    );
    assert_eq!(stream.content_length, Some(200_000));
    assert_eq!(stream.sha256.as_deref(), Some("abc"));
    let mut read = Vec::new();
    stream.read_to_end(&mut read).unwrap();
    assert_eq!(read, body);
}
