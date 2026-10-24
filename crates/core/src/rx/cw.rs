//! Morse decoder for a known speed and tone: a Goertzel detector, adaptive threshold
//! and timing classified against the fixed dot length.

use crate::morse;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CwEvent {
    Char(char),
    /// End of a word.
    Space,
}

pub struct CwDecoder {
    n: usize,
    coeff: f32,
    buf: Vec<f32>,
    block_ms: f32,
    dot_ms: f32,
    noise: f32,
    peak: f32,
    /// Committed key state and how long it has lasted.
    on: bool,
    run_ms: f32,
    /// Raw state waiting to be confirmed (debounce).
    pending: Option<(bool, u32)>,
    symbol: String,
    word_open: bool,
    warmup: u32,
}

/// Blocks used to learn the noise floor (~300 ms) before decoding.
const WARMUP_BLOCKS: u32 = 30;
/// Blocks a state change must persist before it is accepted.
const DEBOUNCE_BLOCKS: u32 = 2;
/// Minimum peak-to-noise ratio (~12 dB) to consider anything a signal.
const MIN_SNR: f32 = 4.0;
const MIN_LEVEL: f32 = 1e-3;

impl CwDecoder {
    pub fn new(sample_rate: u32, wpm: f32, tone_hz: f32) -> CwDecoder {
        let fs = sample_rate as f32;
        let dot_ms = morse::unit_ms(wpm) as f32;
        let block_ms = (dot_ms / 5.0).clamp(2.0, 10.0);
        let n = (fs * block_ms / 1000.0).round().max(8.0) as usize;
        CwDecoder {
            n,
            coeff: 2.0 * (std::f32::consts::TAU * tone_hz / fs).cos(),
            buf: Vec::with_capacity(n),
            block_ms: n as f32 * 1000.0 / fs,
            dot_ms,
            noise: 0.0,
            peak: 0.0,
            on: false,
            run_ms: 0.0,
            pending: None,
            symbol: String::new(),
            word_open: false,
            warmup: 0,
        }
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.on = false;
        self.run_ms = 0.0;
        self.pending = None;
        self.symbol.clear();
        self.word_open = false;
        self.noise = 0.0;
        self.peak = 0.0;
        self.warmup = 0;
    }

    pub fn process(&mut self, samples: &[f32], mut on_event: impl FnMut(CwEvent)) {
        for &s in samples {
            self.buf.push(s);
            if self.buf.len() == self.n {
                let mag = self.magnitude();
                self.buf.clear();
                self.block(mag, &mut on_event);
            }
        }
    }

    fn magnitude(&self) -> f32 {
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in &self.buf {
            let s = x + self.coeff * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let power = (s1 * s1 + s2 * s2 - self.coeff * s1 * s2).max(0.0);
        2.0 * power.sqrt() / self.buf.len() as f32 // ≈ tone amplitude
    }

    fn block(&mut self, mag: f32, on_event: &mut impl FnMut(CwEvent)) {
        // Warm-up: learn the noise floor before deciding anything.
        if self.warmup < WARMUP_BLOCKS {
            self.warmup += 1;
            self.noise += (mag - self.noise) / self.warmup as f32;
            self.peak = self.peak.max(mag);
            return;
        }
        // Peak: instant attack, ~3 s decay. Noise: quick to fall, slow (~1.5 s) to rise.
        self.peak = if mag > self.peak { mag } else { self.peak * (-self.block_ms / 3000.0).exp() };
        let k = if mag < self.noise { 0.3 } else { self.block_ms / 1500.0 };
        self.noise += (mag - self.noise) * k;

        let span = self.peak - self.noise;
        let signal = self.peak > self.noise * MIN_SNR && self.peak > MIN_LEVEL;
        let raw = signal
            && if self.on { mag > self.noise + 0.4 * span } else { mag > self.noise + 0.6 * span };

        self.run_ms += self.block_ms;
        if raw != self.on {
            let (state, count) = self.pending.filter(|p| p.0 == raw).map_or((raw, 1), |(s, c)| (s, c + 1));
            if count >= DEBOUNCE_BLOCKS {
                let back = count as f32 * self.block_ms;
                self.transition(self.run_ms - back, on_event);
                self.on = raw;
                self.run_ms = back;
                self.pending = None;
            } else {
                self.pending = Some((state, count));
            }
        } else {
            self.pending = None;
        }

        if !self.on {
            self.check_gap(on_event);
        }
    }

    /// The committed state just ended after `duration` ms.
    fn transition(&mut self, duration: f32, on_event: &mut impl FnMut(CwEvent)) {
        if self.on {
            self.symbol.push(if duration < 2.0 * self.dot_ms { '.' } else { '-' });
            if self.symbol.len() > 8 {
                self.symbol.clear(); // not Morse; drop it
            }
        } else {
            self.check_gap(on_event);
        }
    }

    fn check_gap(&mut self, on_event: &mut impl FnMut(CwEvent)) {
        if self.run_ms >= 2.0 * self.dot_ms && !self.symbol.is_empty() {
            if let Some(c) = morse::decode(&self.symbol) {
                on_event(CwEvent::Char(c));
                self.word_open = true;
            }
            self.symbol.clear();
        }
        if self.run_ms >= 5.0 * self.dot_ms && self.word_open && self.symbol.is_empty() {
            on_event(CwEvent::Space);
            self.word_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{SampleSource, Synth, cw_segments, Seg};

    fn render(text: &str, wpm: f32, tone: f32, fs: u32, noise: f32) -> Vec<f32> {
        let mut segs = vec![Seg::silence(500.0)];
        segs.extend(cw_segments(text, wpm, tone));
        segs.push(Seg::silence(1500.0));
        let total: f64 = segs.iter().map(|s| s.ms).sum();
        let mut synth = Synth::new(segs.into_iter(), fs as f64, 0.5, total);
        let mut out = vec![0.0; synth.total_samples() as usize];
        synth.fill(&mut out);
        let mut seed = 7u32;
        for s in &mut out {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            *s += noise * ((seed >> 16) as f32 / 32768.0 - 1.0);
        }
        out
    }

    fn decode(audio: &[f32], wpm: f32, tone: f32, fs: u32) -> String {
        let mut dec = CwDecoder::new(fs, wpm, tone);
        let mut text = String::new();
        dec.process(audio, |e| match e {
            CwEvent::Char(c) => text.push(c),
            CwEvent::Space => text.push(' '),
        });
        text.trim().to_string()
    }

    #[test]
    fn decodes_clean_cw() {
        for wpm in [12.0, 20.0, 30.0] {
            let audio = render("CQ DE LU1ABC K", wpm, 700.0, 48000, 0.0);
            assert_eq!(decode(&audio, wpm, 700.0, 48000), "CQ DE LU1ABC K", "{wpm} wpm");
        }
    }

    #[test]
    fn decodes_noisy_cw() {
        let audio = render("SSTV 73", 18.0, 650.0, 44100, 0.15);
        assert_eq!(decode(&audio, 18.0, 650.0, 44100), "SSTV 73");
    }
}
