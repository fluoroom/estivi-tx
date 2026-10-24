//! Application state and the main (playout) window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant, SystemTime};

use chrono::{Local, Utc};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use estivi_core::aslog::LogEntry;
use estivi_core::compose::{Composer, data_ctx};
use estivi_core::config::Config;
use estivi_core::dynamic::ApiCache;
use estivi_core::engine::{Command, Engine, Phase, Status};
use estivi_core::graphics::LayerKind;
use estivi_core::modes::Mode;
use estivi_core::project::{self, GridTz, Program, Project, Rotation, Slot, relativize, resolve};
use estivi_core::schedule;

use crate::view;
use crate::{ConfirmDialog, Files, GridBlock, LogItem, MainWindow, ProgramItem, SlotItem, Theme};

pub type Shared = Rc<RefCell<App>>;

pub fn bind_files(files: &Files) {
    files.on_has_files(|dt| dt.has_file_paths() || dt.has_plain_text());
    files.on_paths_of(|dt| ModelRc::new(VecModel::from(paths_from_transfer(&dt))));
}

/// Slint's DropArea does not receive OS file lists on the winit backend.
/// winit itself does (X11 XDND → HoveredFile / DroppedFile); this hooks those.
pub fn hook_os_drops(
    window: &slint::Window,
    on_hover: impl Fn(bool) + 'static,
    on_drop: impl Fn(Vec<PathBuf>) + 'static,
) {
    use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};

    let pending = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
    let on_hover = Rc::new(on_hover);
    let on_drop = Rc::new(on_drop);
    let flush = slint::Timer::default();
    window.on_winit_window_event(move |_win, ev| match ev {
        WindowEvent::HoveredFile(path) => {
            let mut list = pending.borrow_mut();
            if !list.iter().any(|p| p == path) {
                list.push(path.clone());
            }
            on_hover(true);
            EventResult::PreventDefault
        }
        WindowEvent::HoveredFileCancelled => {
            pending.borrow_mut().clear();
            on_hover(false);
            EventResult::PreventDefault
        }
        WindowEvent::DroppedFile(path) => {
            {
                let mut list = pending.borrow_mut();
                if !list.iter().any(|p| p == path) {
                    list.push(path.clone());
                }
            }
            let pending = pending.clone();
            let on_hover = on_hover.clone();
            let on_drop = on_drop.clone();
            flush.start(slint::TimerMode::SingleShot, Duration::ZERO, move || {
                let paths = std::mem::take(&mut *pending.borrow_mut());
                on_hover(false);
                if !paths.is_empty() {
                    on_drop(paths);
                }
            });
            EventResult::PreventDefault
        }
        _ => EventResult::Propagate,
    });
}

pub fn paths_model(paths: &[PathBuf]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        paths.iter().map(|p| SharedString::from(p.to_string_lossy().into_owned())).collect::<Vec<_>>(),
    ))
}

#[derive(Clone, Copy)]
pub enum NativePick {
    Images { many: bool },
    Media,
    ProjectOpen,
    ProjectSave,
}

/// Native OS file dialog, off the UI thread. Empty list if the user cancelled.
pub fn pick_native(kind: NativePick, done: impl FnOnce(Vec<PathBuf>) + Send + 'static) {
    std::thread::spawn(move || {
        let ext = project::EXTENSION;
        let paths = match kind {
            NativePick::Images { many: true } => rfd::FileDialog::new()
                .set_title("Elegir imágenes")
                .add_filter("Imágenes", &IMAGE_EXTS)
                .pick_files()
                .unwrap_or_default(),
            NativePick::Images { many: false } => rfd::FileDialog::new()
                .set_title("Elegir imagen")
                .add_filter("Imágenes", &IMAGE_EXTS)
                .pick_file()
                .into_iter()
                .collect(),
            NativePick::Media => rfd::FileDialog::new()
                .set_title("Elegir imágenes o un proyecto")
                .add_filter("Imágenes", &IMAGE_EXTS)
                .add_filter("Proyecto Estivi", &[ext])
                .pick_files()
                .unwrap_or_default(),
            NativePick::ProjectOpen => rfd::FileDialog::new()
                .set_title("Abrir proyecto")
                .add_filter("Proyecto Estivi", &[ext])
                .pick_file()
                .into_iter()
                .collect(),
            NativePick::ProjectSave => rfd::FileDialog::new()
                .set_title("Guardar proyecto")
                .add_filter("Proyecto Estivi", &[ext])
                .save_file()
                .into_iter()
                .collect(),
        };
        let _ = slint::invoke_from_event_loop(move || done(paths));
    });
}

