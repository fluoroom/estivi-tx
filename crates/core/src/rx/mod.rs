//! Listening: CW and DTMF decoding of the receiver audio, used for remote triggers.

pub mod cw;
pub mod dtmf;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, bounded};

use crate::config::AudioConfig;
use crate::project::Listen;
use cw::{CwDecoder, CwEvent};
use dtmf::DtmfDecoder;

/// A DTMF sequence ends with `#` or after this much silence.
const DTMF_SEQ_TIMEOUT: Duration = Duration::from_millis(2500);
const TEXT_KEEP: usize = 120;

#[derive(Clone, Debug, Default)]
pub struct ListenStatus {
    pub running: bool,
    pub error: Option<String>,
    /// Input level in dBFS (RMS).
    pub level_db: f32,
    pub cw_text: String,
    pub dtmf_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heard {
    /// A complete DTMF sequence.
    Dtmf(String),
    /// The CW text received so far (each time a word completes).
    CwWord(String),
}

pub struct Listener {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn keep_tail(s: &mut String) {
    let n = s.chars().count();
    if n > TEXT_KEEP {
        *s = s.chars().skip(n - TEXT_KEEP).collect();
    }
}

impl Listener {
    /// Starts decoding. Decoding pauses while `muted` is set (our own transmission).
    pub fn start(audio: AudioConfig, listen: Listen, muted: Arc<AtomicBool>, status: Arc<Mutex<ListenStatus>>, heard: Sender<Heard>) -> Listener {
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::Builder::new()
            .name("escucha".into())
            .spawn(move || run(audio, listen, muted, status, heard, stop2))
            .expect("spawn listener");
        Listener { stop, handle: Some(handle) }
    }
}

fn run(audio: AudioConfig, listen: Listen, muted: Arc<AtomicBool>, status: Arc<Mutex<ListenStatus>>, heard: Sender<Heard>, stop: Arc<AtomicBool>) {
    let (tx, rx) = bounded::<Vec<f32>>(64);
    let capture = crate::audio::capture(&audio, move |block| {
        let _ = tx.try_send(block.to_vec());
    });
    let capture = match capture {
        Ok(c) => c,
        Err(e) => {
            let mut st = status.lock().unwrap();
            st.running = false;
            st.error = Some(format!("{e:#}"));
            return;
        }
    };
    let fs = capture.sample_rate;
    {
        let mut st = status.lock().unwrap();
        st.running = true;
        st.error = None;
    }
    let mut cw = listen.cw.then(|| CwDecoder::new(fs, listen.cw_wpm, listen.cw_tone_hz));
    let mut dtmf = listen.dtmf.then(|| DtmfDecoder::new(fs));
    let mut seq = String::new();
    let mut last_digit = Instant::now();
    let mut cw_text = String::new();
    let mut level_acc = (0.0f64, 0usize);

    while !stop.load(Ordering::Relaxed) {
        let Ok(block) = rx.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        if muted.load(Ordering::Relaxed) {
            cw.as_mut().map(CwDecoder::reset);
            dtmf.as_mut().map(DtmfDecoder::reset);
            continue;
        }
        level_acc.0 += block.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>();
        level_acc.1 += block.len();

        if let Some(dec) = dtmf.as_mut() {
            let mut done = false;
            dec.process(&block, |d| {
                seq.push(d);
                last_digit = Instant::now();
                done |= d == '#';
            });
            let timed_out = !seq.is_empty() && last_digit.elapsed() > DTMF_SEQ_TIMEOUT;
            if done || timed_out {
                let s = std::mem::take(&mut seq);
                let mut st = status.lock().unwrap();
                st.dtmf_text = s.clone();
                drop(st);
                let _ = heard.send(Heard::Dtmf(s));
            } else if !seq.is_empty() {
                status.lock().unwrap().dtmf_text = format!("{seq}…");
            }
        }

        if let Some(dec) = cw.as_mut() {
            let mut word_done = false;
            dec.process(&block, |e| match e {
                CwEvent::Char(c) => cw_text.push(c),
                CwEvent::Space => {
                    cw_text.push(' ');
                    word_done = true;
                }
            });
            keep_tail(&mut cw_text);
            status.lock().unwrap().cw_text = cw_text.clone();
            if word_done {
                let _ = heard.send(Heard::CwWord(cw_text.trim().to_string()));
            }
        }

        // Publish the level ~5 times per second.
        if level_acc.1 >= (fs / 5) as usize {
            let rms = (level_acc.0 / level_acc.1 as f64).sqrt();
            status.lock().unwrap().level_db = (20.0 * rms.max(1e-6).log10()) as f32;
            level_acc = (0.0, 0);
        }
    }
    status.lock().unwrap().running = false;
}

/// Does a heard DTMF sequence match a trigger code? A trailing `#` is optional on both.
pub fn dtmf_matches(heard: &str, code: &str) -> bool {
    let norm = |s: &str| s.trim().trim_end_matches('#').to_ascii_uppercase();
    !norm(code).is_empty() && norm(heard) == norm(code)
}

/// Does the CW text received so far end with the trigger words?
pub fn cw_matches(text: &str, code: &str) -> bool {
    let words = |s: &str| s.split_whitespace().map(str::to_ascii_uppercase).collect::<Vec<_>>();
    let (t, c) = (words(text), words(code));
    !c.is_empty() && t.ends_with(&c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching() {
        assert!(dtmf_matches("*12#", "*12"));
        assert!(dtmf_matches("*12", "*12#"));
        assert!(!dtmf_matches("*123#", "*12#"));
        assert!(cw_matches("CQ DE LU2XYZ QRV SSTV", "qrv sstv"));
        assert!(!cw_matches("QRV SSTVX", "qrv sstv"));
    }
}
