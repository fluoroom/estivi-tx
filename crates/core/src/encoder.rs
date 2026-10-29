//! Picture → SSTV tone segments, for every supported mode.

use std::sync::Arc;

use image::RgbImage;
use serde::{Deserialize, Serialize};

use crate::modes::*;
use crate::synth::{Seg, Synth, cw_segments};

pub const SYNC_HZ: f32 = 1200.0;
pub const BLACK_HZ: f32 = 1500.0;
pub const LEADER_HZ: f32 = 1900.0;
pub const WHITE_HZ: f32 = 2300.0;
pub const VIS_ONE_HZ: f32 = 1100.0;
pub const VIS_ZERO_HZ: f32 = 1300.0;

fn pixel_hz(v: u8) -> f32 {
    BLACK_HZ + (WHITE_HZ - BLACK_HZ) * v as f32 / 255.0
}

/// How Robot/PD luminance and chroma are scaled before modulation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorRange {
    /// Full swing (Y 0–255, as in JPEG). QSSTV decodes this exactly (measured: black
    /// and white bars come back as 0–2 and 253–255); slowrx and pySSTV use it too.
    #[default]
    Full,
    /// ITU-R BT.601 studio swing (Y 16–235), for receivers that expect it.
    Studio,
}

/// Returns (Y, Cb = B-Y, Cr = R-Y).
pub fn rgb_to_ycc([r, g, b]: [u8; 3], range: ColorRange) -> [u8; 3] {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    let (y, cb, cr) = match range {
        ColorRange::Studio => (
            16.0 + (65.481 * r + 128.553 * g + 24.966 * b) / 255.0,
            128.0 + (-37.797 * r - 74.203 * g + 112.0 * b) / 255.0,
            128.0 + (112.0 * r - 93.786 * g - 18.214 * b) / 255.0,
        ),
        ColorRange::Full => (
            0.299 * r + 0.587 * g + 0.114 * b,
            128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b,
            128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b,
        ),
    };
    [y, cb, cr].map(|v| v.round().clamp(0.0, 255.0) as u8)
}

fn avg(a: u8, b: u8) -> u8 {
    (a as u16 + b as u16).div_ceil(2) as u8
}

#[derive(Clone, Debug)]
pub struct CwIdent {
    pub text: String,
    pub wpm: f32,
    pub tone_hz: f32,
}

#[derive(Clone, Debug, Default)]
pub struct TxOptions {
    /// MMSSTV-style 800 ms tone burst before the VIS, to open VOX circuits.
    pub vox_preamble: bool,
    pub color_range: ColorRange,
    /// CW before the VIS (after the VOX burst, if any).
    pub cw_preamble: Option<CwIdent>,
    /// CW after the picture.
    pub cw_id: Option<CwIdent>,
}

/// A picture already sized for its mode, with the planes the mode needs.
pub struct ScanImage {
    spec: &'static ModeSpec,
    rgb: Vec<[u8; 3]>,
    /// Y, Cb, Cr per pixel (Robot/PD only).
    ycc: Vec<[u8; 3]>,
}

impl ScanImage {
    /// `img` must be exactly `spec.width × spec.height`.
    pub fn new(mode: Mode, img: &RgbImage, range: ColorRange) -> anyhow::Result<ScanImage> {
        let spec = mode.spec();
        anyhow::ensure!(
            img.width() == spec.width && img.height() == spec.height,
            "the image is {}x{} and {} needs {}x{}",
            img.width(),
            img.height(),
            spec.name,
            spec.width,
            spec.height
        );
        let rgb: Vec<[u8; 3]> = img.pixels().map(|p| p.0).collect();
        let ycc = match spec.family {
            Family::Robot | Family::Pd => rgb.iter().map(|&p| rgb_to_ycc(p, range)).collect(),
            // B/W modes always use full-swing luma: black must sit exactly at 1500 Hz.
            Family::RobotBw => rgb.iter().map(|&p| rgb_to_ycc(p, ColorRange::Full)).collect(),
            Family::Martin | Family::Scottie => Vec::new(),
        };
        Ok(ScanImage { spec, rgb, ycc })
    }

