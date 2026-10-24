//! The playout engine: runs the grid, keys the radio, plays the audio, logs what aired
//! and reacts to DTMF/CW triggers. Lives on its own thread; the UI talks to it with
//! `Command`s and reads a `Status` snapshot.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use image::RgbaImage;

use crate::aslog::{self, LogEntry};
use crate::compose::{Composer, data_ctx};
use crate::config::{Config, PttMethod};
use crate::dynamic::{ApiCache, expand};
use crate::encoder::{ScanImage, TxPlan};
use crate::modes::Mode;
use crate::project::{Project, TriggerAction, TriggerSource};
use crate::ptt::{self, Ptt};
use crate::rx::{Heard, ListenStatus, Listener, cw_matches, dtmf_matches};
use crate::schedule::{ImageRotator, Rotator, next_project_occurrence};
use crate::synth::{Seg, SampleSource, Synth};

pub enum Command {
    SetProject { project: Box<Project>, path: Option<PathBuf> },
    SetConfig(Box<Config>),
    /// Grid on air / paused.
    SetAgenda(bool),
    /// Transmit right away (a program, or the next of the rotation).
    TransmitNow { program: Option<u64> },
    Stop,
    /// 5 s of 1900 Hz with PTT, to set levels.
    TestTone,
    /// Key the radio for 2 s without audio.
    TestPtt,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Keying,
    OnAir,
    Tail,
}

#[derive(Clone)]
pub struct OnAir {
    pub program: String,
    pub origin: String,
    pub mode: Mode,
    pub serial: u64,
    pub started: DateTime<Utc>,
    pub total_ms: f64,
    pub elapsed_ms: f64,
    /// Fraction of the picture already sent.
    pub picture: f64,
    pub image: Option<Arc<RgbaImage>>,
    pub done: bool,
}

#[derive(Clone, Debug)]
pub struct NextUp {
    pub at: DateTime<Utc>,
    /// When it will really start (after the minimum pause).
    pub due: DateTime<Utc>,
    pub slot: String,
    pub program: Option<u64>,
    pub program_name: String,
    pub freq_hz: u64,
}

#[derive(Clone, Default)]
pub struct Status {
    pub phase: Phase,
    pub agenda: bool,
    pub on_air: Option<OnAir>,
    /// Bumped whenever `on_air.image` changes.
    pub on_air_rev: u64,
    pub next: Option<NextUp>,
    pub serial: u64,
    pub error: Option<String>,
    pub log: Vec<LogEntry>,
    pub log_rev: u64,
    pub listen: ListenStatus,
    pub last_cw: String,
    pub last_dtmf: String,
    /// A triggered transmission waiting for the pause to end.
    pub pending: Option<String>,
}

pub struct Engine {
    tx: Sender<Command>,
    status: Arc<Mutex<Status>>,
    listen: Arc<Mutex<ListenStatus>>,
    handle: Option<JoinHandle<()>>,
}

