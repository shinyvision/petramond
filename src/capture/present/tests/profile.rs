use std::time::{Duration, Instant};

use super::*;

fn percentile(mut v: Vec<Duration>, p: f64) -> Duration {
    v.sort();
    v[((v.len() - 1) as f64 * p).round() as usize]
}

#[test]
#[ignore]
fn apply_profile() {
    for side in [52, 74] {
        let h = harness("present-profile", side, 30, 0x243F_6A88_85A3_08D3, 1000);
        let pool = Arc::new(JobPool::new(JobPool::default_threads()));
        let mut p = Presentation::new(crate::net::remap::local_name_tables(), pool);
        let mut replica = replica();
        p.set_window(window_over(side));

        let started = Instant::now();
        let (state, events) = seek(&h, None, 21.25);
        p.push(Op::Apply {
            id: 1,
            state,
            events: vec![events],
            at: 21.25,
        });
        settle(&mut p, &mut replica);
        let open = started.elapsed();
        let sections = replica.data().sections.len();

        p.push(Op::Queue(vec![h.frames_from(23, 6)]));
        p.push(Op::Time(27.25));
        settle(&mut p, &mut replica);
        let started = Instant::now();
        let (state, events) = seek(&h, Some(28), 21.25);
        p.push(Op::Apply {
            id: 2,
            state,
            events: vec![events],
            at: 21.25,
        });
        settle(&mut p, &mut replica);
        let far = started.elapsed();

        let mut back = Vec::new();
        let mut inline = 0;
        for round in 0..40u64 {
            let (state, events) = seek(&h, Some(22), 22.25);
            p.push(Op::Apply {
                id: 10 + 2 * round,
                state,
                events: vec![events],
                at: 22.25,
            });
            settle(&mut p, &mut replica);
            let (state, events) = seek(&h, Some(23), 21.25);
            p.push(Op::Apply {
                id: 11 + 2 * round,
                state,
                events: vec![events],
                at: 21.25,
            });
            let started = Instant::now();
            let out = p.drive(&mut replica, 1.0 / 60.0);
            back.push(started.elapsed());
            inline += usize::from(out.landed == vec![11 + 2 * round]);
            settle(&mut p, &mut replica);
        }
        eprintln!(
            "presentation @ {sections} sections: open {:.1} ms, far seek {:.2} ms, one tick back \
             (same frame {inline}/40) min {:.3} ms, median {:.3} ms, p90 {:.3} ms; {} pieces cached",
            open.as_secs_f64() * 1e3,
            far.as_secs_f64() * 1e3,
            back.iter().min().unwrap().as_secs_f64() * 1e3,
            percentile(back.clone(), 0.5).as_secs_f64() * 1e3,
            percentile(back, 0.9).as_secs_f64() * 1e3,
            replica.cached_pieces(),
        );
    }
}