fn paths_from_transfer(dt: &slint::DataTransfer) -> Vec<SharedString> {
    let mut out: Vec<SharedString> = dt
        .file_paths()
        .ok()
        .into_iter()
        .flatten()
        .map(|p| p.to_string_lossy().into_owned().into())
        .collect();
    if out.is_empty() {
        if let Ok(text) = dt.plain_text() {
            out.extend(paths_from_uri_list(&text));
        }
    }
    out
}

/// Nautilus / Dolphin / Thunar send `text/uri-list` (`file:///…`).
fn paths_from_uri_list(text: &str) -> Vec<SharedString> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let path = if let Some(rest) = line.strip_prefix("file://") {
                let rest = rest.strip_prefix("localhost").unwrap_or(rest);
                percent_decode(rest)
            } else if line.starts_with('/') {
                line.to_string()
            } else {
                return None;
            };
            Some(SharedString::from(path))
        })
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn apply_theme_to(theme: &Theme, light: bool) {
    theme.set_light(light);
}

pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn is_project_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(project::EXTENSION))
}

pub struct App {
    pub project: Project,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub config: Config,
    pub config_path: PathBuf,
    pub engine: Engine,
    pub composer: Composer,
    pub main: slint::Weak<MainWindow>,
    /// Last engine snapshot.
    pub status: Status,
    /// Bumped on every project change (cache key for previews).
    pub generation: u64,
    pub editor: Option<crate::editor::Editor>,
    pub dialogs: crate::dialogs::Dialogs,
    me: Weak<RefCell<App>>,
    confirm: Option<ConfirmDialog>,
    after_save: Option<Then>,
    thumbs: HashMap<PathBuf, (Option<SystemTime>, Option<slint::Image>)>,
    seen_on_air: u64,
    seen_log: u64,
    preview_key: Option<(Option<u64>, i64, u64)>,
    preview_at: Instant,
    timeline_key: Option<(chrono::NaiveDate, u64, u64)>,
    last_second: i64,
}

/// Runs a closure with the app borrowed, if it is still alive and not busy.
pub fn with_app(weak: &Weak<RefCell<App>>, f: impl FnOnce(&mut App)) {
    if let Some(app) = weak.upgrade() {
        if let Ok(mut a) = app.try_borrow_mut() {
            f(&mut a);
        }
    }
}

impl App {
    pub fn ui(&self) -> MainWindow {
        self.main.upgrade().expect("main window alive")
    }

    pub fn dir(&self) -> Option<PathBuf> {
        self.path.as_deref().and_then(Path::parent).map(Path::to_path_buf)
    }

    pub fn resolve(&self, stored: &str) -> PathBuf {
        resolve(self.dir().as_deref(), stored)
    }

    pub fn relativize(&self, abs: &Path) -> String {
        relativize(self.dir().as_deref(), abs)
    }

    fn send_project(&self) {
        self.engine.send(Command::SetProject { project: Box::new(self.project.clone()), path: self.path.clone() });
    }

    /// Call after any edit of the project.
    pub fn changed(&mut self) {
        self.dirty = true;
        self.generation += 1;
        self.send_project();
        self.refresh_all();
    }

    /// Lighter variant for the graphics editor: no list rebuilds, and the engine can
    /// be skipped while a drag is in progress.
    pub fn graphics_changed(&mut self, notify_engine: bool) {
        self.dirty = true;
        self.generation += 1;
        if notify_engine {
            self.send_project();
        }
        self.ui().set_dirty(true);
        self.preview_key = None;
    }

    /// Call after loading or creating a project.
    pub fn loaded(&mut self) {
        self.dirty = false;
        self.generation += 1;
        self.composer.set_project_dir(self.dir().as_deref());
        self.send_project();
        self.refresh_all();
        self.config.ui.last_project = self.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if let Err(e) = self.config.save(&self.config_path) {
            log::warn!("no se pudo guardar la configuración: {e:#}");
        }
    }

    pub fn refresh_all(&mut self) {
        self.refresh_header();
        self.refresh_programs();
        self.refresh_slots();
        self.preview_key = None;
        self.timeline_key = None;
        if let Some(mut ed) = self.editor.take() {
            ed.refresh(self, false);
            self.editor = Some(ed);
        }
    }

    fn refresh_header(&self) {
        let ui = self.ui();
        let spec = self.project.tx.mode.spec();
        ui.set_project_name(self.project.name.as_str().into());
        ui.set_project_file(self.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default().into());
        ui.set_dirty(self.dirty);
        ui.set_callsign(self.project.station.call.as_str().into());
        let modes: Vec<SharedString> = Mode::ALL.iter().map(|m| m.name().into()).collect();
        ui.set_modes(ModelRc::new(VecModel::from(modes)));
        ui.set_mode_index(Mode::ALL.iter().position(|m| *m == self.project.tx.mode).unwrap_or(0) as i32);
        ui.set_mode_detail(format!("{} · {}×{} px · {} por imagen", spec.name, spec.width, spec.height, view::duration(spec.total_ms())).into());
        ui.set_rotation_index(match self.project.rotation {
            Rotation::Sequential => 0,
            Rotation::Shuffle => 1,
        });
        ui.set_tz_index(match self.project.grid.timezone {
            GridTz::Utc => 0,
            GridTz::Local => 1,
        });
    }

