//! Interleaved f32 audio carried from one format to another, a stream at a
//! time: the world's sound as each tap asked for it.

/// A streaming converter from `(channels, sample_rate)` to another. Channels
/// are mapped first (a mono side is spread or averaged), then the rate is
/// resampled linearly, with the position carried across calls so a stream
/// cut into any pieces converts to the same samples.
pub struct FormatConverter {
    from: (u16, u32),
    to: (u16, u32),
    /// The next output frame's position, in input frames after `last`.
    pos: f64,
    /// The last input frame of the previous call (already mapped), once one
    /// has arrived.
    last: Option<Vec<f32>>,
    mapped: Vec<f32>,
}

impl FormatConverter {
    pub fn new(from: (u16, u32), to: (u16, u32)) -> Self {
        let clamp = |(c, r): (u16, u32)| (c.max(1), r.max(1));
        Self {
            from: clamp(from),
            to: clamp(to),
            pos: 0.0,
            last: None,
            mapped: Vec::new(),
        }
    }

    pub fn output_format(&self) -> (u16, u32) {
        self.to
    }

    /// Append `input` (interleaved, whole frames at the input format)
    /// converted to `out`; answers the frames appended.
    pub fn convert(&mut self, input: &[f32], out: &mut Vec<f32>) -> usize {
        let (from_ch, from_rate) = (usize::from(self.from.0), self.from.1);
        let (to_ch, to_rate) = (usize::from(self.to.0), self.to.1);
        let frames = input.len() / from_ch;
        if from_rate == to_rate {
            if from_ch == to_ch {
                out.extend_from_slice(&input[..frames * from_ch]);
            } else {
                for frame in input.chunks_exact(from_ch) {
                    map_frame(frame, to_ch, out);
                }
            }
            return frames;
        }
        self.mapped.clear();
        for frame in input.chunks_exact(from_ch) {
            map_frame(frame, to_ch, &mut self.mapped);
        }
        let mut mapped = std::mem::take(&mut self.mapped);
        let last = match self.last.take() {
            Some(last) => last,
            None if mapped.len() >= to_ch => {
                let first = mapped[..to_ch].to_vec();
                mapped.drain(..to_ch);
                first
            }
            None => {
                self.mapped = mapped;
                return 0;
            }
        };
        let available = mapped.len() / to_ch;
        let step = f64::from(from_rate) / f64::from(to_rate);
        let frame_at = |i: usize| -> &[f32] {
            if i == 0 {
                &last
            } else {
                &mapped[(i - 1) * to_ch..i * to_ch]
            }
        };
        let mut produced = 0;
        while self.pos < available as f64 {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let (a, b) = (frame_at(i), frame_at(i + 1));
            out.extend(a.iter().zip(b).map(|(a, b)| a + (b - a) * t));
            produced += 1;
            self.pos += step;
        }
        self.pos -= available as f64;
        self.last = Some(if available == 0 {
            last
        } else {
            mapped[(available - 1) * to_ch..available * to_ch].to_vec()
        });
        mapped.clear();
        self.mapped = mapped;
        produced
    }
}

fn map_frame(frame: &[f32], to: usize, out: &mut Vec<f32>) {
    match (frame.len(), to) {
        (from, to) if from == to => out.extend_from_slice(frame),
        (_, 1) => out.push(frame.iter().sum::<f32>() / frame.len() as f32),
        (1, _) => out.extend(std::iter::repeat_n(frame[0], to)),
        _ => out.extend((0..to).map(|c| frame.get(c).copied().unwrap_or(0.0))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_converts_the_same_however_it_is_cut_and_keeps_its_length() {
        let input: Vec<f32> = (0..4_800 * 2).map(|i| (i as f32 * 0.003).sin()).collect();
        let whole = {
            let mut c = FormatConverter::new((2, 48_000), (1, 44_100));
            let mut out = Vec::new();
            c.convert(&input, &mut out);
            out
        };
        let mut cut = Vec::new();
        let mut c = FormatConverter::new((2, 48_000), (1, 44_100));
        for piece in input.chunks(2 * 37) {
            c.convert(piece, &mut cut);
        }
        assert_eq!(whole, cut);
        let expected = 4_800.0 * 44_100.0 / 48_000.0;
        assert!(
            (whole.len() as f64 - expected).abs() <= 1.0,
            "{}",
            whole.len()
        );
    }
}