impl Engine {
    /// `fallback_log` is used while the project has never been saved.
    pub fn start(config: Config, fallback_log: PathBuf, api: ApiCache) -> Engine {
        let (tx, cmd_rx) = unbounded();
        let (heard_tx, heard_rx) = unbounded();
        let status = Arc::new(Mutex::new(Status { agenda: config.ui.autostart, ..Default::default() }));
        let listen = Arc::new(Mutex::new(ListenStatus::default()));
        let mut worker = Worker {
            cmd_rx,
            heard_rx,
            heard_tx,
            status: status.clone(),
            listen_status: listen.clone(),
            agenda: config.ui.autostart,
            project: Project::default(),
            path: None,
            config,
            fallback_log,
            composer: Composer::new(api),
            rotator: Rotator::new(),
            image_rotators: std::collections::HashMap::new(),
            ptt: None,
            listener: None,
            muted: Arc::new(AtomicBool::new(false)),
            last_end: None,
            cursor: Utc::now(),
            next: None,
            next_checked: Instant::now(),
            pending: None,
            deferred: VecDeque::new(),
            serial: 0,
            last_cw: String::new(),
            last_dtmf: String::new(),
            log: Vec::new(),
        };
        let handle = std::thread::Builder::new()
            .name("motor".into())
            .spawn(move || worker.run())
            .expect("spawn engine");
        Engine { tx, status, listen, handle: Some(handle) }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn status(&self) -> Status {
        let mut s = self.status.lock().unwrap().clone();
        s.listen = self.listen.lock().unwrap().clone();
        s
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

enum Outcome {
    Completed,
    Cancelled,
}

/// Transmissions that drift this far past their time are skipped, not sent late.
const MAX_LATE: chrono::TimeDelta = chrono::TimeDelta::seconds(90);

#[derive(Debug, PartialEq, Eq)]
enum Due {
    Wait,
    Go,
    /// Too late (the PC was asleep, a long transmission ran over…): drop it.
    Skip,
}

/// Real start of a grid transmission: its time, or later if the pause is not over.
fn due_time(at: DateTime<Utc>, pause_until: Option<DateTime<Utc>>) -> DateTime<Utc> {
    pause_until.map_or(at, |p| p.max(at))
}

fn decide(now: DateTime<Utc>, due: DateTime<Utc>) -> Due {
    if now < due {
        Due::Wait
    } else if now - due > MAX_LATE {
        Due::Skip
    } else {
        Due::Go
    }
}
const LOG_KEEP: usize = 200;

struct Worker {
    cmd_rx: Receiver<Command>,
    heard_rx: Receiver<Heard>,
    heard_tx: Sender<Heard>,
    status: Arc<Mutex<Status>>,
    listen_status: Arc<Mutex<ListenStatus>>,
    agenda: bool,
    project: Project,
    path: Option<PathBuf>,
    config: Config,
    fallback_log: PathBuf,
    composer: Composer,
    rotator: Rotator,
    image_rotators: std::collections::HashMap<u64, ImageRotator>,
    ptt: Option<Ptt>,
    listener: Option<Listener>,
    muted: Arc<AtomicBool>,
    last_end: Option<DateTime<Utc>>,
    /// The grid is searched from here on.
    cursor: DateTime<Utc>,
    next: Option<NextUp>,
    next_checked: Instant,
    pending: Option<(Option<u64>, String)>,
    deferred: VecDeque<Command>,
    serial: u64,
    last_cw: String,
    last_dtmf: String,
    log: Vec<LogEntry>,
}

impl Worker {
    fn run(&mut self) {
        self.restart_listener();
        loop {
            let cmd = match self.deferred.pop_front() {
                Some(c) => Some(c),
                None => select! {
                    recv(self.cmd_rx) -> c => c.ok(),
                    recv(self.heard_rx) -> h => {
                        if let Ok(h) = h { self.on_heard(h); }
                        None
                    },
                    default(Duration::from_millis(200)) => None,
                },
            };
            if let Some(cmd) = cmd {
                if !self.handle(cmd) {
                    break;
                }
            }
            self.tick();
        }
        self.listener = None;
        self.ptt = None; // Drop releases a keyed PTT.
    }

    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
    }

    fn log_path(&self) -> PathBuf {
        self.path.as_deref().map(aslog::path_for).unwrap_or_else(|| self.fallback_log.clone())
    }

    fn handle(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Shutdown => return false,
            Command::SetProject { project, path } => {
                let listen_changed = project.listen != self.project.listen;
                let path_changed = path != self.path;
                self.project = *project;
                self.path = path;
                self.composer.set_project_dir(self.path.as_deref().and_then(|p| p.parent()));
                if path_changed {
                    self.log = aslog::tail(&self.log_path(), LOG_KEEP);
                    self.serial = self.log.last().map_or(0, |e| e.serial);
                    let (log, serial) = (self.log.clone(), self.serial);
                    self.set_status(|s| {
                        s.log = log;
                        s.log_rev += 1;
                        s.serial = serial;
                    });
                }
                if listen_changed {
                    self.restart_listener();
                }
                self.cursor = self.cursor.min(Utc::now());
                self.next = None;
            }
            Command::SetConfig(config) => {
                let audio_changed = config.audio != self.config.audio;
                let ptt_changed = config.ptt != self.config.ptt || config.cat != self.config.cat;
                self.config = *config;
                if ptt_changed {
                    self.ptt = None;
                }
                if audio_changed {
                    self.restart_listener();
                }
            }
            Command::SetAgenda(on) => {
                self.agenda = on;
                self.cursor = Utc::now();
                self.next = None;
            }
            Command::TransmitNow { program } => self.transmit(program, "manual".into(), 0),
            Command::Stop => self.pending = None,
            Command::TestTone => self.test(true),
            Command::TestPtt => self.test(false),
        }
        true
    }

    fn restart_listener(&mut self) {
        self.listener = None;
        let l = &self.project.listen;
        if l.cw || l.dtmf {
            self.listener = Some(Listener::start(
                self.config.audio.clone(),
                l.clone(),
                self.muted.clone(),
                self.listen_status.clone(),
                self.heard_tx.clone(),
            ));
        } else {
            *self.listen_status.lock().unwrap() = ListenStatus::default();
        }
    }

    fn on_heard(&mut self, heard: Heard) {
        let (source, text) = match &heard {
            Heard::Dtmf(seq) => {
                self.last_dtmf = seq.trim_end_matches('#').to_string();
                (TriggerSource::Dtmf, seq.clone())
            }
            Heard::CwWord(text) => {
                self.last_cw = text.clone();
                (TriggerSource::Cw, text.clone())
            }
        };
        let (cw, dtmf) = (self.last_cw.clone(), self.last_dtmf.clone());
        self.set_status(|s| {
            s.last_cw = cw;
            s.last_dtmf = dtmf;
        });
        let mut triggers = self.project.listen.triggers.clone();
        for p in &self.project.programs {
            for mut t in p.activations.clone() {
                if t.action == TriggerAction::Transmit {
                    t.program = Some(p.id);
                }
                triggers.push(t);
            }
        }
        let fired: Vec<_> = triggers
            .into_iter()
            .filter(|t| t.enabled && t.source == source)
            .filter(|t| match source {
                TriggerSource::Dtmf => dtmf_matches(&text, &t.code),
                TriggerSource::Cw => cw_matches(&text, &t.code),
            })
            .collect();
        for t in fired {
            let origin = format!("{} {}", if source == TriggerSource::Dtmf { "DTMF" } else { "CW" }, t.code.trim());
            log::info!("disparador: {origin}");
            match t.action {
                TriggerAction::Transmit => self.pending = Some((t.program, origin)),
                TriggerAction::Pause => self.agenda = false,
                TriggerAction::Resume => {
                    self.agenda = true;
                    self.cursor = Utc::now();
                    self.next = None;
                }
            }
        }
    }

    fn pause_until(&self) -> Option<DateTime<Utc>> {
        self.last_end.map(|t| t + chrono::TimeDelta::seconds(self.project.tx.min_pause_s as i64))
    }

    fn compute_next(&mut self) {
        let pause_until = self.pause_until();
        self.next = next_project_occurrence(&self.project, self.cursor).map(|o| {
            let program = o
                .program
                .filter(|id| self.project.program(*id).is_some())
                .or_else(|| self.rotator.peek(&self.project.programs, self.project.rotation));
            let program_name = program.and_then(|id| self.project.program(id)).map_or_else(|| "(sin programas)".into(), |p| p.name.clone());
            NextUp {
                at: o.at,
                due: due_time(o.at, pause_until),
                slot: o.slot_name,
                program,
                program_name,
                freq_hz: o.freq_hz,
            }
        });
        self.next_checked = Instant::now();
    }

    fn tick(&mut self) {
        // The UI may not have sent a project yet; the default one has an empty grid.
        let api_sources: Vec<_> = self.project.graphics.api_sources().cloned().collect();
        self.composer.api.refresh_due(&api_sources);

        if self.next.is_none() || self.next_checked.elapsed() > Duration::from_secs(30) {
            self.compute_next();
        }
        let now = Utc::now();
        let pause_over = self.pause_until().is_none_or(|p| now >= p);

        if let Some((program, origin)) = self.pending.clone() {
            if pause_over {
                self.pending = None;
                self.transmit(program, origin, 0);
                return;
            }
        }

        if self.agenda {
            if let Some(n) = self.next.clone() {
                match decide(now, n.due) {
                    Due::Wait => {}
                    Due::Skip => {
                        log::warn!("emisión de las {} salteada (llegó tarde)", n.at.format("%H:%M"));
                        self.cursor = now;
                        self.compute_next();
                    }
                    Due::Go => {
                        self.cursor = n.at + chrono::TimeDelta::seconds(1);
                        self.transmit(n.program, format!("grilla: {}", n.slot), n.freq_hz);
                        self.compute_next();
                    }
                }
            }
        }

        let (agenda, next, pending) = (self.agenda, self.next.clone(), self.pending.as_ref().map(|p| p.1.clone()));
        self.set_status(|s| {
            s.agenda = agenda;
            s.next = next;
            s.pending = pending;
        });
    }

    fn ensure_ptt(&mut self) -> anyhow::Result<()> {
        if self.ptt.is_none() {
            self.ptt = Some(Ptt::open(&self.config)?);
        }
        Ok(())
    }

    /// Waits up to `ms`, returning false if the operator pressed Stop.
    fn wait(&mut self, ms: u64) -> bool {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            if self.stop_requested() {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10).min(end.saturating_duration_since(Instant::now())));
        }
        true
    }

    fn stop_requested(&mut self) -> bool {
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                Command::Stop => return true,
                Command::Shutdown => {
                    self.deferred.push_back(Command::Shutdown);
                    return true;
                }
                Command::SetAgenda(on) => self.agenda = on,
                other => self.deferred.push_back(other),
            }
        }
        false
    }

    /// Keys, plays and unkeys. `on_progress` receives elapsed ms.
    fn air(&mut self, freq_hz: u64, total_ms: f64, make: impl FnOnce(u32) -> Box<dyn SampleSource>, mut on_progress: impl FnMut(&mut Self, f64)) -> anyhow::Result<Outcome> {
        self.ensure_ptt()?;
        self.muted.store(true, Ordering::Relaxed);
        self.set_status(|s| s.phase = Phase::Keying);
        let result = self.air_inner(freq_hz, total_ms, make, &mut on_progress);
        // Always release, whatever happened.
        if let Some(p) = self.ptt.as_mut() {
            if let Err(e) = p.key(false) {
                log::error!("no se pudo soltar el PTT: {e}");
                self.ptt = None;
            }
        }
        self.muted.store(false, Ordering::Relaxed);
        self.set_status(|s| s.phase = Phase::Idle);
        result
    }

    fn air_inner(&mut self, freq_hz: u64, total_ms: f64, make: impl FnOnce(u32) -> Box<dyn SampleSource>, on_progress: &mut impl FnMut(&mut Self, f64)) -> anyhow::Result<Outcome> {
        if self.config.ptt.method == PttMethod::Cat && self.config.cat.set_freq && freq_hz > 0 {
            if let Err(e) = ptt::set_frequency(&self.config.cat, freq_hz) {
                log::warn!("no se pudo cambiar la frecuencia: {e}");
            }
        }
        self.ptt.as_mut().expect("ensure_ptt").key(true)?;
        if !self.wait(self.config.ptt.lead_ms as u64) {
            return Ok(Outcome::Cancelled);
        }
        let playback = crate::audio::play(&self.config.audio, make)?;
        self.set_status(|s| s.phase = Phase::OnAir);
        let rate = playback.sample_rate as f64;
        let started = Instant::now();
        let timeout = Duration::from_secs(self.config.ptt.timeout_s.max(10) as u64);
        let mut finished_at: Option<Instant> = None;
        loop {
            if self.stop_requested() {
                return Ok(Outcome::Cancelled);
            }
            if started.elapsed() > timeout {
                anyhow::bail!("se cortó por superar el tiempo máximo de transmisión ({} s)", timeout.as_secs());
            }
            let elapsed_ms = (playback.played_samples() as f64 / rate * 1000.0).min(total_ms);
            on_progress(self, elapsed_ms);
            if playback.finished() {
                // Let the device play out what is still in its buffer.
                let t = *finished_at.get_or_insert_with(Instant::now);
                if t.elapsed() > Duration::from_millis(250) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        drop(playback);
        self.set_status(|s| s.phase = Phase::Tail);
        self.wait(self.config.ptt.tail_ms as u64);
        Ok(Outcome::Completed)
    }

    /// Sound card clock correction: synthesize as if the card ran at its true rate.
    fn ppm_factor(&self) -> f64 {
        1.0 + self.config.audio.tx_ppm / 1e6
    }

    fn transmit(&mut self, program: Option<u64>, origin: String, freq_hz: u64) {
        let project = &self.project;
        let id = program
            .filter(|id| project.program(*id).is_some())
            .or_else(|| self.rotator.next(&project.programs, project.rotation));
        let Some(prog) = id.and_then(|id| project.program(id)).cloned() else {
            self.set_status(|s| s.error = Some("No hay programas habilitados para transmitir".into()));
            return;
        };
        self.rotator.aired(prog.id);
        let image_idx = self.image_rotators.entry(prog.id).or_default().next(&prog.images, prog.rotation);
        let Some(image_idx) = image_idx else {
            self.set_status(|s| s.error = Some(format!("«{}» no tiene imágenes habilitadas", prog.name)));
            return;
        };
        let image = &prog.images[image_idx];
        let image_path = image.path.clone();
        let image_label = std::path::Path::new(&image_path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let serial = self.serial + 1;
        let started = Utc::now();
        let on_air_name = if image_label.is_empty() || image_label == prog.name {
            prog.name.clone()
        } else {
            format!("{} · {}", prog.name, image_label)
        };
        let mode = project.mode_of(Some(&prog), Some(image));
        let ctx = data_ctx(project, started, serial, &on_air_name, freq_hz, &self.last_cw, &self.last_dtmf, mode);
        let dir = self.path.as_deref().and_then(|p| p.parent()).map(|p| p.to_path_buf());
        let frame = self.composer.frame_image(&self.project, dir.as_deref(), Some(&image_path), mode, &ctx);
        let rgb = image::DynamicImage::ImageRgba8(frame.clone()).to_rgb8();
        let scan = match ScanImage::new(mode, &rgb, self.project.tx.color_range) {
            Ok(s) => s,
            Err(e) => {
                self.set_status(|s| s.error = Some(e.to_string()));
                return;
            }
        };
        let opts = prog.tx_options(&self.project, |t| expand(t, &ctx, None));
        let plan = TxPlan::new(scan, &opts);
        let total_ms = plan.total_ms();
        let image = Arc::new(frame);
        self.set_status(|s| {
            s.on_air = Some(OnAir {
                program: on_air_name.clone(),
                origin: origin.clone(),
                mode,
                serial,
                started,
                total_ms,
                elapsed_ms: 0.0,
                picture: 0.0,
                image: Some(image),
                done: false,
            });
            s.on_air_rev += 1;
            s.error = None;
        });

        let plan2 = plan.clone();
        let k = self.ppm_factor();
        let result = self.air(
            freq_hz,
            total_ms,
            move |rate| Box::new(plan2.synth(rate as f64 * k, 1.0)),
            |w, elapsed| {
                let picture = plan.picture_progress(elapsed);
                w.set_status(|s| {
                    if let Some(a) = s.on_air.as_mut() {
                        a.elapsed_ms = elapsed;
                        a.picture = picture;
                    }
                });
            },
        );
        let ended = Utc::now();
        self.serial = serial;
        self.last_end = Some(ended);
        let result_text = match &result {
            Ok(Outcome::Completed) => "OK".to_string(),
            Ok(Outcome::Cancelled) => "Cortada".to_string(),
            Err(e) => format!("Error: {e:#}"),
        };
        let entry = LogEntry {
            start: started,
            end: ended,
            serial,
            mode: mode.name().into(),
            program: on_air_name.clone(),
            freq_hz,
            origin,
            result: result_text,
        };
        if let Err(e) = aslog::append(&self.log_path(), &entry) {
            log::error!("registro: {e}");
        }
        self.log.push(entry);
        if self.log.len() > LOG_KEEP {
            self.log.remove(0);
        }
        let log = self.log.clone();
        let err = result.err().map(|e| format!("{e:#}"));
        self.set_status(|s| {
            s.serial = serial;
            s.log = log;
            s.log_rev += 1;
            if let Some(a) = s.on_air.as_mut() {
                a.done = true;
            }
            if err.is_some() {
                s.error = err;
            }
        });
        self.next = None;
    }

    fn test(&mut self, tone: bool) {
        let k = self.ppm_factor();
        let (seg, total_ms) = if tone { (Seg::tone(1900.0, 5000.0), 5000.0) } else { (Seg::silence(2000.0), 2000.0) };
        let result = self.air(0, total_ms, move |rate| Box::new(Synth::new(std::iter::once(seg), rate as f64 * k, 1.0, total_ms)), |_, _| {});
        if let Err(e) = result {
            self.set_status(|s| s.error = Some(format!("Prueba: {e:#}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn pause_delays_the_start() {
        let at = t("2026-10-23T21:00:00Z");
        assert_eq!(due_time(at, None), at);
        assert_eq!(due_time(at, Some(t("2026-10-23T20:59:00Z"))), at);
        assert_eq!(due_time(at, Some(t("2026-10-23T21:00:40Z"))), t("2026-10-23T21:00:40Z"));
    }

    #[test]
    fn late_transmissions_are_skipped() {
        let due = t("2026-10-23T21:00:00Z");
        assert_eq!(decide(t("2026-10-23T20:59:59Z"), due), Due::Wait);
        assert_eq!(decide(due, due), Due::Go);
        assert_eq!(decide(t("2026-10-23T21:01:30Z"), due), Due::Go);
        assert_eq!(decide(t("2026-10-23T21:01:31Z"), due), Due::Skip);
    }
}