    pub fn thumb(&mut self, path: &Path) -> Option<slint::Image> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some((t, img)) = self.thumbs.get(path) {
            if *t == mtime {
                return img.clone();
            }
        }
        let img = mtime.and_then(|_| view::thumbnail(path));
        self.thumbs.insert(path.to_path_buf(), (mtime, img.clone()));
        img
    }

    fn refresh_programs(&mut self) {
        let programs = self.project.programs.clone();
        let items: Vec<ProgramItem> = programs
            .iter()
            .map(|p| {
                let first = p.first_path().unwrap_or("");
                let abs = self.resolve(first);
                let thumb = if first.is_empty() { None } else { self.thumb(&abs) };
                let missing = p.images.iter().any(|i| {
                    if !i.enabled {
                        return false;
                    }
                    let img_abs = self.resolve(&i.path);
                    self.thumb(&img_abs).is_none()
                });
                ProgramItem {
                    id: p.id as i32,
                    name: p.name.as_str().into(),
                    file: first.into(),
                    enabled: p.enabled,
                    missing,
                    thumb: thumb.unwrap_or_default(),
                    count: p.image_count() as i32,
                    hours: SharedString::new(),
                    mode: p.mode.map(|m| SharedString::from(m.spec().short)).unwrap_or_default(),
                }
            })
            .collect();
        let ui = self.ui();
        ui.set_programs(ModelRc::new(VecModel::from(items)));
        if ui.get_selected_program() >= programs.len() as i32 {
            ui.set_selected_program(programs.len() as i32 - 1);
        }
    }

    fn refresh_slots(&self) {
        let names: HashMap<u64, &str> = self.project.programs.iter().map(|p| (p.id, p.name.as_str())).collect();
        let items: Vec<SlotItem> = self
            .project
            .grid
            .slots
            .iter()
            .map(|s| SlotItem {
                id: s.id as i32,
                enabled: s.enabled,
                name: s.name.as_str().into(),
                days: view::days_summary(&s.days).into(),
                hours: view::hours_summary(s).into(),
                content: match s.program {
                    None => "Rotación".into(),
                    Some(id) => names.get(&id).map_or("(programa borrado → rotación)".into(), |n| SharedString::from(*n)),
                },
                freq: view::freq(s.freq_hz).into(),
            })
            .collect();
        let ui = self.ui();
        ui.set_slots(ModelRc::new(VecModel::from(items)));
        if ui.get_selected_slot() >= self.project.grid.slots.len() as i32 {
            ui.set_selected_slot(self.project.grid.slots.len() as i32 - 1);
        }
    }

    fn refresh_log(&self, log: &[LogEntry]) {
        let items: Vec<LogItem> = log
            .iter()
            .rev()
            .map(|e| LogItem {
                time: e.start.format("%d/%m %H:%M:%S").to_string().into(),
                serial: e.serial.to_string().into(),
                program: e.program.as_str().into(),
                mode: e.mode.as_str().into(),
                origin: e.origin.as_str().into(),
                result: e.result.as_str().into(),
                ok: e.result == "OK",
            })
            .collect();
        self.ui().set_log(ModelRc::new(VecModel::from(items)));
    }

    fn refresh_timeline(&mut self) {
        let tz = self.project.grid.timezone;
        let today = view::grid_today(tz);
        let key = (today, self.generation, self.status.log_rev);
        let ui = self.ui();
        let start = view::grid_midnight(today, tz);
        let day_ms = 86_400_000.0;
        // Minute resolution: finer steps would redraw the whole window ten times a second
        // for a sub-pixel move of the "now" line.
        ui.set_now_pos(((Utc::now() - start).num_minutes() as f64 * 60_000.0 / day_ms).clamp(0.0, 1.0) as f32);
        if self.timeline_key == Some(key) {
            return;
        }
        self.timeline_key = Some(key);
        let end = start + chrono::TimeDelta::days(1);
        let near = |a: chrono::DateTime<Utc>, b: chrono::DateTime<Utc>| (a - b).num_seconds().abs() <= 90;
        let on_air = self.status.on_air.as_ref().filter(|a| !a.done).map(|a| a.started);
        let blocks: Vec<GridBlock> = schedule::occurrences(&self.project.grid, start, end)
            .into_iter()
            .map(|o| {
                let mode = o.program.and_then(|id| self.project.program(id)).map(|p| self.project.mode_of(Some(p), None)).unwrap_or(self.project.tx.mode);
                let tx_ms = mode.spec().total_ms() + 2000.0;
                let aired = self.status.log.iter().rev().find(|e| near(e.start, o.at));
                let state = if on_air.is_some_and(|t| near(t, o.at)) {
                    3
                } else {
                    match aired {
                        Some(e) if e.result == "OK" => 1,
                        Some(_) => 2,
                        None => 0,
                    }
                };
                GridBlock { start: ((o.at - start).num_milliseconds() as f64 / day_ms) as f32, len: (tx_ms / day_ms) as f32, state }
            })
            .collect();
        ui.set_grid_blocks(ModelRc::new(VecModel::from(blocks)));
        ui.set_day_label(view::day_label(today, tz, self.project.grid.offset_s).into());
    }

    fn refresh_next_preview(&mut self) {
        let next = self.status.next.clone();
        let program_id = next.as_ref().and_then(|n| n.program);
        let minute = next.as_ref().map_or(Utc::now().timestamp() / 60, |n| n.at.timestamp() / 60);
        let key = (program_id, minute, self.generation);
        // Re-render on changes, and every minute for clock data.
        if self.preview_key == Some(key) && self.preview_at.elapsed() < Duration::from_secs(60) {
            return;
        }
        self.preview_key = Some(key);
        self.preview_at = Instant::now();
        let ui = self.ui();
        let program = program_id.and_then(|id| self.project.program(id)).or_else(|| self.project.programs.iter().find(|p| p.enabled)).cloned();
        let Some(program) = program else {
            ui.set_next_has_image(false);
            ui.set_next_info("".into());
            return;
        };
        let image = program.enabled_images().next();
        let mode = self.project.mode_of(Some(&program), image);
        let spec = mode.spec();
        ui.set_next_aspect(spec.height as f32 / spec.width as f32);
        ui.set_mode_detail(format!("{} · {} {} · {}", spec.name, spec.size_label(), spec.color_space(), view::duration(spec.total_ms())).into());
        let at = next.as_ref().map_or_else(Utc::now, |n| n.at);
        let ctx = data_ctx(&self.project, at, self.status.serial + 1, &program.name, next.as_ref().map_or(0, |n| n.freq_hz), &self.status.last_cw, &self.status.last_dtmf, mode);
        let dir = self.dir();
        let frame = self.composer.frame(&self.project, dir.as_deref(), Some(&program), &ctx);
        ui.set_next_image(view::to_image(&frame));
        ui.set_next_has_image(true);
        ui.set_next_info(match &next {
            Some(n) => format!("{} · {}", view::grid_time(n.at, self.project.grid.timezone), program.name).into(),
            None => format!("{} (fuera de grilla)", program.name).into(),
        });
    }

    /// Called ~10 times per second.
    pub fn tick(&mut self) {
        self.status = self.engine.status();
        let st = self.status.clone();
        let ui = self.ui();
        let now = Utc::now();

        if now.timestamp() != self.last_second {
            self.last_second = now.timestamp();
            ui.set_utc_time(now.format("%H:%M:%S").to_string().into());
            ui.set_utc_date(now.format("%d/%m/%Y").to_string().into());
            ui.set_local_time(Local::now().format("%H:%M").to_string().into());
        }

        ui.set_phase(match st.phase {
            Phase::Idle => 0,
            Phase::Keying => 1,
            Phase::OnAir => 2,
            Phase::Tail => 3,
        });
        ui.set_agenda(st.agenda);
        ui.set_error_text(st.error.clone().unwrap_or_default().into());
        ui.set_pending_text(st.pending.as_ref().map(|p| format!("{p} · sale cuando termine la pausa mínima")).unwrap_or_default().into());
        let tz = self.project.grid.timezone;
        match &st.next {
            Some(n) => {
                ui.set_next_time(view::grid_time(n.at, tz).into());
                ui.set_next_title(n.program_name.as_str().into());
                let late = if n.due > n.at { format!(" · sale {} por la pausa mínima", view::grid_time(n.due, tz)) } else { String::new() };
                ui.set_next_slot(format!("Bloque «{}»{}", n.slot, late).into());
                ui.set_countdown(view::countdown((n.due - now).num_seconds()).into());
            }
            None => {
                ui.set_next_time("--:--".into());
                ui.set_next_title(if self.project.grid.slots.is_empty() { "La grilla está vacía".into() } else { "Sin emisiones en los próximos días".into() });
                ui.set_next_slot("".into());
                ui.set_countdown("--:--".into());
            }
        }

        if let Some(a) = &st.on_air {
            if st.on_air_rev != self.seen_on_air {
                self.seen_on_air = st.on_air_rev;
                if let Some(img) = &a.image {
                    ui.set_on_air_image(view::to_image(img));
                    ui.set_on_air_aspect(img.height() as f32 / img.width() as f32);
                    ui.set_on_air_has_image(true);
                }
                self.timeline_key = None;
            }
            ui.set_on_air_done(a.done);
            ui.set_on_air_progress(a.picture as f32);
            ui.set_on_air_title(a.program.as_str().into());
            ui.set_on_air_info(format!("{} · emisión #{} · {}", a.mode.name(), a.serial, a.origin).into());
            ui.set_on_air_time(if a.done {
                a.started.format("%H:%M UTC").to_string().into()
            } else {
                format!("{} / {}", view::clock(a.elapsed_ms), view::clock(a.total_ms)).into()
            });
        }

        if st.log_rev != self.seen_log {
            self.seen_log = st.log_rev;
            self.refresh_log(&st.log);
        }

        let l = &st.listen;
        let wants = self.project.listen.cw || self.project.listen.dtmf;
        ui.set_listen_on(l.running);
        ui.set_listen_level(if l.running { ((l.level_db + 60.0) / 60.0).clamp(0.0, 1.0) } else { 0.0 });
        let cw: String = {
            let n = l.cw_text.chars().count();
            l.cw_text.chars().skip(n.saturating_sub(36)).collect()
        };
        ui.set_listen_cw(cw.into());
        ui.set_listen_dtmf(l.dtmf_text.as_str().into());
        ui.set_listen_error(l.error.clone().unwrap_or_default().into());
        let mut what = Vec::new();
        if self.project.listen.cw {
            what.push(format!("CW {:.0} ppm a {:.0} Hz", self.project.listen.cw_wpm, self.project.listen.cw_tone_hz));
        }
        if self.project.listen.dtmf {
            what.push("DTMF".into());
        }
        ui.set_listen_state(
            if l.error.is_some() {
                "Error de audio".into()
            } else if !wants {
                "Desactivada".into()
            } else if l.running {
                format!("Activa · {}", what.join(" · "))
            } else {
                "Iniciando…".into()
            }
            .into(),
        );

        self.refresh_timeline();
        self.refresh_next_preview();
        if let Some(mut ed) = self.editor.take() {
            ed.tick(self);
            self.editor = Some(ed);
        }
    }
}

