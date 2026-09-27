//! Where a sounding clip is, exactly, so it can move to another mixer and
//! go on from that sample.
//!
//! A voice counts the clip's samples as they are READ, beneath the player's
//! speed change, so the count is a position in the clip itself whatever the
//! pitch. rodio's own `get_pos` is refreshed only every 5 ms and measured
//! after the speed change; a carried voice read from it would repeat or skip
//! up to a poll of audio.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::{ChannelCount, SampleRate, Source};

use super::DecodedSound;

/// Interleaved samples of a clip read so far by one player.
#[derive(Clone, Default)]
pub(super) struct Playhead(Arc<AtomicU64>);

/// Where a voice started in its clip and how far it has read since.
#[derive(Clone, Default)]
pub(super) struct ClipCursor {
    /// The clip frame the player began at.
    start_frame: usize,
    head: Playhead,
}

impl ClipCursor {
    fn new(start_frame: usize) -> Self {
        Self {
            start_frame,
            head: Playhead::default(),
        }
    }

    /// The clip frame this voice is at, wrapped for a loop; `None` once a
    /// one-shot has played to its end.
    pub(super) fn frame(&self, clip: &DecodedSound, looped: bool) -> Option<usize> {
        let channels = clip.channels.get() as u64;
        let frames = clip.samples.len() / clip.channels.get() as usize;
        if frames == 0 {
            return None;
        }
        let read = (self.head.0.load(Ordering::Relaxed) / channels) as usize;
        let at = self.start_frame + read;
        if looped {
            Some(at % frames)
        } else {
            (at < frames).then_some(at)
        }
    }
}

/// `clip` from `start_frame` on (a loop wraps back round to the clip's start
/// forever), counting what is read into the returned cursor. The voice reads
/// the clip's shared samples in place: nothing is copied per play.
pub(super) fn clip_from(
    clip: &DecodedSound,
    start_frame: usize,
    looped: bool,
) -> (Box<dyn Source + Send>, ClipCursor) {
    let channels = clip.channels.get() as usize;
    let frames = clip.samples.len() / channels;
    let start_frame = if frames == 0 {
        0
    } else if looped {
        start_frame % frames
    } else {
        start_frame.min(frames)
    };
    let cursor = ClipCursor::new(start_frame);
    let at = start_frame * channels;
    let span = clip.samples.len() - at;
    let source = SharedClip {
        samples: Arc::clone(&clip.samples),
        channels: clip.channels,
        sample_rate: clip.sample_rate,
        at,
        span,
        looped,
        head: cursor.head.clone(),
    };
    (Box::new(source), cursor)
}

/// A read cursor over a clip's shared samples.
struct SharedClip {
    samples: Arc<[f32]>,
    channels: ChannelCount,
    sample_rate: SampleRate,
    /// The next interleaved sample to read.
    at: usize,
    /// A one-shot's samples from where it started.
    span: usize,
    looped: bool,
    head: Playhead,
}

impl SharedClip {
    fn exhausted(&self) -> bool {
        self.at >= self.samples.len() && (!self.looped || self.samples.is_empty())
    }
}

impl Iterator for SharedClip {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.exhausted() {
            return None;
        }
        if self.at >= self.samples.len() {
            self.at = 0;
        }
        let sample = self.samples[self.at];
        self.at += 1;
        self.head.0.fetch_add(1, Ordering::Relaxed);
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.looped && !self.samples.is_empty() {
            (usize::MAX, None)
        } else {
            let left = self.samples.len().saturating_sub(self.at);
            (left, Some(left))
        }
    }
}

impl Source for SharedClip {
    fn current_span_len(&self) -> Option<usize> {
        if self.exhausted() {
            Some(0)
        } else if self.looped {
            None
        } else {
            Some(self.span)
        }
    }

    fn channels(&self) -> ChannelCount {
        self.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        if self.looped {
            return None;
        }
        let frames = self.span / self.channels.get() as usize;
        Some(Duration::from_secs_f64(
            frames as f64 / self.sample_rate.get() as f64,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(frames: usize) -> DecodedSound {
        DecodedSound {
            channels: ChannelCount::new(2).unwrap(),
            sample_rate: SampleRate::new(48_000).unwrap(),
            samples: (0..frames * 2).map(|i| (i / 2) as f32).collect(),
        }
    }

    #[test]
    fn a_voice_resumes_from_the_frame_it_had_read_to() {
        let clip = clip(10);
        let (mut source, cursor) = clip_from(&clip, 3, false);
        for _ in 0..4 {
            source.next();
        }
        assert_eq!(cursor.frame(&clip, false), Some(5));
        let (mut again, _) = clip_from(&clip, 5, false);
        assert_eq!(again.next(), Some(5.0));
        for _ in 0..10 {
            source.next();
        }
        assert_eq!(cursor.frame(&clip, false), None, "played to its end");
    }

    /// Every voice reads the one decoded copy: a play holds a reference to
    /// the clip's samples, never a copy of them.
    #[test]
    fn voices_share_the_decoded_samples() {
        let clip = clip(8);
        let (one, _) = clip_from(&clip, 0, false);
        let (two, _) = clip_from(&clip, 5, true);
        assert_eq!(Arc::strong_count(&clip.samples), 3);
        drop((one, two));
        assert_eq!(Arc::strong_count(&clip.samples), 1);
    }

    #[test]
    fn a_loop_resumes_mid_clip_and_wraps() {
        let clip = clip(4);
        let (mut source, cursor) = clip_from(&clip, 3, true);
        let first: Vec<f32> = (0..6).filter_map(|_| source.next()).collect();
        assert_eq!(first, [3.0, 3.0, 0.0, 0.0, 1.0, 1.0]);
        assert_eq!(cursor.frame(&clip, true), Some(2));
    }
}
