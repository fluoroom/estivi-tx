//! DTMF detection with the Goertzel algorithm over ~25 ms blocks.

pub(crate) const ROWS: [f32; 4] = [697.0, 770.0, 852.0, 941.0];
pub(crate) const COLS: [f32; 4] = [1209.0, 1336.0, 1477.0, 1633.0];
pub(crate) const KEYS: [[char; 4]; 4] = [['1', '2', '3', 'A'], ['4', '5', '6', 'B'], ['7', '8', '9', 'C'], ['*', '0', '#', 'D']];

const BLOCK_S: f32 = 0.0256;
/// Both tones together must carry most of the block energy.
const MIN_TONE_SHARE: f32 = 0.6;
/// Next strongest tone of the same group must be at least ~8 dB below.
const MAX_SECOND: f32 = 0.16;
/// Allowed level difference between the row and column tone (~±8 dB).
const MAX_TWIST: f32 = 6.3;
/// Mean power floor (≈ −60 dBFS) under which the block is treated as silence.
const MIN_POWER: f32 = 1e-6;

pub struct DtmfDecoder {
    n: usize,
    coeffs: [f32; 8],
    buf: Vec<f32>,
    prev: Option<char>,
    held: Option<char>,
}

impl DtmfDecoder {
    pub fn new(sample_rate: u32) -> DtmfDecoder {
        let fs = sample_rate as f32;
        let mut coeffs = [0.0; 8];
        for (c, f) in coeffs.iter_mut().zip(ROWS.iter().chain(COLS.iter())) {
            *c = 2.0 * (std::f32::consts::TAU * f / fs).cos();
        }
        let n = (fs * BLOCK_S).round() as usize;
        DtmfDecoder { n, coeffs, buf: Vec::with_capacity(n), prev: None, held: None }
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.prev = None;
        self.held = None;
    }

    /// Feeds audio; `on_digit` is called once per key press.
    pub fn process(&mut self, samples: &[f32], mut on_digit: impl FnMut(char)) {
        for &s in samples {
            self.buf.push(s);
            if self.buf.len() == self.n {
                let detected = self.detect();
                self.buf.clear();
                // A digit counts when two consecutive blocks agree; it must be released
                // (a block without it) before the same key can count again.
                match detected {
                    Some(d) if self.prev == Some(d) && self.held != Some(d) => {
                        self.held = Some(d);
                        on_digit(d);
                    }
                    None => self.held = None,
                    _ => {}
                }
                self.prev = detected;
            }
        }
    }

    fn detect(&self) -> Option<char> {
        let n = self.buf.len() as f32;
        let energy: f32 = self.buf.iter().map(|x| x * x).sum();
        if energy / n < MIN_POWER {
            return None;
        }
        let mut p = [0.0f32; 8];
        for (k, &coeff) in self.coeffs.iter().enumerate() {
            let (mut s1, mut s2) = (0.0f32, 0.0f32);
            for &x in &self.buf {
                let s = x + coeff * s1 - s2;
                s2 = s1;
                s1 = s;
            }
            // Energy of the tone inside the block (comparable to `energy`).
            p[k] = 2.0 * (s1 * s1 + s2 * s2 - coeff * s1 * s2) / n;
        }
        let best = |group: &[f32]| {
            let mut idx: Vec<usize> = (0..4).collect();
            idx.sort_by(|&a, &b| group[b].total_cmp(&group[a]));
            (idx[0], group[idx[0]], group[idx[1]])
        };
        let (r, pr, r2) = best(&p[..4]);
        let (c, pc, c2) = best(&p[4..]);
        let ok = (pr + pc) / energy > MIN_TONE_SHARE
            && r2 < pr * MAX_SECOND
            && c2 < pc * MAX_SECOND
            && pc / pr < MAX_TWIST
            && pr / pc < MAX_TWIST;
        ok.then_some(KEYS[r][c])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tones(digits: &str, fs: u32, noise: f32) -> Vec<f32> {
        let mut out = Vec::new();
        let mut seed = 12345u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (seed >> 16) as f32 / 32768.0 - 1.0
        };
        for d in digits.chars() {
            let (r, c) = (0..4).flat_map(|r| (0..4).map(move |c| (r, c))).find(|&(r, c)| KEYS[r][c] == d).unwrap();
            for i in 0..(fs as f32 * 0.08) as usize {
                let t = i as f32 / fs as f32;
                let v = 0.3 * (std::f32::consts::TAU * ROWS[r] * t).sin() + 0.3 * (std::f32::consts::TAU * COLS[c] * t).sin();
                out.push(v + noise * rnd());
            }
            for _ in 0..(fs as f32 * 0.06) as usize {
                out.push(noise * rnd());
            }
        }
        out
    }

    #[test]
    fn decodes_all_keys() {
        for fs in [8000, 44100, 48000] {
            let mut dec = DtmfDecoder::new(fs);
            let mut got = String::new();
            dec.process(&tones("123A456B789C*0#D", fs, 0.05), |d| got.push(d));
            assert_eq!(got, "123A456B789C*0#D", "fs={fs}");
        }
    }

    #[test]
    fn repeated_key_needs_release() {
        let mut dec = DtmfDecoder::new(48000);
        let mut got = String::new();
        dec.process(&tones("55", 48000, 0.0), |d| got.push(d));
        assert_eq!(got, "55");
    }

    #[test]
    fn ignores_speech_like_noise_and_single_tones() {
        let fs = 48000;
        let mut dec = DtmfDecoder::new(fs);
        let mut got = String::new();
        let single: Vec<f32> = (0..fs).map(|i| 0.5 * (std::f32::consts::TAU * 1000.0 * i as f32 / fs as f32).sin()).collect();
        dec.process(&single, |d| got.push(d));
        let mut seed = 1u32;
        let noise: Vec<f32> = (0..fs)
            .map(|_| {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (seed >> 16) as f32 / 32768.0 - 1.0
            })
            .collect();
        dec.process(&noise, |d| got.push(d));
        assert!(got.is_empty(), "{got}");
    }
}