/// Converts stored paths when the project file moves to another folder.
fn rebase_paths(project: &mut Project, old: Option<&Path>, new: Option<&Path>) {
    let fix = |s: &mut String| {
        if !s.trim().is_empty() {
            *s = relativize(new, &resolve(old, s));
        }
    };
    for p in &mut project.programs {
        for img in &mut p.images {
            fix(&mut img.path);
        }
        fix(&mut p.path);
    }
    for l in &mut project.graphics.layers {
        if let LayerKind::Image(i) = &mut l.kind {
            fix(&mut i.path);
        }
    }
}

/// What to do once unsaved changes are dealt with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    New,
    Open,
    Quit,
}

// File dialog purposes, carried through the hop back to the UI thread.
pub const PICK_PROGRAMS: &str = "programas";
pub const PICK_OPEN: &str = "abrir";
pub const PICK_SAVE: &str = "guardar";
pub const PICK_LAYER_NEW: &str = "capa-nueva";
pub const PICK_LAYER_FILE: &str = "capa-archivo";

const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "bmp", "gif"];

fn normalize_save_path(p: PathBuf, name: &str) -> PathBuf {
    if p.is_dir() {
        let stem = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect::<String>();
        p.join(format!("{}.{}", if stem.is_empty() { "canal" } else { &stem }, project::EXTENSION))
    } else if p.extension().is_none() {
        p.with_extension(project::EXTENSION)
    } else {
        p
    }
}

