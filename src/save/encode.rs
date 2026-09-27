use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use petramond_world::chunk::SectionPos;

use super::codec::{self, SectionSnapshot};
use super::palette::Palette;

enum State {
    Pending(Vec<SectionSnapshot>),
    Encoding,
    Done(Vec<(SectionPos, Vec<u8>)>),
    Taken,
}

struct Shared {
    state: Mutex<State>,
    done: Condvar,
}

#[derive(Clone)]
pub(super) struct EncodeSlot(Arc<Shared>);

impl EncodeSlot {
    pub(super) fn new(snaps: Vec<SectionSnapshot>) -> Self {
        Self(Arc::new(Shared {
            state: Mutex::new(State::Pending(snaps)),
            done: Condvar::new(),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn encode(&self, pal: &Palette) {
        let snaps = {
            let mut state = self.lock();
            match std::mem::replace(&mut *state, State::Encoding) {
                State::Pending(snaps) => snaps,
                other => {
                    *state = other;
                    return;
                }
            }
        };
        let mut restore = Restore {
            slot: self,
            snaps: Some(snaps),
        };
        let records = restore
            .snaps
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|s| (s.pos, codec::encode_snapshot(s, pal)))
            .collect();
        restore.snaps = None;
        *self.lock() = State::Done(records);
        self.0.done.notify_all();
    }

    pub(super) fn take(&self, pal: &Palette) -> Vec<(SectionPos, Vec<u8>)> {
        self.encode(pal);
        let mut state = self.lock();
        loop {
            match std::mem::replace(&mut *state, State::Taken) {
                State::Done(records) => return records,
                State::Encoding => {
                    *state = State::Encoding;
                    state = self
                        .0
                        .done
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                pending @ State::Pending(_) => {
                    *state = pending;
                    drop(state);
                    self.encode(pal);
                    state = self.lock();
                }
                State::Taken => unreachable!("an encode slot is taken once"),
            }
        }
    }
}

struct Restore<'a> {
    slot: &'a EncodeSlot,
    snaps: Option<Vec<SectionSnapshot>>,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if let Some(snaps) = self.snaps.take() {
            *self.slot.lock() = State::Pending(snaps);
            self.slot.0.done.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::block::Block;
    use petramond_world::section::Section;

    fn snaps() -> Vec<SectionSnapshot> {
        (0..3)
            .map(|x| {
                let mut s = Section::new(x, 4, 0);
                s.set_block(1, 1, 1, Block::Stone);
                SectionSnapshot::from_section(&s)
            })
            .collect()
    }

    fn expected(pal: &Palette) -> Vec<(SectionPos, Vec<u8>)> {
        snaps()
            .iter()
            .map(|s| (s.pos, codec::encode_snapshot(s, pal)))
            .collect()
    }

    #[test]
    fn a_group_no_job_started_is_encoded_by_the_taker() {
        let pal = Palette::identity();
        assert_eq!(EncodeSlot::new(snaps()).take(&pal), expected(&pal));
    }

    #[test]
    fn a_group_encoded_elsewhere_is_taken_as_is() {
        let pal = Arc::new(Palette::identity());
        let slot = EncodeSlot::new(snaps());
        let job = {
            let (slot, pal) = (slot.clone(), pal.clone());
            std::thread::spawn(move || slot.encode(&pal))
        };
        let records = slot.take(&pal);
        job.join().unwrap();
        assert_eq!(records, expected(&pal), "encoded exactly once, in order");
    }

    #[test]
    fn encoding_twice_is_a_no_op() {
        let pal = Palette::identity();
        let slot = EncodeSlot::new(snaps());
        slot.encode(&pal);
        slot.encode(&pal);
        assert_eq!(slot.take(&pal), expected(&pal));
    }
}
