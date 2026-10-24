//! Phase-continuous tone synthesis from a stream of (frequency, duration) segments.
//!
//! Segment boundaries are placed on the *cumulative* ideal time, so rounding to whole
//! samples never accumulates: line N of a picture starts at exactly round(t_N · fs)
//! no matter how many thousands of pixels came before. That is what keeps an SSTV
//! picture from slanting.

use std::f64::consts::TAU;

/// One tone segment. `hz == 0.0` is silence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub hz: f32,
    pub ms: f64,
    /// Keyed (CW) segments get raised-cosine edges to avoid key clicks.
    pub keyed: bool,
}

impl Seg {
    pub const fn tone(hz: f32, ms: f64) -> Seg {
        Seg { hz, ms, keyed: false }
    }
    pub const fn keyed(hz: f32, ms: f64) -> Seg {
        Seg { hz, ms, keyed: true }
    }
    pub const fn silence(ms: f64) -> Seg {
        Seg { hz: 0.0, ms, keyed: false }
    }
}

/// Anything the audio output can play.
pub trait SampleSource: Send {
    /// Writes mono samples in [-1, 1]. Returns how many were written; fewer than
    /// `out.len()` means the source is exhausted.
    fn fill(&mut self, out: &mut [f32]) -> usize;
    fn total_samples(&self) -> u64;
}

const KEY_RAMP_MS: f64 = 4.0;
const FADE_IN_MS: f64 = 3.0;

pub struct Synth<I: Iterator<Item = Seg>> {
    segs: I,
    fs: f64,
    amp: f32,
    phase: f64,
    n: u64,
    cur: Option<Seg>,
    seg_start: u64,
    seg_end: u64,
    t_end: f64,
    total: u64,
    fade_in: u64,
}

impl<I: Iterator<Item = Seg>> Synth<I> {
    /// `fs` is the *effective* sample rate: the nominal device rate corrected by the
    /// sound card clock calibration, so that real-world timing comes out right.
    pub fn new(segs: I, fs: f64, amp: f32, total_ms: f64) -> Self {
        Synth {
            segs,
            fs,
            amp: amp.clamp(0.0, 1.0),
            phase: 0.0,
            n: 0,
            cur: None,
            seg_start: 0,
            seg_end: 0,
            t_end: 0.0,
            total: (total_ms * fs / 1000.0).round() as u64,
            fade_in: (FADE_IN_MS * fs / 1000.0) as u64,
        }
    }

    fn advance(&mut self) -> bool {
        while self.n >= self.seg_end {
            match self.segs.next() {
                Some(seg) => {
                    self.t_end += seg.ms * self.fs / 1000.0;
                    self.seg_start = self.seg_end;
                    self.seg_end = self.t_end.round() as u64;
                    self.cur = Some(seg);
                }
                None => {
                    self.cur = None;
                    return false;
                }
            }
        }
        true
    }

    fn key_envelope(&self, seg: &Seg) -> f32 {
        let len = self.seg_end - self.seg_start;
        let pos = self.n - self.seg_start;
        let ramp = ((KEY_RAMP_MS * self.fs / 1000.0) as u64).min(len / 2).max(1);
        let edge = pos.min(len.saturating_sub(1).saturating_sub(pos));
        if !seg.keyed || edge >= ramp {
            1.0
        } else {
            let x = edge as f64 / ramp as f64;
            (0.5 - 0.5 * (std::f64::consts::PI * x).cos()) as f32
        }
    }
}

impl<I: Iterator<Item = Seg> + Send> SampleSource for Synth<I> {
    fn fill(&mut self, out: &mut [f32]) -> usize {
        for (i, slot) in out.iter_mut().enumerate() {
            if !self.advance() {
                return i;
            }
            let seg = self.cur.expect("advance() returned true");
            let mut v = if seg.hz > 0.0 {
                let s = self.phase.sin() as f32;
                self.phase += TAU * seg.hz as f64 / self.fs;
                if self.phase > TAU {
                    self.phase -= TAU;
                }
                s * self.key_envelope(&seg)
            } else {
                0.0
            };
            if self.n < self.fade_in {
                v *= self.n as f32 / self.fade_in as f32;
            }
            *slot = v * self.amp;
            self.n += 1;
        }
        out.len()
    }