impl App {
    /// A message in our own window (no dependency on zenity or similar helpers).
    pub fn notify(&mut self, heading: &str, message: &str) {
        self.show_confirm(heading, message, ("Aceptar", "", ""), None);
    }

    fn show_confirm(&mut self, heading: &str, message: &str, (primary, secondary, cancel): (&str, &str, &str), then: Option<Then>) {
        let Ok(dlg) = ConfirmDialog::new() else { return };
        apply_theme_to(&dlg.global::<Theme>(), self.config.ui.light);
        dlg.set_heading(heading.into());
        dlg.set_message(message.into());
        dlg.set_primary(primary.into());
        dlg.set_secondary(secondary.into());
        dlg.set_cancel(cancel.into());
        let w = self.me.clone();
        let d = dlg.as_weak();
        dlg.on_chosen(move |choice| {
            if let Some(d) = d.upgrade() {
                let _ = d.hide();
            }
            let (Some(app), Some(then)) = (w.upgrade(), then) else { return };
            match choice {
                0 => save(&app, false, Some(then)),
                1 => perform(&app, then),
                _ => {}
            }
        });
        let _ = dlg.show();
        self.confirm = Some(dlg);
    }
}

/// Deals with unsaved changes (asking), then does `then`.
pub fn request(app: &Shared, then: Then) {
    if !app.borrow().dirty {
        perform(app, then);
        return;
    }
    app.borrow_mut().show_confirm(
        "¿Guardar los cambios?",
        "El proyecto tiene cambios sin guardar.",
        ("Guardar", "No guardar", "Cancelar"),
        Some(then),
    );
}

