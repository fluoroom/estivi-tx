//! The playout engine. One PTT at a time: the grid, the instant tab, or one activation.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeDelta, Utc};
use crossbeam_channel::{Receiver, Sender, select, unbounded};
use image::RgbaImage;

use crate::air::{self, GraphicsLevel, PieceKind, Turn, due};
use crate::aslog::{self, LogEntry};
use crate::compose::{Composer, LiveText, data_ctx};
use crate::config::{Config, Lang, PttMethod};
use crate::dynamic::{ApiCache, expand};
use crate::encoder::{ScanImage, sstv_header};
use crate::modes::Mode;
use crate::project::{Action, Activation, Carrier, Project, TriggerSource};
use crate::ptt::{self, Ptt};
use crate::rx::{Heard, ListenStatus, Listener, cw_matches, dtmf_matches};
use crate::schedule::{grid_date, occurrences};
use crate::shell;
use crate::synth::{Seg, Synth};

pub enum Command {
    SetProject { project: Box<Project>, path: Option<PathBuf> },
    SetConfig(Box<Config>),
    SetAgenda(bool),
    /// Instant tab. Refused while any transmission holds the PTT.
    TransmitManual(Box<air::Manual>),
    Stop,
    TestTone,
    TestPtt,
    Shutdown,
    /// NTP thread → engine. Not used by the window.
    Clock(Result<i64, String>),
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
    pub picture: f64,
    /// Milliseconds into the turn audio when the picture scan starts, and how long it lasts.
    pub scan_at: f64,
    pub scan_ms: f64,
    pub image: Option<Arc<RgbaImage>>,
    pub done: bool,
}

#[derive(Clone, Debug)]
pub struct NextUp {
    pub at: DateTime<Utc>,
    pub due: DateTime<Utc>,
    pub name: String,
    pub program_id: Option<u64>,
    pub program_name: String,
    pub block_id: Option<u64>,
    pub freq_hz: u64,
    pub mode: Mode,
}

#[derive(Clone, Default)]
pub struct Status {
    pub phase: Phase,
    pub agenda: bool,
    pub on_air: Option<OnAir>,
    pub on_air_rev: u64,
    pub next: Option<NextUp>,
    pub serial: u64,
    pub error: Option<String>,
    pub log: Vec<LogEntry>,
    pub log_rev: u64,
    pub listen: ListenStatus,
    pub last_cw: String,
    pub last_dtmf: String,
    pub pending: Option<String>,
    /// Something holds the PTT.
    pub busy: bool,
    /// The thing holding it is a grid turn, so the instant tab must stay quiet.
    pub busy_grid: bool,
    pub busy_reason: String,
    pub clock_offset_ms: i64,
    pub clock_error: String,
}

pub struct Engine {
    tx: Sender<Command>,
    status: Arc<Mutex<Status>>,
    listen: Arc<Mutex<ListenStatus>>,
    handle: Option<JoinHandle<()>>,
}