    pub fn spec(&self) -> &'static ModeSpec {
        self.spec
    }

    fn rgb_row(&self, y: u32) -> &[[u8; 3]] {
        let w = self.spec.width as usize;
        &self.rgb[y as usize * w..(y as usize + 1) * w]
    }

    fn ycc_row(&self, y: u32) -> &[[u8; 3]] {
        let w = self.spec.width as usize;
        &self.ycc[y as usize * w..(y as usize + 1) * w]
    }

    fn scan(out: &mut Vec<Seg>, values: impl ExactSizeIterator<Item = u8>, total_ms: f64) {
        let px_ms = total_ms / values.len() as f64;
        out.extend(values.map(|v| Seg::tone(pixel_hz(v), px_ms)));
    }

    /// Appends the segments of transmitted line `line` (0-based) to `out`.
    pub fn line(&self, line: u32, out: &mut Vec<Seg>) {
        let spec = self.spec;
        match spec.timing {
            Timing::Martin { scan_ms } | Timing::Scottie { scan_ms } => {
                // M3/M4 send 128 lines of a 256-row picture: average each row pair.
                let row: Vec<[u8; 3]> = if spec.tx_lines == spec.height {
                    self.rgb_row(line).to_vec()
                } else {
                    let (a, b) = (self.rgb_row(2 * line), self.rgb_row(2 * line + 1));
                    a.iter()
                        .zip(b)
                        .map(|(p, q)| [avg(p[0], q[0]), avg(p[1], q[1]), avg(p[2], q[2])])
                        .collect()
                };
                let ch = |c: usize| row.iter().map(move |p| p[c]);
                if let Timing::Martin { .. } = spec.timing {
                    out.push(Seg::tone(SYNC_HZ, MARTIN_SYNC_MS));
                    out.push(Seg::tone(BLACK_HZ, MARTIN_GAP_MS));
                    for c in [1, 2, 0] {
                        Self::scan(out, ch(c), scan_ms);
                        out.push(Seg::tone(BLACK_HZ, MARTIN_GAP_MS));
                    }
                } else {
                    out.push(Seg::tone(BLACK_HZ, SCOTTIE_GAP_MS));
                    Self::scan(out, ch(1), scan_ms);
                    out.push(Seg::tone(BLACK_HZ, SCOTTIE_GAP_MS));
                    Self::scan(out, ch(2), scan_ms);
                    out.push(Seg::tone(SYNC_HZ, SCOTTIE_SYNC_MS));
                    out.push(Seg::tone(BLACK_HZ, SCOTTIE_GAP_MS));
                    Self::scan(out, ch(0), scan_ms);
                }
            }
            Timing::Robot { sync_ms, porch_ms, y_ms, sep_ms, sep_porch_ms, c_ms, alternating } => {
                let row = self.ycc_row(line);
                out.push(Seg::tone(SYNC_HZ, sync_ms));
                out.push(Seg::tone(BLACK_HZ, porch_ms));
                Self::scan(out, row.iter().map(|p| p[0]), y_ms);
                if alternating {
                    // Robot 36: even lines carry R-Y, odd lines B-Y, both averaged over
                    // the line pair since the receiver applies each to two rows.
                    let (a, b) = (self.ycc_row(line & !1), self.ycc_row((line | 1).min(spec.height - 1)));
                    let (c, sep_hz) = if line % 2 == 0 { (2, BLACK_HZ) } else { (1, WHITE_HZ) };
                    out.push(Seg::tone(sep_hz, sep_ms));
                    out.push(Seg::tone(LEADER_HZ, sep_porch_ms));
                    Self::scan(out, a.iter().zip(b).map(|(p, q)| avg(p[c], q[c])), c_ms);
                } else {
                    out.push(Seg::tone(BLACK_HZ, sep_ms));
                    out.push(Seg::tone(LEADER_HZ, sep_porch_ms));
                    Self::scan(out, row.iter().map(|p| p[2]), c_ms);
                    out.push(Seg::tone(WHITE_HZ, sep_ms));
                    out.push(Seg::tone(LEADER_HZ, sep_porch_ms));
                    Self::scan(out, row.iter().map(|p| p[1]), c_ms);
                }
            }
            Timing::RobotBw { sync_ms, y_ms } => {
                out.push(Seg::tone(SYNC_HZ, sync_ms));
                Self::scan(out, self.ycc_row(line).iter().map(|p| p[0]), y_ms);
            }
            Timing::Pd { pixel_ms } => {
                let (a, b) = (self.ycc_row(2 * line), self.ycc_row(2 * line + 1));
                let scan_ms = pixel_ms * spec.width as f64;
                out.push(Seg::tone(SYNC_HZ, PD_SYNC_MS));
                out.push(Seg::tone(BLACK_HZ, PD_PORCH_MS));
                Self::scan(out, a.iter().map(|p| p[0]), scan_ms);
                Self::scan(out, a.iter().zip(b).map(|(p, q)| avg(p[2], q[2])), scan_ms);
                Self::scan(out, a.iter().zip(b).map(|(p, q)| avg(p[1], q[1])), scan_ms);
                Self::scan(out, b.iter().map(|p| p[0]), scan_ms);
            }
        }
    }
}