fn perform(app: &Shared, then: Then) {
    match then {
        Then::New => {
            let mut a = app.borrow_mut();
            a.project = Project::default();
            a.path = None;
            a.loaded();
        }
        Then::Open => {
            crate::dialogs::ask_path(
                app,
                "Abrir proyecto",
                "Soltá el archivo .estivi desde el explorador, o escribí la ruta.",
                "Abrir",
                PICK_OPEN,
            );
        }
        Then::Quit => {
            let _ = slint::quit_event_loop();
        }
    }
}

/// Saves to the current file, or asks for one (always when `ask`). `then` runs after a
/// successful save.
pub fn save(app: &Shared, ask: bool, then: Option<Then>) {
    let (current, name, ui) = {
        let a = app.borrow();
        (a.path.clone(), a.project.name.clone(), a.main.clone())
    };
    match (current, ask) {
        (Some(p), false) => {
            if write_project(app, p) {
                if let Some(t) = then {
                    perform(app, t);
                }
            }
        }
        (current, _) => {
            app.borrow_mut().after_save = then;
            let _ = (current, name, ui);
            crate::dialogs::ask_path(
                app,
                "Guardar proyecto",
                "Soltá una carpeta o un .estivi existente, o escribí la ruta completa.",
                "Guardar",
                PICK_SAVE,
            );
        }
    }
}

fn write_project(app: &Shared, mut target: PathBuf) -> bool {
    if target.extension().is_none() {
        target.set_extension(project::EXTENSION);
    }
    let mut a = app.borrow_mut();
    let old_dir = a.dir();
    let mut project = a.project.clone();
    rebase_paths(&mut project, old_dir.as_deref(), target.parent());
    match project.save(&target) {
        Ok(()) => {
            a.project = project;
            a.path = Some(target);
            a.loaded();
            true
        }
        Err(e) => {
            a.notify("No se pudo guardar el proyecto", &format!("{e:#}"));
            false
        }
    }
}

fn files_picked(app: &Shared, purpose: &str, paths: Vec<PathBuf>) {
    if purpose == PICK_SAVE {
        let then = app.borrow_mut().after_save.take();
        if let Some(p) = paths.into_iter().next() {
            let name = app.borrow().project.name.clone();
            if write_project(app, normalize_save_path(p, &name)) {
                if let Some(t) = then {
                    perform(app, t);
                }
            }
        }
        return;
    }
    let mut a = app.borrow_mut();
    apply_picked(&mut a, purpose, paths);
}

pub fn apply_picked(a: &mut App, purpose: &str, paths: Vec<PathBuf>) {
    let Some(first) = paths.first().cloned() else { return };
    match purpose {
        PICK_OPEN => match Project::load(&first) {
            Ok(p) => {
                a.project = p;
                a.path = Some(first);
                a.loaded();
            }
            Err(e) => a.notify("No se pudo abrir el proyecto", &format!("{e:#}")),
        },
        PICK_PROGRAMS => {
            add_images_as_program(a, paths);
        }
        PICK_LAYER_NEW | PICK_LAYER_FILE => {
            if is_image(&first) {
                crate::editor::image_picked_mut(a, purpose == PICK_LAYER_NEW, &first);
            }
        }
        _ => {}
    }
}

