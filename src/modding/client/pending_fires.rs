use crate::player::RigId;

const ECHO_WINDOW: f64 = 1.0;

#[derive(Debug, Default)]
pub(super) struct PendingFires {
    entries: Vec<(RigId, u16, f64)>,
    clock: f64,
}

impl PendingFires {
    pub fn advance(&mut self, dt: f32) {
        self.clock += f64::from(dt.max(0.0));
        let cutoff = self.clock - ECHO_WINDOW;
        self.entries.retain(|&(_, _, fired)| fired > cutoff);
    }

    pub fn fired(&mut self, rig: RigId, event: u16) {
        self.entries.push((rig, event, self.clock));
    }

    pub fn absorb(&mut self, rig: RigId, event: u16) -> bool {
        match self
            .entries
            .iter()
            .position(|&(r, e, _)| (r, e) == (rig, event))
        {
            Some(at) => {
                self.entries.remove(at);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_fire_absorbs_one_echo_for_the_same_time_at_any_frame_rate() {
        let (rig, swing, other) = (RigId(0), 3, 4);
        let mut pending = PendingFires::default();
        pending.fired(rig, swing);
        pending.fired(rig, swing);
        assert!(
            !pending.absorb(rig, other),
            "an event nobody fired locally is the server's"
        );
        assert!(pending.absorb(rig, swing));
        assert!(pending.absorb(rig, swing), "two fires, two echoes");
        assert!(
            !pending.absorb(rig, swing),
            "the third echo is a new server fire"
        );

        for frame_dt in [1.0 / 30.0, 1.0 / 240.0] {
            let mut pending = PendingFires::default();
            pending.fired(rig, swing);
            for _ in 0..(0.9 / frame_dt) as usize {
                pending.advance(frame_dt);
            }
            assert!(
                pending.absorb(rig, swing),
                "still waiting 0.9 s later at {frame_dt}"
            );
            pending.fired(rig, other);
            for _ in 0..(1.1 / frame_dt) as usize {
                pending.advance(frame_dt);
            }
            assert!(
                !pending.absorb(rig, other),
                "expired past the window at {frame_dt}"
            );
        }
    }
}