pub fn vis_segments(code: u8) -> Vec<Seg> {
    let mut out = vec![
        Seg::tone(LEADER_HZ, 300.0),
        Seg::tone(SYNC_HZ, 10.0),
        Seg::tone(LEADER_HZ, 300.0),
        Seg::tone(SYNC_HZ, 30.0),
    ];
    let mut ones = 0;
    for bit in 0..7 {
        let one = (code >> bit) & 1 == 1;
        ones += one as u32;
        out.push(Seg::tone(if one { VIS_ONE_HZ } else { VIS_ZERO_HZ }, 30.0));
    }
    // Even parity over the 7 data bits.
    out.push(Seg::tone(if ones % 2 == 1 { VIS_ONE_HZ } else { VIS_ZERO_HZ }, 30.0));
    out.push(Seg::tone(SYNC_HZ, 30.0));
    out
}

const VOX_PREAMBLE: [f32; 8] = [1900.0, 1500.0, 1900.0, 1500.0, 2300.0, 1500.0, 2300.0, 1500.0];
const ID_GAP_MS: f64 = 250.0;

/// The 800 ms VOX burst: eight 100 ms tones. It sits in front of a whole transmission,
/// not in front of each picture.
pub fn vox_segments() -> Vec<Seg> {
    VOX_PREAMBLE.iter().copied().map(|hz| Seg::tone(hz, 100.0)).collect()
}

pub const VOX_MS: f64 = 800.0;

/// VIS header, plus the 9 ms sync Scottie receivers expect glued to it.
pub fn sstv_header(mode: Mode) -> Vec<Seg> {
    let mut head = vis_segments(mode.spec().vis);
    if mode.spec().family == Family::Scottie {
        head.push(Seg::tone(SYNC_HZ, SCOTTIE_SYNC_MS));
    }
    head
}

/// A complete transmission: optional preamble, VIS, picture and identification.
#[derive(Clone)]
pub struct TxPlan {
    image: Arc<ScanImage>,
    head: Arc<Vec<Seg>>,
    tail: Arc<Vec<Seg>>,
}

impl TxPlan {
    pub fn new(image: ScanImage, opts: &TxOptions) -> TxPlan {
        let spec = image.spec;
        let mut head = Vec::new();
        if opts.vox_preamble {
            head.extend(VOX_PREAMBLE.iter().map(|&hz| Seg::tone(hz, 100.0)));
        }
        if let Some(cw) = opts.cw_preamble.as_ref().filter(|c| !c.text.trim().is_empty()) {
            head.extend(cw_segments(&cw.text, cw.wpm, cw.tone_hz));
            head.push(Seg::silence(ID_GAP_MS));
        }
        head.extend(sstv_header(spec.mode));
        let mut tail = Vec::new();
        if let Some(cw) = opts.cw_id.as_ref().filter(|c| !c.text.trim().is_empty()) {
            tail.push(Seg::silence(ID_GAP_MS));
            tail.extend(cw_segments(&cw.text, cw.wpm, cw.tone_hz));
        }
        TxPlan { image: Arc::new(image), head: Arc::new(head), tail: Arc::new(tail) }
    }

    pub fn spec(&self) -> &'static ModeSpec {
        self.image.spec
    }

    pub fn segments(&self) -> TxSegments {
        TxSegments { plan: self.clone(), stage: Stage::Head(0), line: 0, buf: Vec::new(), pos: 0 }
    }

    pub fn head_ms(&self) -> f64 {
        self.head.iter().map(|s| s.ms).sum()
    }

    pub fn total_ms(&self) -> f64 {
        self.head_ms() + self.spec().tx_lines as f64 * self.spec().line_ms() + self.tail.iter().map(|s| s.ms).sum::<f64>()
    }

    /// Fraction of the picture already sent after `elapsed_ms` of transmission (0..=1).
    pub fn picture_progress(&self, elapsed_ms: f64) -> f64 {
        let spec = self.spec();
        ((elapsed_ms - self.head_ms()) / (spec.tx_lines as f64 * spec.line_ms())).clamp(0.0, 1.0)
    }

    /// Ready-to-play audio at effective sample rate `fs`.
    pub fn synth(&self, fs: f64, amp: f32) -> Synth<TxSegments> {
        Synth::new(self.segments(), fs, amp, self.total_ms())
    }
}

enum Stage {
    Head(usize),
    Lines,
    Tail(usize),
    Done,
}

pub struct TxSegments {
    plan: TxPlan,
    stage: Stage,
    line: u32,
    buf: Vec<Seg>,
    pos: usize,
}

impl Iterator for TxSegments {
    type Item = Seg;

