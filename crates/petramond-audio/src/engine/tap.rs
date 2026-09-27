//! The world's sound as a sub-mix on the device, and a copy of it for taps.
//!
//! On the device, every world voice mixes into one world sub-mix, which the
//! device mixer plays like any other source. The wrapper that hands the
//! sub-mix to the device also COPIES its samples, while anyone is tapping, in
//! chunks passed to the frame thread over a channel; the frame thread only
//! drains them. The player hears exactly what is copied, and with no tap the
//! wrapper passes samples through untouched.
//!
//! Spent chunks come back over a second channel, so a steady tap allocates
//! nothing on the audio thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use rodio::mixer::{self, Mixer, MixerSource};
use rodio::source::Source;
use rodio::{ChannelCount, Sample, SampleRate};

use super::Audio;

const CHUNK_SAMPLES: usize = 1024;

pub(super) struct TapSource {
    inner: MixerSource,
    tapped: Arc<AtomicBool>,
    chunk: Vec<f32>,
    out: Sender<Vec<f32>>,
    spent: Receiver<Vec<f32>>,
}

impl TapSource {
    fn flush(&mut self) {
        if self.chunk.is_empty() {
            return;
        }
        let next = self
            .spent
            .try_recv()
            .map(|mut spare| {
                spare.clear();
                spare
            })
            .unwrap_or_else(|_| Vec::with_capacity(CHUNK_SAMPLES));
        let full = std::mem::replace(&mut self.chunk, next);
        let _ = self.out.send(full);
    }
}

impl Iterator for TapSource {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        let sample = self.inner.next().unwrap_or(0.0);
        if self.tapped.load(Ordering::Relaxed) {
            self.chunk.push(sample);
            if self.chunk.len() >= CHUNK_SAMPLES {
                self.flush();
            }
        } else if !self.chunk.is_empty() {
            self.flush();
        }
        Some(sample)
    }
}

impl Source for TapSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

pub(super) struct WorldSubmix {
    mixer: Mixer,
    format: (u16, u32),
    tapped: Arc<AtomicBool>,
    copies: Receiver<Vec<f32>>,
    spent: Sender<Vec<f32>>,
}

impl WorldSubmix {
    pub(super) fn new(channels: ChannelCount, sample_rate: SampleRate) -> (Self, TapSource) {
        let (mixer, inner) = mixer::mixer(channels, sample_rate);
        let tapped = Arc::new(AtomicBool::new(false));
        let (out, copies) = channel();
        let (spent_tx, spent_rx) = channel();
        (
            Self {
                mixer,
                format: (channels.get(), sample_rate.get()),
                tapped: Arc::clone(&tapped),
                copies,
                spent: spent_tx,
            },
            TapSource {
                inner,
                tapped,
                chunk: Vec::with_capacity(CHUNK_SAMPLES),
                out,
                spent: spent_rx,
            },
        )
    }

    pub(super) fn mixer(&self) -> &Mixer {
        &self.mixer
    }
}

impl Audio {
    pub fn world_format(&self) -> Option<(u16, u32)> {
        self.offline_format()
            .or_else(|| self.submix.as_ref().map(|submix| submix.format))
    }

    pub fn mixes(&self) -> bool {
        true
    }

    pub fn has_device(&self) -> bool {
        self.submix.is_some()
    }

    pub fn set_world_copied(&mut self, copied: bool) {
        let Some(submix) = &self.submix else {
            return;
        };
        let was = submix.tapped.swap(copied, Ordering::Relaxed);
        if copied && !was {
            while let Ok(stale) = submix.copies.try_recv() {
                let _ = submix.spent.send(stale);
            }
        }
    }

    pub fn drain_world_copy(&mut self, out: &mut Vec<f32>) {
        let Some(submix) = &self.submix else {
            return;
        };
        while let Ok(chunk) = submix.copies.try_recv() {
            out.extend_from_slice(&chunk);
            let _ = submix.spent.send(chunk);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_copies_exactly_what_the_device_plays_and_changes_none_of_it() {
        let channels = ChannelCount::new(2).unwrap();
        let rate = SampleRate::new(48_000).unwrap();
        let (submix, mut device_side) = WorldSubmix::new(channels, rate);
        let tone: Vec<f32> = (0..20_000).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        submix.mixer().add(rodio::buffer::SamplesBuffer::new(
            channels,
            rate,
            tone.clone(),
        ));
        let tapped = Arc::clone(&submix.tapped);
        let mut played = Vec::new();
        for _ in 0..3_000 {
            played.push(device_side.next().unwrap());
        }
        tapped.store(true, Ordering::Relaxed);
        let from = played.len();
        for _ in 0..5_001 {
            played.push(device_side.next().unwrap());
        }
        tapped.store(false, Ordering::Relaxed);
        let to = played.len();
        for _ in 0..3_000 {
            played.push(device_side.next().unwrap());
        }
        assert_eq!(
            &played[..tone.len().min(played.len())],
            &tone[..played.len()]
        );
        let mut copied = Vec::new();
        while let Ok(chunk) = submix.copies.try_recv() {
            copied.extend(chunk);
        }
        assert_eq!(copied, played[from..to], "the copy is the played stretch");
    }

    #[test]
    fn the_world_submix_never_ends_on_the_device() {
        let (_submix, mut device_side) = WorldSubmix::new(
            ChannelCount::new(1).unwrap(),
            SampleRate::new(8_000).unwrap(),
        );
        assert!((0..100).all(|_| device_side.next() == Some(0.0)));
    }
}