impl Engine {
    /// `config_path` is where a new NTP offset is saved. `fallback_log` is used until the project has a path.
    pub fn start(config: Config, config_path: PathBuf, fallback_log: PathBuf, api: ApiCache) -> Engine {
        let (tx, cmd_rx) = unbounded();
        let (heard_tx, heard_rx) = unbounded();
        let status = Arc::new(Mutex::new(Status { agenda: config.ui.autostart, ..Default::default() }));
        let listen = Arc::new(Mutex::new(ListenStatus::default()));
        let mut worker = Worker {
            cmd_tx: tx.clone(),
            cmd_rx,
            heard_rx,
            heard_tx,
            status: status.clone(),
            listen_status: listen.clone(),
            agenda: config.ui.autostart,
            project: Project::default(),
            path: None,
            config_path,
            config,
            fallback_log,
            composer: Composer::new(api),
            ptt: None,
            listener: None,
            muted: Arc::new(AtomicBool::new(false)),
            cursor: Utc::now(),
            next: None,
            next_checked: Instant::now() - Duration::from_secs(60),
            queue: VecDeque::new(),
            running: None,
            holder: None,
            serial: 0,
            pick: 0,
            last_cw: String::new(),
            last_dtmf: String::new(),
            log: Vec::new(),
            deferred: VecDeque::new(),
            clock_inflight: false,
            clock_next: Instant::now(),
        };
        let handle = std::thread::Builder::new().name("motor".into()).spawn(move || worker.run()).expect("spawn engine");
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Holder {
    Grid,
    Instant,
    Activation,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Completed,
    Cancelled,
    Cut,
}

const LOG_KEEP: usize = 200;

struct ActJob {
    id: u64,
    name: String,
    actions: Vec<Action>,
}

struct Worker {
    cmd_tx: Sender<Command>,
    cmd_rx: Receiver<Command>,
    heard_rx: Receiver<Heard>,
    heard_tx: Sender<Heard>,
    status: Arc<Mutex<Status>>,
    listen_status: Arc<Mutex<ListenStatus>>,
    agenda: bool,
    project: Project,
    path: Option<PathBuf>,
    config_path: PathBuf,
    config: Config,
    fallback_log: PathBuf,
    composer: Composer,
    ptt: Option<Ptt>,
    listener: Option<Listener>,
    muted: Arc<AtomicBool>,
    cursor: DateTime<Utc>,
    next: Option<Turn>,
    next_checked: Instant,
    queue: VecDeque<ActJob>,
    running: Option<u64>,
    holder: Option<Holder>,
    serial: u64,
    pick: u64,
    last_cw: String,
    last_dtmf: String,
    log: Vec<LogEntry>,
    deferred: VecDeque<Command>,
    clock_inflight: bool,
    clock_next: Instant,
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
        self.ptt = None;
    }

    fn now(&self) -> DateTime<Utc> {
        crate::clock::corrected(self.config.time.offset_ms)
    }

    fn tr(&self, en: &str, es: &str) -> String {
        Lang::resolve(&self.config.ui.lang).t(en, es)
    }

    fn dir(&self) -> Option<PathBuf> {
        self.path.as_deref().and_then(|p| p.parent()).map(|p| p.to_path_buf())
    }

    fn log_path(&self) -> PathBuf {
        self.path.as_deref().map(aslog::path_for).unwrap_or_else(|| self.fallback_log.clone())
    }

    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock().unwrap());
    }

    fn publish(&mut self) {
        let next = self.next.as_ref().map(|t| NextUp {
            at: t.at,
            due: t.at,
            name: t.name.clone(),
            program_id: t.program_id,
            program_name: t.program_name.clone(),
            block_id: t.block_id,
            freq_hz: t.freq_hz,
            mode: t.mode,
        });
        let (agenda, pending) = (self.agenda, self.queue.front().map(|j| j.name.clone()));
        let (busy, busy_grid, reason) = self.busy_view();
        let (offset, clock_error) = (self.config.time.offset_ms, self.config.time.last_error.clone());
        self.set_status(|s| {
            s.agenda = agenda;
            s.next = next;
            s.pending = pending;
            s.busy = busy;
            s.busy_grid = busy_grid;
            s.busy_reason = reason;
            s.clock_offset_ms = offset;
            s.clock_error = clock_error;
        });
    }

    fn busy_view(&self) -> (bool, bool, String) {
        match self.holder {
            Some(Holder::Grid) => (true, true, self.tr("The grid is transmitting", "La grilla está transmitiendo")),
            Some(Holder::Instant) => (true, false, self.tr("An instant transmission is on the air", "Hay una transmisión instantánea al aire")),
            Some(Holder::Activation) => (true, false, self.tr("An activation is on the air", "Hay una activación al aire")),
            None => (false, false, String::new()),
        }
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
                self.cursor = self.cursor.min(self.now());
                self.next = None;
            }
            Command::SetConfig(config) => {
                let audio_changed = config.audio != self.config.audio;
                let ptt_changed = config.ptt != self.config.ptt || config.cat != self.config.cat;
                let time = self.config.time.clone();
                self.config = *config;
                // The window does not own the offset; a settings save must not wipe a sync that just landed.
                if self.config.time.offset_ms == 0 && time.offset_ms != 0 && self.config.time.last_sync == time.last_sync {
                    self.config.time.offset_ms = time.offset_ms;
                }
                if ptt_changed {
                    self.ptt = None;
                }
                if audio_changed {
                    self.restart_listener();
                }
                self.clock_next = Instant::now();
            }
            Command::SetAgenda(on) => {
                self.agenda = on;
                if on {
                    self.cursor = self.now();
                }
                self.next = None;
            }
            Command::TransmitManual(manual) => {
                if self.holder.is_some() {
                    let reason = self.busy_view().2;
                    self.set_status(|s| s.error = Some(reason));
                } else {
                    self.play_manual(*manual);
                }
            }
            Command::Stop => {
                self.queue.clear();
            }
            Command::TestTone => self.test(true),
            Command::TestPtt => self.test(false),
            Command::Clock(result) => self.clock_done(result),
        }
        true
    }

    fn restart_listener(&mut self) {
        self.listener = None;
        let l = &self.project.listen;
        if l.cw || l.dtmf {
            self.listener = Some(Listener::start(self.config.audio.clone(), l.clone(), self.muted.clone(), self.listen_status.clone(), self.heard_tx.clone()));
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
        let acts: Vec<Activation> = self.project.activations.clone();
        for act in acts {
            if !act.enabled {
                continue;
            }
            let hit = act.triggers.iter().any(|t| {
                t.source == source
                    && match source {
                        TriggerSource::Dtmf => dtmf_matches(&text, &t.code),
                        TriggerSource::Cw => cw_matches(&text, &t.code),
                    }
            });
            if hit {
                self.fire(act);
            }
        }
    }

    /// Pause/resume that sit in front of the first audio action run now. The rest, in order,
    /// waits until the channel is free. A second match while this one is queued or running is ignored.
    fn fire(&mut self, act: Activation) {
        if self.queue.iter().any(|j| j.id == act.id) || self.running == Some(act.id) {
            return;
        }
        let mut actions = Vec::new();
        let mut leading = true;
        for action in act.actions {
            if leading && matches!(action, Action::Pause | Action::Resume) {
                self.apply_agenda(&action);
            } else {
                leading = false;
                actions.push(action);
            }
        }
        if actions.iter().any(Action::takes_ptt) {
            self.queue.push_back(ActJob { id: act.id, name: act.name, actions });
        } else {
            for action in &actions {
                self.apply_agenda(action);
            }
        }
    }

    fn apply_agenda(&mut self, action: &Action) {
        match action {
            Action::Pause => self.agenda = false,
            Action::Resume => {
                self.agenda = true;
                self.cursor = self.now();
                self.next = None;
            }
            _ => {}
        }
    }

    fn tick(&mut self) {
        let sources = self.project.all_api_sources();
        self.composer.api.refresh_due(&sources);
        self.poll_clock();
        if self.holder.is_none() {
            if let Some(job) = self.queue.pop_front() {
                self.run_job(job);
                self.publish();
                return;
            }
        }
        if self.next.is_none() || self.next_checked.elapsed() > Duration::from_secs(2) {
            self.compute_next();
        }
        if self.agenda && self.holder.is_none() {
            if let Some(turn) = self.next.clone() {
                match due(self.now(), &turn, None, self.project.tx.min_pause_s) {
                    air::Due::Wait => {}
                    air::Due::Skip => {
                        log::info!("turn at {} skipped", turn.at.format("%H:%M:%S"));
                        self.cursor = turn.at + TimeDelta::seconds(1);
                        self.next = None;
                    }
                    air::Due::Go => {
                        self.cursor = turn.at + TimeDelta::seconds(1);
                        self.next = None;
                        self.play_turn(turn, "grid", true);
                    }
                }
            }
        }
        self.publish();
    }

    fn compute_next(&mut self) {
        self.next_checked = Instant::now();
        let now = self.now();
        let today = grid_date(self.project.grid.timezone, now);
        let from = today - TimeDelta::days(1);
        let to = today + TimeDelta::days(2);
        let lead = self.config.ptt.lead_ms;
        let tail = self.config.ptt.tail_ms;
        let mut best: Option<Turn> = None;
        for occ in occurrences(&self.project, from, to, today) {
            if !occ.transmits() {
                continue;
            }
            for turn in air::plan_occurrence(&self.project, &occ, today, lead, tail, self.serial + 1, now) {
                if turn.at < self.cursor || !turn.airs {
                    continue;
                }
                if matches!(due(now, &turn, None, self.project.tx.min_pause_s), air::Due::Skip) {
                    continue;
                }
                if best.as_ref().is_none_or(|b| turn.at < b.at) {
                    best = Some(turn);
                }
            }
        }
        self.next = best;
    }

    fn poll_clock(&mut self) {
        if !self.config.time.enabled || self.clock_inflight || Instant::now() < self.clock_next {
            return;
        }
        self.clock_inflight = true;
        let server = self.config.time.server.clone();
        let tx = self.cmd_tx.clone();
        std::thread::spawn(move || {
            let result = crate::clock::query_offset_ms(&server, Duration::from_secs(3)).map_err(|e| e.to_string());
            let _ = tx.send(Command::Clock(result));
        });
    }

    fn clock_done(&mut self, result: Result<i64, String>) {
        self.clock_inflight = false;
        match result {
            Ok(ms) => {
                self.config.time.offset_ms = ms;
                self.config.time.last_error.clear();
                self.config.time.last_sync = Utc::now().to_rfc3339();
                let hours = self.config.time.period_h.max(1) as u64;
                self.clock_next = Instant::now() + Duration::from_secs(hours * 3600);
                self.next = None;
            }
            Err(e) => {
                log::warn!("NTP: {e}");
                self.config.time.last_error = e;
                self.clock_next = Instant::now() + Duration::from_secs(60);
            }
        }
        if let Err(e) = self.config.save(&self.config_path) {
            log::error!("could not save the clock: {e}");
        }
    }

    fn play_manual(&mut self, manual: air::Manual) {
        let now = self.now();
        let today = grid_date(self.project.grid.timezone, now);
        let turn = air::plan_manual(&self.project, &manual, self.config.ptt.lead_ms, self.config.ptt.tail_ms, self.serial + 1, now, today);
        self.play_turn(turn, "instantaneo", false);
    }

    fn play_turn(&mut self, turn: Turn, origin: &str, cutoff: bool) {
        self.holder = Some(if cutoff { Holder::Grid } else { Holder::Instant });
        let release = cutoff.then_some(air::release_at(turn.block_end));
        let freq = turn.freq_hz;
        let started = self.now();
        let serial = self.serial + 1;
        let built = self.turn_audio(&turn);
        let (segs, preview, total) = match built {
            Ok(v) => v,
            Err(e) => {
                self.set_status(|s| s.error = Some(e.to_string()));
                self.holder = None;
                return;
            }
        };
        let (scan_at, scan_ms) = air::image_window(&turn.pieces);
        self.show_on_air(&turn.name, origin, turn.mode, serial, started, total, preview, scan_at, scan_ms);
        let outcome = self.guarded(freq, release, |w| w.play_audio(&segs, release));
        self.finish_log(&turn.name, turn.mode, serial, started, freq, origin, turn.block_id, turn.program_id, &outcome);
        self.holder = None;
        let _ = outcome;
    }

    fn turn_audio(&mut self, turn: &Turn) -> anyhow::Result<(Vec<Seg>, Option<RgbaImage>, f64)> {
        let mut segs = Vec::new();
        let mut preview = None;
        for piece in &turn.pieces {
            if let Some(ready) = air::piece_segments(piece, &turn.call, turn.cw_wpm, turn.cw_tone) {
                segs.extend(ready);
                continue;
            }
            let mode = piece.mode.unwrap_or(turn.mode);
            let frame = self.render_piece(turn, piece, mode);
            if piece.kind == PieceKind::Image {
                preview = Some(frame.clone());
            }
            let scan = ScanImage::new(mode, &image::DynamicImage::ImageRgba8(frame).to_rgb8(), self.project.tx.color_range)?;
            for line in 0..mode.spec().tx_lines {
                scan.line(line, &mut segs);
            }
        }
        let total = segs.iter().map(|s| s.ms).sum();
        Ok((segs, preview, total))
    }

    fn render_piece(&mut self, turn: &Turn, piece: &air::Piece, mode: Mode) -> RgbaImage {
        let live = LiveText {
            program: turn.program_name.clone(),
            block: turn.name.clone(),
            range: turn.range.clone(),
            days: turn.weekdays.map(|d| crate::schedule::weekdays_label(&d)).unwrap_or_default(),
            daysleft: String::new(),
            next: String::new(),
        };
        let ctx = data_ctx(&self.project, self.now(), self.serial + 1, mode, &live, turn.freq_hz, &self.last_cw, &self.last_dtmf);
        let dir = self.dir();
        let block = matches!(turn.graphics, GraphicsLevel::Block)
            .then(|| turn.block_id.and_then(|id| self.project.block(id)).map(|b| b.graphics.clone()))
            .flatten();
        let program = matches!(turn.graphics, GraphicsLevel::Block | GraphicsLevel::Program)
            .then(|| turn.program_id.and_then(|id| self.project.program(id)).map(|p| p.graphics.clone()))
            .flatten();
        let channel = !matches!(turn.graphics, GraphicsLevel::None);
        self.composer.frame_chain_with(&self.project, dir.as_deref(), Some(&piece.path), None, block.as_ref(), program.as_ref(), channel, mode, &ctx)
    }

    fn run_job(&mut self, job: ActJob) {
        self.running = Some(job.id);
        self.holder = Some(Holder::Activation);
        let freq = self.project.freq_hz;
        let name = job.name.clone();
        let actions = job.actions.clone();
        let outcome = self.guarded(freq, None, |w| {
            for action in &actions {
                if matches!(action, Action::Pause | Action::Resume) {
                    w.apply_agenda(action);
                    continue;
                }
                match w.action_segments(action, &name) {
                    Ok(segs) if segs.is_empty() => {
                        let t = w.now();
                        w.write_log(&name, Mode::MartinM1, "sin salida", freq, &format!("activacion {name}"), None, None, false, t);
                    }
                    Ok(segs) => {
                        let serial = w.serial + 1;
                        let started = w.now();
                        let total = segs.iter().map(|s| s.ms).sum();
                        w.show_on_air(&name, &format!("activacion {name}"), Mode::MartinM1, serial, started, total, None, 0.0, 0.0);
                        let outcome = w.play_audio(&segs, None)?;
                        w.finish_log(&name, Mode::MartinM1, serial, started, freq, &format!("activacion {name}"), None, None, &Ok(outcome));
                        if !matches!(outcome, Outcome::Completed) {
                            return Ok(outcome);
                        }
                    }
                    Err(e) => {
                        log::warn!("activation {name}: {e}");
                        let t = w.now();
                        w.write_log(&name, Mode::MartinM1, &e, freq, &format!("activacion {name}"), None, None, false, t);
                    }
                }
            }
            Ok(Outcome::Completed)
        });
        if let Err(e) = &outcome {
            self.set_status(|s| s.error = Some(format!("{e:#}")));
        }
        self.running = None;
        self.holder = None;
    }

    fn action_segments(&mut self, action: &Action, name: &str) -> Result<Vec<Seg>, String> {
        let segs = match action {
            Action::Pause | Action::Resume => Vec::new(),
            Action::Cw { text, wpm, tone_hz } => {
                let live = self.plain_live(name);
                let ctx = data_ctx(&self.project, self.now(), self.serial + 1, self.project.tx.mode, &live, self.project.freq_hz, &self.last_cw, &self.last_dtmf);
                let text = expand(text, &ctx, None);
                if text.trim().is_empty() { Vec::new() } else { crate::synth::cw_segments(&text, *wpm, *tone_hz) }
            }
            Action::Ax25 { lines, comment } => {
                let info = crate::ax25::format_info(&crate::ax25::InfoFields {
                    lines: lines.clone().to_set(),
                    freq_hz: self.project.freq_hz,
                    call: &self.project.station.call,
                    name,
                    range: "",
                    days: None,
                    daysleft: None,
                    next: &[],
                    comment,
                });
                if info.is_empty() { Vec::new() } else { crate::ax25::segments(&info, &self.project.station.call) }
            }
            Action::Dtmf { sequence } => crate::synth::dtmf_segments(sequence),
            Action::Command { command, via } => {
                let live = self.plain_live(name);
                let ctx = data_ctx(&self.project, self.now(), self.serial + 1, self.project.tx.mode, &live, self.project.freq_hz, &self.last_cw, &self.last_dtmf);
                let line = expand(command, &ctx, None);
                let out = shell::capture(&line, Duration::from_secs(10)).map_err(|e| e.to_string())?;
                self.via(&out, *via)?
            }
            Action::Api { url, json_path, via } => {
                let src = crate::graphics::ApiSource { url: url.clone(), json_path: json_path.clone(), refresh_s: 0 };
                let out = crate::dynamic::fetch_for_tx(&src).map_err(|e| e.to_string())?;
                self.via(&out, *via)?
            }
            Action::Image { path, mode } => {
                if path.trim().is_empty() {
                    return Err("empty image".into());
                }
                self.sstv(path, self.project.mode_or(*mode), None).map_err(|e| e.to_string())?
            }
            Action::Random { paths, mode } => {
                if paths.is_empty() {
                    return Err("the image pool is empty".into());
                }
                let path = paths[self.pick as usize % paths.len()].clone();
                self.pick = self.pick.wrapping_add(1);
                self.sstv(&path, self.project.mode_or(*mode), None).map_err(|e| e.to_string())?
            }
        };
        if segs.is_empty() {
            return Ok(segs);
        }
        Ok(self.with_vox(segs))
    }

    fn plain_live(&self, name: &str) -> LiveText {
        LiveText { block: name.to_string(), ..LiveText::default() }
    }

    fn via(&self, text: &str, via: Carrier) -> Result<Vec<Seg>, String> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        Ok(match via {
            Carrier::Cw => crate::synth::cw_segments(text, self.project.tx.cw_wpm, self.project.tx.cw_tone_hz),
            Carrier::Dtmf => crate::synth::dtmf_segments(text),
            Carrier::Ax25 => crate::ax25::segments(text, &self.project.station.call),
        })
    }

    fn with_vox(&self, segs: Vec<Seg>) -> Vec<Seg> {
        if !self.project.tx.vox_preamble {
            return segs;
        }
        let mut out = crate::encoder::vox_segments();
        out.extend(segs);
        out
    }

    fn sstv(&mut self, path: &str, mode: Mode, mark: Option<&str>) -> anyhow::Result<Vec<Seg>> {
        let mut segs = Vec::new();
        if self.project.ax25.mode {
            let text = crate::ax25::mode_line(mode.spec().short, mark);
            segs.extend(crate::ax25::segments(&text, &self.project.station.call));
        }
        segs.extend(sstv_header(mode));
        let live = self.plain_live("");
        let ctx = data_ctx(&self.project, self.now(), self.serial + 1, mode, &live, self.project.freq_hz, &self.last_cw, &self.last_dtmf);
        let dir = self.dir();
        let frame = self.composer.frame_image(&self.project, dir.as_deref(), Some(path), mode, &ctx);
        let scan = ScanImage::new(mode, &image::DynamicImage::ImageRgba8(frame).to_rgb8(), self.project.tx.color_range)?;
        for line in 0..mode.spec().tx_lines {
            scan.line(line, &mut segs);
        }
        Ok(segs)
    }

    fn show_on_air(&mut self, name: &str, origin: &str, mode: Mode, serial: u64, started: DateTime<Utc>, total_ms: f64, image: Option<RgbaImage>, scan_at: f64, scan_ms: f64) {
        let image = image.map(Arc::new);
        let (name, origin) = (name.to_string(), origin.to_string());
        self.set_status(|s| {
            s.on_air = Some(OnAir { program: name, origin, mode, serial, started, total_ms, elapsed_ms: 0.0, picture: 0.0, scan_at, scan_ms, image, done: false });
            s.on_air_rev += 1;
            s.error = None;
            s.busy = true;
            s.busy_grid = matches!(self.holder, Some(Holder::Grid));
        });
    }

    fn guarded(&mut self, freq: u64, release: Option<DateTime<Utc>>, body: impl FnOnce(&mut Self) -> anyhow::Result<Outcome>) -> anyhow::Result<Outcome> {
        self.ensure_ptt()?;
        self.muted.store(true, Ordering::Relaxed);
        self.publish();
        self.set_status(|s| s.phase = Phase::Keying);
        let result = self.guarded_inner(freq, release, body);
        if let Some(p) = self.ptt.as_mut() {
            if let Err(e) = p.key(false) {
                log::error!("could not release PTT: {e}");
                self.ptt = None;
            }
        }
        self.muted.store(false, Ordering::Relaxed);
        self.set_status(|s| s.phase = Phase::Idle);
        result
    }

    fn guarded_inner(&mut self, freq: u64, release: Option<DateTime<Utc>>, body: impl FnOnce(&mut Self) -> anyhow::Result<Outcome>) -> anyhow::Result<Outcome> {
        if self.config.ptt.method == PttMethod::Cat && self.config.cat.set_freq && freq > 0 {
            if let Err(e) = ptt::set_frequency(&self.config.cat, freq) {
                log::warn!("could not change the frequency: {e}");
            }
        }
        self.ptt.as_mut().expect("ensure_ptt").key(true)?;
        match self.wait_ms(self.config.ptt.lead_ms as u64, release) {
            Outcome::Completed => {}
            other => return Ok(other),
        }
        let outcome = body(self)?;
        if outcome == Outcome::Completed {
            self.set_status(|s| s.phase = Phase::Tail);
            if self.wait_ms(self.config.ptt.tail_ms as u64, release) == Outcome::Cut {
                return Ok(Outcome::Cut);
            }
        }
        Ok(outcome)
    }

    fn wait_ms(&mut self, ms: u64, release: Option<DateTime<Utc>>) -> Outcome {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            if self.drain() {
                return Outcome::Cancelled;
            }
            if release.is_some_and(|r| self.now() >= r) {
                return Outcome::Cut;
            }
            let left = end.saturating_duration_since(Instant::now());
            std::thread::sleep(left.min(Duration::from_millis(5)));
        }
        if release.is_some_and(|r| self.now() >= r) { Outcome::Cut } else { Outcome::Completed }
    }

    fn play_audio(&mut self, segs: &[Seg], release: Option<DateTime<Utc>>) -> anyhow::Result<Outcome> {
        let total = segs.iter().map(|s| s.ms).sum::<f64>();
        let k = self.ppm_factor();
        let owned = segs.to_vec();
        let playback = crate::audio::play(&self.config.audio, move |rate| Box::new(Synth::new(owned.into_iter(), rate as f64 * k, 1.0, total)))?;
        self.set_status(|s| s.phase = Phase::OnAir);
        let rate = playback.sample_rate as f64;
        let started = Instant::now();
        let timeout = Duration::from_secs(self.config.ptt.timeout_s.max(10) as u64);
        let mut finished_at = None;
        let outcome = loop {
            if self.drain() {
                break Outcome::Cancelled;
            }
            if release.is_some_and(|r| self.now() >= r) {
                break Outcome::Cut;
            }
            if started.elapsed() > timeout {
                anyhow::bail!("{} ({} s)", self.tr("cut: transmission exceeded the maximum time", "se cortó por superar el tiempo máximo de transmisión"), timeout.as_secs());
            }
            let elapsed = (playback.played_samples() as f64 / rate * 1000.0).min(total);
            self.set_status(|s| {
                if let Some(a) = s.on_air.as_mut() {
                    a.elapsed_ms = elapsed;
                    a.picture = if total > 0.0 { (elapsed / total).clamp(0.0, 1.0) } else { 1.0 };
                }
            });
            if playback.finished() {
                let t = *finished_at.get_or_insert_with(Instant::now);
                if t.elapsed() > Duration::from_millis(250) {
                    break Outcome::Completed;
                }
            }
            let pause = match release {
                Some(r) => {
                    let left = (r - self.now()).num_milliseconds();
                    if left < 3 { Duration::from_micros(200) } else { Duration::from_millis((left as u64 / 2).clamp(1, 20)) }
                }
                None => Duration::from_millis(40),
            };
            std::thread::sleep(pause);
        };
        drop(playback);
        Ok(outcome)
    }

    /// Stop, shutdown, agenda and instant requests that arrive while the PTT is down.
    /// Heard triggers are applied too, so a duplicate activation is seen while this one is still running.
    fn drain(&mut self) -> bool {
        let mut stop = false;
        while let Ok(cmd) = self.cmd_rx.try_recv() {
            match cmd {
                Command::Stop => stop = true,
                Command::Shutdown => {
                    self.deferred.push_back(Command::Shutdown);
                    stop = true;
                }
                Command::SetAgenda(on) => {
                    self.agenda = on;
                    if on {
                        self.next = None;
                    }
                }
                Command::TransmitManual(_) => {
                    let reason = self.busy_view().2;
                    self.set_status(|s| s.error = Some(reason));
                }
                Command::Clock(result) => self.clock_done(result),
                other => self.deferred.push_back(other),
            }
        }
        while let Ok(h) = self.heard_rx.try_recv() {
            self.on_heard(h);
        }
        stop
    }

    fn ensure_ptt(&mut self) -> anyhow::Result<()> {
        if self.ptt.is_none() {
            self.ptt = Some(Ptt::open(&self.config)?);
        }
        Ok(())
    }

    fn ppm_factor(&self) -> f64 {
        1.0 + self.config.audio.tx_ppm / 1e6
    }

    fn finish_log(&mut self, name: &str, mode: Mode, serial: u64, started: DateTime<Utc>, freq: u64, origin: &str, block: Option<u64>, program: Option<u64>, outcome: &anyhow::Result<Outcome>) {
        let (result, cut) = match outcome {
            Ok(Outcome::Completed) => ("OK".to_string(), false),
            Ok(Outcome::Cancelled) => ("Cut".to_string(), false),
            Ok(Outcome::Cut) => ("Cut".to_string(), true),
            Err(e) => (format!("{}: {e:#}", self.tr("Error", "Error")), false),
        };
        self.write_log(name, mode, &result, freq, origin, block, program, cut, started);
        self.serial = serial;
        let err = outcome.as_ref().err().map(|e| format!("{e:#}"));
        self.set_status(|s| {
            s.serial = serial;
            if let Some(a) = s.on_air.as_mut() {
                a.done = true;
                a.elapsed_ms = a.total_ms;
            }
            if let Some(e) = err {
                s.error = Some(e);
            }
        });
    }

    fn write_log(&mut self, name: &str, mode: Mode, result: &str, freq: u64, origin: &str, block: Option<u64>, program: Option<u64>, cut: bool, start: DateTime<Utc>) {
        let entry = LogEntry {
            start,
            end: self.now(),
            serial: self.serial + 1,
            mode: mode.name().into(),
            program: name.into(),
            freq_hz: freq,
            origin: origin.into(),
            result: result.into(),
            block_id: block.unwrap_or(0),
            program_id: program.unwrap_or(0),
            cut,
        };
        if let Err(e) = aslog::append(&self.log_path(), &entry) {
            log::error!("log: {e}");
        }
        self.serial = entry.serial;
        self.log.push(entry);
        if self.log.len() > LOG_KEEP {
            self.log.remove(0);
        }
        let log = self.log.clone();
        let serial = self.serial;
        self.set_status(|s| {
            s.log = log;
            s.log_rev += 1;
            s.serial = serial;
        });
    }

    fn test(&mut self, tone: bool) {
        if self.holder.is_some() {
            return;
        }
        self.holder = Some(Holder::Instant);
        let seg = if tone { Seg::tone(1900.0, 5000.0) } else { Seg::silence(2000.0) };
        let result = self.guarded(0, None, |w| w.play_audio(&[seg], None));
        self.holder = None;
        if let Err(e) = result {
            self.set_status(|s| s.error = Some(format!("{}: {e:#}", self.tr("Test", "Prueba"))));
        }
    }
}