    fn next(&mut self) -> Option<Seg> {
        loop {
            match self.stage {
                Stage::Head(i) => {
                    if let Some(&s) = self.plan.head.get(i) {
                        self.stage = Stage::Head(i + 1);
                        return Some(s);
                    }
                    self.stage = Stage::Lines;
                }
                Stage::Lines => {
                    if self.pos < self.buf.len() {
                        self.pos += 1;
                        return Some(self.buf[self.pos - 1]);
                    }
                    if self.line >= self.plan.spec().tx_lines {
                        self.stage = Stage::Tail(0);
                        continue;
                    }
                    self.buf.clear();
                    self.pos = 0;
                    self.plan.image.line(self.line, &mut self.buf);
                    self.line += 1;
                }
                Stage::Tail(i) => {
                    if let Some(&s) = self.plan.tail.get(i) {
                        self.stage = Stage::Tail(i + 1);
                        return Some(s);
                    }
                    self.stage = Stage::Done;
                }
                Stage::Done => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::SampleSource;

    fn test_image(mode: Mode) -> RgbImage {
        let s = mode.spec();
        RgbImage::from_fn(s.width, s.height, |x, y| {
            image::Rgb([(x * 255 / s.width) as u8, (y * 255 / s.height) as u8, 128])
        })
    }

    #[test]
    fn segment_sum_matches_mode_duration() {
        for mode in Mode::ALL {
            let plan = TxPlan::new(ScanImage::new(mode, &test_image(mode), ColorRange::Studio).unwrap(), &TxOptions::default());
            let sum: f64 = plan.segments().map(|s| s.ms).sum();
            assert!((sum - mode.spec().total_ms()).abs() < 1e-3, "{mode:?}: {sum} vs {}", mode.spec().total_ms());
            assert!((sum - plan.total_ms()).abs() < 1e-3);
        }
    }

    #[test]
    fn cw_preamble_adds_time_before_vis() {
        let mode = Mode::MartinM1;
        let img = test_image(mode);
        let base = TxPlan::new(ScanImage::new(mode, &img, ColorRange::Full).unwrap(), &TxOptions::default());
        let with = TxPlan::new(
            ScanImage::new(mode, &img, ColorRange::Full).unwrap(),
            &TxOptions {
                cw_preamble: Some(CwIdent { text: "DE TEST".into(), wpm: 20.0, tone_hz: 800.0 }),
                ..Default::default()
            },
        );
        assert!(with.total_ms() > base.total_ms() + 100.0, "{} vs {}", with.total_ms(), base.total_ms());
        assert!(with.head_ms() > base.head_ms());
    }

    #[test]
    fn vis_parity_is_even() {
        for mode in Mode::ALL {
            let segs = vis_segments(mode.spec().vis);
            let ones = segs[4..12].iter().filter(|s| s.hz == VIS_ONE_HZ).count();
            assert_eq!(ones % 2, 0, "{mode:?}");
        }
    }

    /// Demodulates the generated audio with a quadrature mixer at known positions and
    /// checks that pixel frequencies come back: proves timing and tone mapping agree.
    #[test]
    fn martin_m1_roundtrip_frequencies() {
        let mode = Mode::MartinM1;
        let img = RgbImage::from_fn(320, 256, |x, _| if x < 160 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) });
        let plan = TxPlan::new(ScanImage::new(mode, &img, ColorRange::Studio).unwrap(), &TxOptions::default());
        let fs = 48_000.0;
        let mut synth = plan.synth(fs, 1.0);
        let mut audio = vec![0.0f32; synth.total_samples() as usize];
        let n = synth.fill(&mut audio);
        assert_eq!(n, audio.len());

        // Estimate the instantaneous frequency around time t (ms) by zero-crossing count.
        let freq_at = |t_ms: f64, win_ms: f64| -> f64 {
            let a = ((t_ms - win_ms / 2.0) * fs / 1000.0) as usize;
            let b = ((t_ms + win_ms / 2.0) * fs / 1000.0) as usize;
            let crossings = audio[a..b].windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
            crossings as f64 / 2.0 / ((b - a) as f64 / fs)
        };
        let line0 = VIS_HEADER_MS + 100.0 * mode.spec().line_ms();
        // Red channel scan starts after sync, porch, G, sep, B, sep.
        let red_start = line0 + MARTIN_SYNC_MS + MARTIN_GAP_MS + 2.0 * (146.432 + MARTIN_GAP_MS);
        let f_left = freq_at(red_start + 146.432 * 0.25, 20.0);
        let f_right = freq_at(red_start + 146.432 * 0.75, 20.0);
        assert!((f_left - 2300.0).abs() < 30.0, "left red {f_left}");
        assert!((f_right - 1500.0).abs() < 30.0, "right red {f_right}");
        // Sync pulse at the start of the line.
        let f_sync = freq_at(line0 + 2.4, 4.0);
        assert!((f_sync - 1200.0).abs() < 130.0, "sync {f_sync}");
    }
}