    fn total_samples(&self) -> u64 {
        self.total
    }
}

/// Morse segments for `text` (unknown characters are skipped).
pub fn cw_segments(text: &str, wpm: f32, tone_hz: f32) -> Vec<Seg> {
    let u = crate::morse::unit_ms(wpm);
    let mut out = Vec::new();
    for (wi, word) in text.split_whitespace().enumerate() {
        if wi > 0 {
            out.push(Seg::silence(4.0 * u)); // 3 already added after the last char → 7
        }
        for c in word.chars() {
            let Some(code) = crate::morse::encode(c) else { continue };
            for el in code.chars() {
                let len = if el == '-' { 3.0 * u } else { u };
                out.push(Seg::keyed(tone_hz, len));
                out.push(Seg::silence(u));
            }
            out.push(Seg::silence(2.0 * u));
        }
    }
    out
}

pub const FSKID_BIT_MS: f64 = 22.0;
pub const FSKID_ONE_HZ: f32 = 1900.0;
pub const FSKID_ZERO_HZ: f32 = 2100.0;

/// MMSSTV-style FSK ID: 6-bit symbols, LSB first, 22 ms per bit.
/// Frame: 0x20 0x2A, then each character as (ASCII − 0x20), then 0x01.
pub fn fskid_segments(text: &str) -> Vec<Seg> {
    let mut symbols = vec![0x20u8, 0x2A];
    symbols.extend(
        text.chars()
            .map(|c| c.to_ascii_uppercase())
            .filter(|c| (' '..='_').contains(c))
            .map(|c| c as u8 - 0x20),
    );
    symbols.push(0x01);
    let mut out = Vec::with_capacity(symbols.len() * 6);
    for mut s in symbols {
        for _ in 0..6 {
            let hz = if s & 1 == 1 { FSKID_ONE_HZ } else { FSKID_ZERO_HZ };
            out.push(Seg::tone(hz, FSKID_BIT_MS));
            s >>= 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_do_not_drift() {
        // 10 000 segments of 0.19 ms (PD120 pixels) at 11025 Hz: each segment is
        // ~2.09 samples, but the total must still land on round(1900 ms · fs).
        let segs = std::iter::repeat_n(Seg::tone(1500.0, 0.19), 10_000);
        let fs = 11025.0;
        let mut s = Synth::new(segs, fs, 1.0, 1900.0);
        let mut buf = vec![0.0; 100_000];
        let n = s.fill(&mut buf);
        assert_eq!(n as u64, (1900.0 * fs / 1000.0_f64).round() as u64);
        assert_eq!(n as u64, s.total_samples());
    }

    #[test]
    fn cw_timing_paris() {
        // "PARIS " (with its 7-unit word space) is exactly 50 units long; the final
        // word ends with a plain 3-unit character space instead.
        let segs = cw_segments("PARIS PARIS", 20.0, 700.0);
        let total: f64 = segs.iter().map(|s| s.ms).sum();
        let u = crate::morse::unit_ms(20.0);
        assert!((total - (50.0 + 46.0) * u).abs() < 1e-6, "{}", total / u);
    }

    #[test]
    fn fskid_frame() {
        let segs = fskid_segments("lu1abc");
        // header (2) + 6 chars + terminator, 6 bits each
        assert_eq!(segs.len(), 9 * 6);
        // First symbol 0x20 = 100000b, LSB first → five zeros then a one.
        let hz: Vec<f32> = segs[..6].iter().map(|s| s.hz).collect();
        assert_eq!(hz, [2100.0, 2100.0, 2100.0, 2100.0, 2100.0, 1900.0]);
    }
}