fn add_images_as_program(a: &mut App, paths: Vec<PathBuf>) {
    let images: Vec<estivi_core::project::ProgramImage> = paths
        .iter()
        .filter(|p| is_image(p))
        .map(|f| estivi_core::project::ProgramImage::new(a.relativize(f)))
        .collect();
    if images.is_empty() {
        return;
    }
    let name = paths
        .first()
        .and_then(|f| f.file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Programa".into());
    let id = a.project.next_program_id();
    a.project.programs.push(Program { id, name, images, ..Default::default() });
    a.changed();
}

pub fn run(config: Config, config_path: PathBuf) -> anyhow::Result<()> {
    let ui = MainWindow::new()?;
    let api = ApiCache::default();
    let engine = Engine::start(config.clone(), config_path.with_file_name("estivi-registro.csv"), api.clone());
    engine.send(Command::SetConfig(Box::new(config.clone())));
    // A project given on the command line (double click on a .estivi file) wins over
    // the last one used.
    let last = std::env::args_os()
        .nth(1)
        .map(|a| a.to_string_lossy().into_owned())
        .unwrap_or_else(|| config.ui.last_project.clone());
    let app: Shared = Rc::new_cyclic(|me| {
        RefCell::new(App {
        project: Project::default(),
        path: None,
        dirty: false,
        config,
        config_path,
        engine,
        composer: Composer::new(api),
        main: ui.as_weak(),
        status: Status::default(),
        generation: 0,
        editor: None,
        dialogs: Default::default(),
        me: me.clone(),
        confirm: None,
        after_save: None,
        thumbs: HashMap::new(),
        seen_on_air: 0,
        seen_log: 0,
        preview_key: None,
        preview_at: Instant::now(),
        timeline_key: None,
        last_second: 0,
        })
    });

    if !last.is_empty() {
        let path = PathBuf::from(&last);
        match Project::load(&path) {
            Ok(p) => {
                let mut a = app.borrow_mut();
                a.project = p;
                a.path = Some(path);
            }
            Err(e) => log::warn!("no se pudo abrir el último proyecto: {e:#}"),
        }
    }
    app.borrow_mut().loaded();
    wire(&app, &ui);

    let timer = slint::Timer::default();
    {
        let weak = Rc::downgrade(&app);
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || with_app(&weak, App::tick));
    }
    {
        let weak = Rc::downgrade(&app);
        ui.window().on_close_requested(move || {
            let Some(app) = weak.upgrade() else { return slint::CloseRequestResponse::HideWindow };
            if app.borrow().dirty {
                request(&app, Then::Quit);
                slint::CloseRequestResponse::KeepWindowShown
            } else {
                let _ = slint::quit_event_loop();
                slint::CloseRequestResponse::HideWindow
            }
        });
    }
    bind_files(&ui.global::<Files>());
    {
        let hover = ui.as_weak();
        let drop = ui.as_weak();
        hook_os_drops(
            ui.window(),
            move |on| {
                if let Some(ui) = hover.upgrade() {
                    ui.global::<Files>().set_hovering(on);
                }
            },
            move |paths| {
                let Some(ui) = drop.upgrade() else { return };
                if paths.len() == 1 {
                    ui.invoke_drop_project(paths[0].to_string_lossy().into_owned().into());
                }
                ui.invoke_drop_programs(paths_model(&paths));
            },
        );
    }
    apply_theme_to(&ui.global::<Theme>(), app.borrow().config.ui.light);
    ui.show()?;
    slint::run_event_loop()?;
    drop(timer);
    // Dropping the app stops the engine, which releases the PTT.
    drop(app);
    Ok(())
}

fn wire(app: &Shared, ui: &MainWindow) {
    let w = Rc::downgrade(app);

    ui.on_new_project({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                request(&app, Then::New);
            }
        }
    });
    ui.on_open_project({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                request(&app, Then::Open);
            }
        }
    });
    ui.on_save_project({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                save(&app, false, None);
            }
        }
    });
    ui.on_save_project_as({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                save(&app, true, None);
            }
        }
    });
    ui.on_files_picked({
        let w = w.clone();
        move |purpose, paths| {
            if let Some(app) = w.upgrade() {
                files_picked(&app, &purpose, paths.iter().map(|p| PathBuf::from(p.as_str())).collect());
            }
        }
    });
    ui.on_project_settings({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                crate::dialogs::open_project(&app);
            }
        }
    });
    ui.on_open_config({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                crate::dialogs::open_config(&app);
            }
        }
    });
    ui.on_open_utils({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                crate::dialogs::open_utils(&app);
            }
        }
    });
    ui.on_open_editor({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                crate::editor::open(&app);
            }
        }
    });
    ui.on_mode_selected({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                if let Some(&m) = Mode::ALL.get(i as usize) {
                    if a.project.tx.mode != m {
                        a.project.tx.mode = m;
                        a.changed();
                    }
                }
            })
        }
    });
    ui.on_transmit_now({
        let w = w.clone();
        move || with_app(&w, |a| a.engine.send(Command::TransmitNow { program: None }))
    });
    ui.on_stop({
        let w = w.clone();
        move || with_app(&w, |a| a.engine.send(Command::Stop))
    });
    ui.on_set_agenda({
        let w = w.clone();
        move |on| with_app(&w, |a| a.engine.send(Command::SetAgenda(on)))
    });
    ui.on_add_programs({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                crate::dialogs::open_program(&app, None);
            }
        }
    });
    ui.on_edit_program({
        let w = w.clone();
        move |i| {
            if let Some(app) = w.upgrade() {
                crate::dialogs::open_program(&app, Some(i as usize));
            }
        }
    });
    ui.on_pick_files({
        let ui = ui.as_weak();
        move || {
            let ui = ui.clone();
            pick_native(NativePick::Media, move |paths| {
                let Some(ui) = ui.upgrade() else { return };
                if paths.len() == 1 {
                    ui.invoke_drop_project(paths[0].to_string_lossy().into_owned().into());
                }
                ui.invoke_drop_programs(paths_model(&paths));
            });
        }
    });
    ui.on_drop_programs({
        let w = w.clone();
        move |paths| {
            with_app(&w, |a| {
                let files: Vec<PathBuf> = paths.iter().map(|p| PathBuf::from(p.as_str())).filter(|p| is_image(p)).collect();
                if files.is_empty() {
                    return;
                }
                let selected = a.ui().get_selected_program();
                if selected >= 0 && (selected as usize) < a.project.programs.len() {
                    let stored: Vec<String> = files.iter().map(|f| a.relativize(f)).collect();
                    if let Some(p) = a.project.programs.get_mut(selected as usize) {
                        p.images.extend(stored.into_iter().map(estivi_core::project::ProgramImage::new));
                    }
                    a.changed();
                    return;
                }
                add_images_as_program(a, files);
            })
        }
    });
    ui.on_drop_project({
        let w = w.clone();
        move |path| {
            let p = PathBuf::from(path.as_str());
            if !is_project_file(&p) {
                return;
            }
            if let Some(app) = w.upgrade() {
                if app.borrow().dirty {
                    app.borrow_mut().notify(
                        "Hay cambios sin guardar",
                        "Guardá el proyecto actual y volvé a soltar el archivo .estivi.",
                    );
                    return;
                }
                apply_picked(&mut app.borrow_mut(), PICK_OPEN, vec![p]);
            }
        }
    });
    ui.on_set_light({
        let w = w.clone();
        move |light| {
            with_app(&w, |a| {
                a.config.ui.light = light;
                apply_theme_to(&a.ui().global::<Theme>(), light);
                if let Some(ed) = &a.editor {
                    ed.apply_theme(light);
                }
                a.dialogs.apply_theme(light);
                if let Err(e) = a.config.save(&a.config_path) {
                    log::warn!("no se pudo guardar la configuración: {e:#}");
                }
            })
        }
    });
    ui.on_remove_program({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                if (i as usize) < a.project.programs.len() {
                    a.project.programs.remove(i as usize);
                    a.changed();
                }
            })
        }
    });
    ui.on_move_program({
        let w = w.clone();
        move |i, delta| {
            with_app(&w, |a| {
                let (i, j) = (i as usize, (i + delta) as usize);
                if i < a.project.programs.len() && j < a.project.programs.len() {
                    a.project.programs.swap(i, j);
                    a.ui().set_selected_program(j as i32);
                    a.changed();
                }
            })
        }
    });
    ui.on_toggle_program({
        let w = w.clone();
        move |i, on| {
            with_app(&w, |a| {
                if let Some(p) = a.project.programs.get_mut(i as usize) {
                    p.enabled = on;
                    a.changed();
                }
            })
        }
    });
    ui.on_program_selected(|_| {});
    ui.on_transmit_program({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                if let Some(p) = a.project.programs.get(i as usize) {
                    a.engine.send(Command::TransmitNow { program: Some(p.id) });
                }
            })
        }
    });
    ui.on_rename_program({
        let w = w.clone();
        move |i| {
            if let Some(app) = w.upgrade() {
                crate::dialogs::rename_program(&app, i as usize);
            }
        }
    });
    ui.on_rotation_selected({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                a.project.rotation = if i == 1 { Rotation::Shuffle } else { Rotation::Sequential };
                a.changed();
            })
        }
    });
    ui.on_tz_selected({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                a.project.grid.timezone = if i == 1 { GridTz::Local } else { GridTz::Utc };
                a.changed();
            })
        }
    });
    ui.on_add_slot({
        let w = w.clone();
        move || {
            if let Some(app) = w.upgrade() {
                crate::dialogs::open_slot(&app, None);
            }
        }
    });
    ui.on_edit_slot({
        let w = w.clone();
        move |i| {
            if let Some(app) = w.upgrade() {
                crate::dialogs::open_slot(&app, Some(i as usize));
            }
        }
    });
    ui.on_duplicate_slot({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                if let Some(s) = a.project.grid.slots.get(i as usize).cloned() {
                    let id = a.project.next_slot_id();
                    a.project.grid.slots.insert(i as usize + 1, Slot { id, name: format!("{} (copia)", s.name), ..s });
                    a.ui().set_selected_slot(i + 1);
                    a.changed();
                }
            })
        }
    });
    ui.on_remove_slot({
        let w = w.clone();
        move |i| {
            with_app(&w, |a| {
                if (i as usize) < a.project.grid.slots.len() {
                    a.project.grid.slots.remove(i as usize);
                    a.changed();
                }
            })
        }
    });
    ui.on_toggle_slot({
        let w = w.clone();
        move |i, on| {
            with_app(&w, |a| {
                if let Some(s) = a.project.grid.slots.get_mut(i as usize) {
                    s.enabled = on;
                    a.changed();
                }
            })
        }
    });
}
