//! The station window: the grid in the middle, and the panels around it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use estivi_core::air::{self, timeline};
use estivi_core::compose::Composer;
use estivi_core::config::{Config, Lang, PttMethod};
use estivi_core::dynamic::ApiCache;
use estivi_core::engine::{Command, Engine, Phase, Status};
use estivi_core::export::{self, AudioFormat, ExportRequest, Shape};
use estivi_core::modes::Mode;
use estivi_core::playlist::{self};
use estivi_core::project::{
    self, ActTrigger, Action, Activation, Block, BlockImage, Carrier, GridTz, ImageRole, Program, Project, Still, TriggerSource,
    format_hhmm, parse_hhmm, relativize, resolve,
};
use estivi_core::schedule::{self, Card, Keep, Occurrence, Zoom, future_limit, grid_date, proposed_window, to_utc};

use crate::editor;
use crate::i18n;
use crate::view;
use crate::{CardItem, Files, MainWindow, ModeCheat, PicItem, StepItem, Theme, UtilsDialog};

pub type Shared = Rc<RefCell<App>>;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];

pub fn bind_files(files: &Files) {
    files.on_has_files(|dt| dt.has_file_paths() || dt.has_plain_text());
    files.on_paths_of(|dt| ModelRc::new(VecModel::from(paths_from_transfer(&dt))));
}

pub fn hook_os_drops(window: &slint::Window, on_hover: impl Fn(bool) + 'static, on_drop: impl Fn(Vec<PathBuf>) + 'static) {
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
    ModelRc::new(VecModel::from(paths.iter().map(|p| SharedString::from(p.to_string_lossy().into_owned())).collect::<Vec<_>>()))
}

#[derive(Clone, Copy)]
pub enum NativePick {
    Images { many: bool },
    ProjectOpen,
    ProjectSave,
    Directory,
}

pub fn pick_native(kind: NativePick, done: impl FnOnce(Vec<PathBuf>) + Send + 'static) {
    std::thread::spawn(move || {
        let ext = project::EXTENSION;
        let paths = match kind {
            NativePick::Images { many: true } => rfd::FileDialog::new()
                .set_title(i18n::t("Choose pictures", "Elegir imágenes"))
                .add_filter(&i18n::t("Pictures", "Imágenes"), &IMAGE_EXTS)
                .pick_files()
                .unwrap_or_default(),
            NativePick::Images { many: false } => rfd::FileDialog::new()
                .set_title(i18n::t("Choose picture", "Elegir imagen"))
                .add_filter(&i18n::t("Pictures", "Imágenes"), &IMAGE_EXTS)
                .pick_file()
                .into_iter()
                .collect(),
            NativePick::ProjectOpen => rfd::FileDialog::new()
                .set_title(i18n::t("Open project", "Abrir proyecto"))
                .add_filter(&i18n::t("Estivi project", "Proyecto Estivi"), &[ext])
                .pick_file()
                .into_iter()
                .collect(),
            NativePick::ProjectSave => rfd::FileDialog::new()
                .set_title(i18n::t("Save project", "Guardar proyecto"))
                .add_filter(&i18n::t("Estivi project", "Proyecto Estivi"), &[ext])
                .save_file()
                .into_iter()
                .collect(),
            NativePick::Directory => rfd::FileDialog::new()
                .set_title(i18n::t("Choose folder", "Elegir carpeta"))
                .pick_folder()
                .into_iter()
                .collect(),
        };
        let _ = slint::invoke_from_event_loop(move || done(paths));
    });
}

fn paths_from_transfer(dt: &slint::DataTransfer) -> Vec<SharedString> {
    let mut out: Vec<SharedString> = dt.file_paths().ok().into_iter().flatten().map(|p| p.to_string_lossy().into_owned().into()).collect();
    if out.is_empty() {
        if let Ok(text) = dt.plain_text() {
            out.extend(paths_from_uri_list(&text));
        }
    }
    out
}

fn paths_from_uri_list(text: &str) -> Vec<SharedString> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let path = if let Some(rest) = line.strip_prefix("file://") {
                percent_decode(rest.strip_prefix("localhost").unwrap_or(rest))
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

pub fn with_app(weak: &Weak<RefCell<App>>, f: impl FnOnce(&mut App)) {
    if let Some(app) = weak.upgrade() {
        if let Ok(mut a) = app.try_borrow_mut() {
            f(&mut a);
        }
    }
}

#[derive(Clone, Copy)]
enum After {
    Nothing,
    Quit,
    New,
    Open,
}

enum Pending {
    None,
    Message,
    Create,
    Weekday { id: u64, day: usize },
    DeleteProgram { id: u64 },
    DeleteBlock { id: u64 },
    Export,
    Quality { req: ExportRequest, dir: PathBuf },
    Clock,
    Activation(Option<u64>),
    Leave(After),
}

enum Focus {
    None,
    Card(usize),
    Program(u64),
}

struct InstantHold {
    welcome: Option<Still>,
    bye: Option<Still>,
}

pub struct App {
    pub project: Project,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub config: Config,
    pub config_path: PathBuf,
    pub engine: Engine,
    pub composer: Composer,
    pub status: Status,
    pub editor: Option<crate::editor::Editor>,
    main: slint::Weak<MainWindow>,
    modes: Option<UtilsDialog>,
    cards: Vec<Card>,
    focus: Focus,
    pic: Option<u64>,
    zoom: i32,
    anchor: NaiveDate,
    pending: Pending,
    after: After,
    act_at: usize,
    prog_at: usize,
    instant: InstantHold,
    seen_air: u64,
    seen_log: u64,
}

impl App {
    pub fn ui(&self) -> MainWindow {
        self.main.upgrade().expect("main window")
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

    fn es(&self) -> bool {
        matches!(i18n::get(), Lang::Spanish)
    }

    fn now(&self) -> chrono::DateTime<Utc> {
        Utc::now() + TimeDelta::milliseconds(self.status.clock_offset_ms)
    }

    fn today(&self) -> NaiveDate {
        grid_date(self.project.grid.timezone, self.now())
    }

    fn send_project(&self) {
        self.engine.send(Command::SetProject { project: Box::new(self.project.clone()), path: self.path.clone() });
    }

    fn save_config(&mut self) {
        if let Err(e) = self.config.save(&self.config_path) {
            log::warn!("config: {e:#}");
        }
        self.engine.send(Command::SetConfig(Box::new(self.config.clone())));
    }

    pub fn graphics_changed(&mut self, notify: bool) {
        self.dirty = true;
        if notify {
            self.send_project();
        }
        self.ui().set_dirty(true);
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.send_project();
        self.ui().set_dirty(true);
        self.refresh_cards();
        self.push_block(false);
        self.push_lists();
    }

    pub fn loaded(&mut self) {
        self.dirty = false;
        self.composer.set_project_dir(self.dir().as_deref());
        self.send_project();
        self.config.ui.last_project = self.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = self.config.save(&self.config_path);
        self.push_channel();
        self.push_equipment();
        self.refresh_cards();
        self.push_block(true);
        self.push_lists();
        self.ui().set_dirty(false);
        if !self.config.time.prompted {
            self.open_clock();
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        self.ui().set_note(text.into().into());
    }

    fn close_dialog(&mut self) {
        self.pending = Pending::None;
        let ui = self.ui();
        ui.set_dialog(0);
        ui.set_dialog_body("".into());
    }

    fn ask(&mut self, kind: i32, title: &str, body: &str, ok: &str, alt: &str, extra: &str, pending: Pending) {
        self.pending = pending;
        let ui = self.ui();
        ui.set_dialog_title(title.into());
        ui.set_dialog_body(body.into());
        ui.set_dialog_ok(ok.into());
        ui.set_dialog_alt(alt.into());
        ui.set_dialog_extra(extra.into());
        ui.set_dialog(kind);
    }

    fn message(&mut self, body: impl Into<String>) {
        let body = body.into();
        self.ask(1, &i18n::t("Notice", "Aviso"), &body, &i18n::t("OK", "Aceptar"), "", "", Pending::Message);
    }

    fn tick(&mut self) {
        self.status = self.engine.status();
        self.paint_status();
        if let Some(mut ed) = self.editor.take() {
            ed.tick(self);
            self.editor = Some(ed);
        }
    }

    fn paint_status(&mut self) {
        let ui = self.ui();
        let s = &self.status;
        let on = if let Some(air) = &s.on_air {
            i18n::t(
                &format!("{} · {}", air.program, air.mode.name()),
                &format!("{} · {}", air.program, air.mode.name()),
            )
        } else {
            match s.phase {
                Phase::Keying => i18n::ts("Keying", "Porteando"),
                Phase::Tail => i18n::ts("Tail", "Cola"),
                Phase::OnAir => i18n::ts("On air", "Al aire"),
                Phase::Idle if s.agenda => i18n::ts("Armed", "Armada"),
                Phase::Idle => i18n::ts("Standby", "En espera"),
            }
            .into()
        };
        ui.set_on_air(on.into());
        let next = match &s.next {
            Some(n) => format!("{} · {}", n.name, view::when(n.at, s.clock_offset_ms)),
            None => i18n::t("Nothing queued", "Nada en cola"),
        };
        ui.set_next_up(next.into());
        ui.set_agenda(s.agenda);
        ui.set_busy(s.busy);
        ui.set_busy_reason(s.busy_reason.as_str().into());
        ui.set_instant_ok(!s.busy);
        ui.set_instant_why(s.busy_reason.as_str().into());
        ui.set_clock_note(view::clock_line(s.clock_offset_ms, &s.clock_error).into());
        if let Some(err) = &s.error {
            ui.set_note(err.as_str().into());
        }
        if s.on_air_rev != self.seen_air {
            self.seen_air = s.on_air_rev;
            if let Some(img) = s.on_air.as_ref().and_then(|a| a.image.as_ref()) {
                ui.set_on_air_image(view::to_image(img));
                ui.set_has_image(true);
            } else {
                ui.set_has_image(false);
            }
        }
        if s.log_rev != self.seen_log {
            self.seen_log = s.log_rev;
            let line = s.log.last().map(|e| format!("#{} {} {}", e.serial, e.program, e.result)).unwrap_or_default();
            ui.set_log_text(line.into());
        }
        let listen = format!(
            "CW {}   DTMF {}   {:+.0} dB",
            if s.listen.cw_text.is_empty() { "—" } else { s.listen.cw_text.as_str() },
            if s.listen.dtmf_text.is_empty() { "—" } else { s.listen.dtmf_text.as_str() },
            s.listen.level_db
        );
        ui.set_listen_text(listen.into());
    }

    fn zoom(&self) -> Zoom {
        match self.zoom {
            1 => Zoom::Week,
            2 => Zoom::Day,
            _ => Zoom::Month,
        }
    }

    fn grid_origin(&self) -> NaiveDate {
        match self.zoom() {
            Zoom::Month => {
                let first = self.anchor.with_day(1).unwrap_or(self.anchor);
                first - TimeDelta::days(i64::from(first.weekday().num_days_from_monday()))
            }
            Zoom::Week => self.anchor - TimeDelta::days(i64::from(self.anchor.weekday().num_days_from_monday())),
            Zoom::Day => self.anchor,
        }
    }

    fn cell_date(&self, cell: i32) -> NaiveDate {
        self.grid_origin() + TimeDelta::days(i64::from(cell.max(0)))
    }

    fn refresh_cards(&mut self) {
        let today = self.today();
        let mut anchor = self.anchor;
        if self.zoom() != Zoom::Month && anchor > future_limit(today) {
            anchor = future_limit(today);
        }
        let cards = match schedule::cards(&self.project, self.zoom(), anchor, today) {
            Ok(c) => c,
            Err(_) => {
                self.note(i18n::t("The grid stops three years ahead.", "La grilla llega hasta tres años adelante."));
                return;
            }
        };
        self.anchor = anchor;
        let origin = self.grid_origin();
        let month = self.anchor.month();
        let mut heads = Vec::new();
        let n = if self.zoom == 0 { 42 } else if self.zoom == 1 { 7 } else { 1 };
        for i in 0..n {
            let date = origin + TimeDelta::days(i);
            let label = if self.zoom == 0 {
                let day = date.format("%d").to_string();
                if date.month() == month { day } else { format!("· {day}") }
            } else {
                view::day_heading(date)
            };
            heads.push(SharedString::from(label));
        }
        let today_cell = (today - origin).num_days();
        let today_cell = if (0..i64::from(n)).contains(&today_cell) { today_cell as i32 } else { -1 };
        let items: Vec<CardItem> = cards
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let cell = if self.zoom == 0 {
                    (c.date - origin).num_days() as i32
                } else {
                    c.col
                };
                CardItem {
                    idx: i as i32,
                    cell,
                    stack: c.stack,
                    name: c.name.as_str().into(),
                    hours: format!("{}–{}", c.start_hm, c.end_hm).into(),
                    y: c.y,
                    h: c.h.max(0.01),
                    r: i32::from(c.rgb[0]),
                    g: i32::from(c.rgb[1]),
                    b: i32::from(c.rgb[2]),
                    opacity: c.opacity,
                    empty: c.empty,
                }
            })
            .collect();
        self.cards = cards;
        if let Focus::Card(i) = self.focus {
            if i >= self.cards.len() {
                self.focus = Focus::None;
            }
        }
        let ui = self.ui();
        ui.set_cards(ModelRc::new(VecModel::from(items)));
        ui.set_heads(ModelRc::new(VecModel::from(heads)));
        ui.set_columns(if self.zoom == 2 { 1 } else { 7 });
        ui.set_today_cell(today_cell);
        ui.set_period(self.period_label().into());
        ui.set_zoom(self.zoom);
    }

    fn period_label(&self) -> String {
        let months_en = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        let months_es = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];
        match self.zoom() {
            Zoom::Month => {
                let name = if self.es() { months_es } else { months_en }[(self.anchor.month() - 1) as usize];
                format!("{name} {}", self.anchor.year())
            }
            Zoom::Week => {
                let a = self.grid_origin();
                let b = a + TimeDelta::days(6);
                format!("{} – {}", a.format("%d %b"), b.format("%d %b %Y"))
            }
            Zoom::Day => self.anchor.format("%A %d %B %Y").to_string(),
        }
    }

    fn card(&self) -> Option<&Card> {
        match self.focus {
            Focus::Card(i) => self.cards.get(i),
            _ => None,
        }
    }

    fn occurrence_of(&self, card: &Card) -> Option<Occurrence> {
        let today = self.today();
        let from = card.date - TimeDelta::days(1);
        let to = card.date + TimeDelta::days(1);
        schedule::occurrences(&self.project, from, to, today).into_iter().find(|o| {
            let pid = o.program_id.map(|id| id as i64).unwrap_or(-1);
            let bid = o.block_id.map(|id| id as i64).unwrap_or(-1);
            pid == card.program_id && o.start_hm == card.start_hm && (o.date == card.date || o.date + TimeDelta::days(1) == card.date) && (card.block_id < 0 || bid == card.block_id)
        })
    }

    fn focused_program(&self) -> Option<u64> {
        match self.focus {
            Focus::Program(id) => Some(id),
            Focus::Card(i) => self.cards.get(i).and_then(|c| (c.program_id >= 0).then_some(c.program_id as u64)),
            Focus::None => None,
        }
    }

    fn images_now(&self) -> Vec<BlockImage> {
        self.card().and_then(|c| self.occurrence_of(c)).map(|o| o.images).unwrap_or_default()
    }

    fn commit_images(&mut self, images: Vec<BlockImage>) {
        if let Err(e) = project::check_images(&images) {
            self.note(e.to_string());
            return;
        }
        let Some(card) = self.card().cloned() else {
            self.note(i18n::t("Pick a day on the grid first.", "Elegí un día en la grilla primero."));
            return;
        };
        let has = images.iter().any(|i| !i.path.trim().is_empty());
        if card.program_id >= 0 {
            let pid = card.program_id as u64;
            if !has {
                if card.block_id >= 0 {
                    let id = card.block_id as u64;
                    self.project.blocks.retain(|b| b.id != id);
                }
            } else if card.block_id >= 0 {
                if let Some(b) = self.project.block_mut(card.block_id as u64) {
                    b.images = images;
                }
            } else {
                let block = self.project.ensure_program_block(pid, card.date);
                block.images = images;
            }
        } else if card.block_id >= 0 {
            if let Some(b) = self.project.block_mut(card.block_id as u64) {
                b.images = images;
            }
        }
        self.changed();
    }

    fn push_block(&mut self, texts: bool) {
        let ui = self.ui();
        let card = self.card().cloned();
        let program_id = self.focused_program();
        let is_program = program_id.is_some();
        ui.set_has_block(card.is_some() || program_id.is_some());
        ui.set_is_program(is_program);
        if let Some(id) = program_id {
            if let Some(p) = self.project.program(id) {
                ui.set_program_on(p.enabled);
                ui.set_form_hue(i32::from(p.hue % 360));
                ui.set_wd0(p.weekdays[0]);
                ui.set_wd1(p.weekdays[1]);
                ui.set_wd2(p.weekdays[2]);
                ui.set_wd3(p.weekdays[3]);
                ui.set_wd4(p.weekdays[4]);
                ui.set_wd5(p.weekdays[5]);
                ui.set_wd6(p.weekdays[6]);
                if texts {
                    ui.set_form_ends(p.ends.map(|t| grid_date(self.project.grid.timezone, t - TimeDelta::seconds(1)).format("%Y-%m-%d").to_string()).unwrap_or_default().into());
                    ui.set_form_welcome(still_label(p.welcome.as_ref()).into());
                    ui.set_form_bye(still_label(p.bye.as_ref()).into());
                    if card.is_none() {
                        ui.set_form_name(p.name.as_str().into());
                        ui.set_form_start(p.start.as_str().into());
                        ui.set_form_end(p.end.as_str().into());
                    }
                }
                if card.is_none() {
                    ui.set_block_title(i18n::t(&format!("{} · off the grid until it has a day and hours", p.name), &format!("{} · fuera de la grilla hasta tener día y horario", p.name)).into());
                }
            }
        }
        let Some(card) = card else {
            ui.set_pics(ModelRc::new(VecModel::from(Vec::<PicItem>::new())));
            ui.set_steps(ModelRc::new(VecModel::from(Vec::<StepItem>::new())));
            ui.set_tx_total("".into());
            if program_id.is_none() {
                ui.set_block_title("".into());
            }
            return;
        };
        if texts {
            ui.set_form_name(card.name.as_str().into());
            ui.set_form_start(card.start_hm.as_str().into());
            ui.set_form_end(card.end_hm.as_str().into());
        }
        ui.set_block_title(format!("{}  {}  {}", card.name, card.date.format("%Y-%m-%d"), format!("{}–{}", card.start_hm, card.end_hm)).into());
        let occ = self.occurrence_of(&card);
        ui.set_block_on(occ.as_ref().map(|o| o.block_enabled).unwrap_or(true));
        let pics: Vec<PicItem> = occ
            .as_ref()
            .map(|o| {
                o.images
                    .iter()
                    .map(|im| PicItem {
                        id: im.id as i32,
                        label: view::file_name(&im.path).into(),
                        detail: format!("{} · {}", view::role_name(im), im.mode.map(|m| m.spec().short).unwrap_or("·")).into(),
                        on: im.enabled,
                        role: match im.role {
                            ImageRole::Pre { .. } => 1,
                            ImageRole::Post { .. } => 2,
                            ImageRole::Normal => 0,
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();
        ui.set_pics(ModelRc::new(VecModel::from(pics)));
        let (steps, total) = if let Some(occ) = &occ {
            let turns = air::plan_occurrence(&self.project, occ, self.today(), self.config.ptt.lead_ms, self.config.ptt.tail_ms, self.status.serial, self.now());
            let rows = timeline(&turns, occ.start, occ.end);
            let aired: f64 = rows.iter().filter(|r| r.kind == air::RowKind::Total && r.airs).map(|r| r.ms).sum();
            let steps = rows
                .iter()
                .map(|r| StepItem { label: view::row_label(r).into(), time: view::duration(r.ms).into(), dim: !r.airs })
                .collect();
            (steps, i18n::t(&format!("On air {}", view::duration(aired)), &format!("Al aire {}", view::duration(aired))))
        } else {
            (Vec::new(), String::new())
        };
        ui.set_steps(ModelRc::new(VecModel::from(steps)));
        ui.set_tx_total(total.into());
    }

    fn push_lists(&mut self) {
        let programs: Vec<String> = self.project.programs.iter().map(|p| {
            if p.scheduled() { p.name.clone() } else { format!("{} · {}", p.name, i18n::t("no schedule", "sin horario")) }
        }).collect();
        let act: Vec<String> = self.project.activations.iter().enumerate().map(|(i, a)| {
            let mark = if i == self.act_at { "▸ " } else { "  " };
            let code = a.triggers.first().map(|t| t.code.as_str()).unwrap_or("—");
            format!("{mark}{} {}", a.name, code)
        }).collect();
        let ui = self.ui();
        ui.set_program_list(programs.join("\n").into());
        ui.set_act_list(if act.is_empty() { i18n::t("No activations yet.", "Todavía no hay activaciones.").into() } else { act.join("\n").into() });
    }

    fn push_channel(&mut self) {
        let p = &self.project;
        let ui = self.ui();
        ui.set_project_name(p.name.as_str().into());
        ui.set_callsign(p.station.call.as_str().into());
        ui.set_channel_name(p.name.as_str().into());
        ui.set_call_text(p.station.call.as_str().into());
        ui.set_op_text(p.station.operator.as_str().into());
        ui.set_qth_text(p.station.qth.as_str().into());
        ui.set_loc_text(p.station.locator.as_str().into());
        ui.set_freq_text(view::freq(p.freq_hz).into());
        ui.set_interval(p.interval_min as i32);
        ui.set_offset_s(p.grid.offset_s as i32);
        ui.set_mode_index(view::mode_index(p.tx.mode));
        ui.set_modes(ModelRc::new(VecModel::from(view::mode_names())));
        ui.set_pause_s(p.tx.min_pause_s as i32);
        ui.set_vox(p.tx.vox_preamble);
        ui.set_cw_pre(p.tx.cw_preamble);
        ui.set_cw_id(p.tx.cw_id);
        ui.set_cw_text(p.tx.cw_text.as_str().into());
        ui.set_ax_info(p.ax25.info);
        ui.set_ax_mode(p.ax25.mode);
        ui.set_next_count(p.ax25.next_count as i32);
        ui.set_next_hours(p.ax25.next_hours as i32);
        ui.set_dedupe(p.ax25.dedupe);
        ui.set_tz_index(if matches!(p.grid.timezone, GridTz::Local) { 1 } else { 0 });
        let mp3 = export::format_block(AudioFormat::Mp3, self.es()).unwrap_or_default();
        let aac = export::format_block(AudioFormat::Aac, self.es()).unwrap_or_default();
        ui.set_exp_note(format!("{mp3}\n{aac}").into());
    }

    fn push_equipment(&mut self) {
        let c = &self.config;
        let ui = self.ui();
        ui.set_ptt_index(match c.ptt.method {
            PttMethod::None => 0,
            PttMethod::Vox => 1,
            PttMethod::Serial => 2,
            PttMethod::Cat => 3,
        });
        ui.set_ptt_port(c.ptt.port.as_str().into());
        ui.set_lead_ms(c.ptt.lead_ms as i32);
        ui.set_tail_ms(c.ptt.tail_ms as i32);
        ui.set_cat_host(c.cat.host.as_str().into());
        ui.set_cat_port(i32::from(c.cat.port));
        ui.set_cat_freq(c.cat.set_freq);
        ui.set_audio_out(c.audio.output.as_str().into());
        ui.set_audio_in(c.audio.input.as_str().into());
        ui.set_lang_index(if matches!(Lang::resolve(&c.ui.lang), Lang::Spanish) { 1 } else { 0 });
        ui.set_ntp_on(c.time.enabled);
        ui.set_ntp_server(c.time.server.as_str().into());
        ui.set_ntp_hours(c.time.period_h as i32);
        ui.set_ptt_note(i18n::t(
            "Audio, PTT and the clock belong to this computer. The channel lives in the .estivi file.",
            "El audio, el PTT y el reloj son de esta computadora. El canal vive en el archivo .estivi.",
        ).into());
    }

    fn set_hours(&mut self, start: Option<String>, end: Option<String>) {
        let today = self.today();
        if let Some(id) = self.focused_program() {
            let Some(current) = self.project.program(id).cloned() else { return };
            let mut next = current.clone();
            if let Some(s) = start {
                if parse_hhmm(&s).is_none() && !s.trim().is_empty() {
                    self.note(i18n::t("Use HH:MM.", "Usá HH:MM."));
                    self.push_block(true);
                    return;
                }
                next.start = s;
            }
            if let Some(e) = end {
                if parse_hhmm(&e).is_none() && !e.trim().is_empty() {
                    self.note(i18n::t("Use HH:MM.", "Usá HH:MM."));
                    self.push_block(true);
                    return;
                }
                next.end = e;
            }
            if let Some(c) = schedule::program_conflict(&self.project, &next, Some(id), today) {
                self.message(c.text(self.es()));
                self.push_block(true);
                return;
            }
            if let Some(p) = self.project.program_mut(id) {
                p.start = next.start;
                p.end = next.end;
            }
            self.changed();
            return;
        }
        let Some(card) = self.card().cloned() else { return };
        if card.block_id < 0 {
            return;
        }
        let id = card.block_id as u64;
        let Some(block) = self.project.block(id).cloned() else { return };
        let mut start_hm = block.start.clone();
        let mut end_hm = block.end.clone();
        if let Some(s) = start { start_hm = s; }
        if let Some(e) = end { end_hm = e; }
        if parse_hhmm(&start_hm).is_none() || parse_hhmm(&end_hm).is_none() {
            self.note(i18n::t("Use HH:MM.", "Usá HH:MM."));
            self.push_block(true);
            return;
        }
        if let Some(c) = schedule::span_conflict(&self.project, block.date, &start_hm, &end_hm, Some(id), None, today) {
            self.message(c.text(self.es()));
            self.push_block(true);
            return;
        }
        if let Some(b) = self.project.block_mut(id) {
            b.start = start_hm;
            b.end = end_hm;
        }
        self.changed();
    }

    fn set_edge(&mut self, index: i32, is_end: bool, minute: i32) {
        if !(0..24 * 60).contains(&minute) {
            return;
        }
        let Some(card) = self.cards.get(index as usize).cloned() else { return };
        self.focus = Focus::Card(index as usize);
        let hm = format_hhmm(minute as u32);
        if is_end {
            self.set_hours(None, Some(hm));
        } else {
            self.set_hours(Some(hm), None);
        }
        let _ = card;
    }

    fn open_create(&mut self, cell: i32, minute: i32) {
        let date = self.cell_date(cell);
        let (start, end) = proposed_window(minute.max(0) as u32, self.project.interval_min);
        let ui = self.ui();
        ui.set_dlg_name(i18n::t("New block", "Bloque nuevo").into());
        ui.set_dlg_date(date.format("%Y-%m-%d").to_string().into());
        ui.set_dlg_start(start.into());
        ui.set_dlg_end(end.into());
        self.ask(
            2,
            &i18n::t("Empty hour", "Hora libre"),
            &i18n::t(
                "Nothing is created until you choose a common block or a program. A program repeats on this weekday.",
                "No se crea nada hasta que elijas un bloque común o un programa. Un programa se repite este día de la semana.",
            ),
            &i18n::t("Common block", "Bloque común"),
            &i18n::t("Program", "Programa"),
            &i18n::t("Cancel", "Cancelar"),
            Pending::Create,
        );
    }

    fn confirm_create(&mut self, which: i32) {
        if which == 2 {
            self.close_dialog();
            return;
        }
        let ui = self.ui();
        let name = ui.get_dlg_name().to_string();
        let date = match NaiveDate::parse_from_str(ui.get_dlg_date().as_str(), "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => {
                self.note(i18n::t("The date is YYYY-MM-DD.", "La fecha es AAAA-MM-DD."));
                return;
            }
        };
        let start = ui.get_dlg_start().to_string();
        let end = ui.get_dlg_end().to_string();
        if parse_hhmm(&start).is_none() || parse_hhmm(&end).is_none() {
            self.note(i18n::t("Use HH:MM.", "Usá HH:MM."));
            return;
        }
        let today = self.today();
        let es = self.es();
        if which == 0 {
            if let Err(c) = schedule::can_place_common(&self.project, date, &start, &end, today) {
                self.message(c.text(es));
                return;
            }
            let id = self.project.next_id();
            self.project.blocks.push(Block { id, program: None, date, name, start, end, enabled: true, images: Vec::new() });
        } else {
            if let Err(c) = schedule::can_place_program(&self.project, date, &start, &end, today) {
                self.message(c.text(es));
                return;
            }
            let mut weekdays = [false; 7];
            weekdays[date.weekday().num_days_from_monday() as usize] = true;
            let id = self.project.next_id();
            self.project.programs.push(Program { id, name, weekdays, start, end, ..Program::default() });
            self.focus = Focus::Program(id);
        }
        self.close_dialog();
        self.changed();
    }

    fn weekday(&mut self, day: usize, on: bool) {
        let Some(id) = self.focused_program() else { return };
        let today = self.today();
        let now = self.now();
        match schedule::set_weekday(&mut self.project, id, day, on, None, now, today) {
            Ok(()) => self.changed(),
            Err(schedule::EditError::NeedChoice) => {
                let ui = self.ui();
                ui.set_dlg_horizon(future_limit(today).format("%Y-%m-%d").to_string().into());
                self.ask(
                    3,
                    &i18n::t("Weekday", "Día de la semana"),
                    &i18n::t(
                        "This weekday is about to be turned off. Past blocks stay. Choose what happens to the future ones. Nothing changes until you choose.",
                        "Este día está por apagarse. Los bloques pasados quedan. Elegí qué pasa con los futuros. No cambia nada hasta que elijas.",
                    ),
                    &i18n::t("Delete future pictures of this day", "Borrar las fotos futuras de este día"),
                    &i18n::t("Keep pictures as common blocks", "Conservar las fotos como bloques comunes"),
                    &i18n::t("Also keep empty days until the date", "Conservar también los días vacíos hasta la fecha"),
                    Pending::Weekday { id, day },
                );
            }
            Err(schedule::EditError::Conflict(c)) => {
                self.message(c.text(self.es()));
                self.push_block(true);
            }
            Err(schedule::EditError::Missing) => self.push_block(true),
        }
    }

    fn finish_choice(&mut self, which: i32) {
        let today = self.today();
        let now = self.now();
        let horizon = self.horizon();
        let pending = std::mem::replace(&mut self.pending, Pending::None);
        let result = match pending {
            Pending::Weekday { id, day } => {
                let keep = match which {
                    0 => Keep::Discard,
                    1 => Keep::KeepFilled,
                    _ => Keep::KeepThrough(horizon.unwrap_or_else(|| future_limit(today))),
                };
                schedule::set_weekday(&mut self.project, id, day, false, Some(keep), now, today)
            }
            Pending::DeleteProgram { id } => {
                let keep = match which {
                    0 => Keep::Discard,
                    1 => Keep::KeepFilled,
                    _ => Keep::KeepThrough(horizon.unwrap_or_else(|| future_limit(today))),
                };
                if matches!(self.focus, Focus::Program(pid) if pid == id) {
                    self.focus = Focus::None;
                }
                schedule::delete_program(&mut self.project, id, keep, now, today)
            }
            _ => Ok(()),
        };
        self.close_dialog();
        if let Err(schedule::EditError::Conflict(c)) = result {
            self.message(c.text(self.es()));
        }
        self.changed();
    }

    fn horizon(&self) -> Option<NaiveDate> {
        let text = self.ui().get_dlg_horizon().to_string();
        let date = NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()?;
        let cap = future_limit(self.today());
        Some(if date > cap { cap } else { date })
    }

    fn open_delete_program(&mut self) {
        let Some(id) = self.focused_program() else { return };
        let today = self.today();
        self.ui().set_dlg_horizon(future_limit(today).format("%Y-%m-%d").to_string().into());
        self.ask(
            3,
            &i18n::t("Delete program", "Borrar programa"),
            &i18n::t(
                "The program is about to disappear. Choose what happens to its blocks. Nothing is deleted until you choose.",
                "El programa está por desaparecer. Elegí qué pasa con sus bloques. No se borra nada hasta que elijas.",
            ),
            &i18n::t("Delete every block", "Borrar todos los bloques"),
            &i18n::t("Keep blocks that have pictures", "Conservar los bloques con fotos"),
            &i18n::t("Keep every day until the date", "Conservar todos los días hasta la fecha"),
            Pending::DeleteProgram { id },
        );
    }

    fn open_export(&mut self) {
        let today = self.today();
        let ui = self.ui();
        ui.set_exp_from(format!("{} 00:00", today.format("%Y-%m-%d")).into());
        ui.set_exp_to(format!("{} 01:00", today.format("%Y-%m-%d")).into());
        ui.set_exp_shape(1);
        ui.set_exp_rate(2);
        self.ask(
            4,
            &i18n::t("Export audio", "Exportar audio"),
            &i18n::t(
                "The files add up to the range. Press play on each one at the time in its name. FLAC is the format this program writes.",
                "Los archivos suman el rango. Apretá play en cada uno a la hora que dice el nombre. FLAC es el formato que escribe este programa.",
            ),
            &i18n::t("Export", "Exportar"),
            &i18n::t("Cancel", "Cancelar"),
            "",
            Pending::Export,
        );
    }

    fn read_export(&self) -> Option<ExportRequest> {
        let ui = self.ui();
        let start = parse_local(&ui.get_exp_from(), self.project.grid.timezone)?;
        let end = parse_local(&ui.get_exp_to(), self.project.grid.timezone)?;
        let shape = match ui.get_exp_shape() {
            0 => Shape::Whole,
            2 => Shape::Pieces,
            3 => Shape::Every(ui.get_exp_slice().max(1) as u32),
            4 => Shape::PerBlock,
            _ => Shape::TxSilence,
        };
        let rate = export::SAMPLE_RATES.get(ui.get_exp_rate().max(0) as usize).copied().unwrap_or(44_100);
        Some(ExportRequest { start, end, shape, format: AudioFormat::Flac, rate, lead_ms: self.config.ptt.lead_ms, tail_ms: self.config.ptt.tail_ms })
    }

    fn launch_export(&mut self, req: ExportRequest, dir: PathBuf) {
        let project = self.project.clone();
        let base = self.dir();
        let weak_main = self.main.clone();
        self.note(i18n::t("Exporting…", "Exportando…"));
        std::thread::spawn(move || {
            let result = export::write_dir(&project, base.as_deref(), &dir, &req);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(ui) = weak_main.upgrade() else { return };
                match result {
                    Ok(paths) => ui.set_note(format!("{} {}", paths.len(), i18n::t("files", "archivos")).into()),
                    Err(e) => ui.set_note(e.to_string().into()),
                }
            });
        });
    }

    fn open_clock(&mut self) {
        self.ask(
            6,
            &i18n::t("Clock", "Reloj"),
            &i18n::t(
                "Estivi can follow an NTP server and store the difference. The computer clock is not changed. If the server fails later, the last offset is kept and transmission is not blocked.",
                "Estivi puede seguir un servidor NTP y guardar la diferencia. El reloj de la computadora no se toca. Si el servidor falla después, se conserva el último desfase y la transmisión no se bloquea.",
            ),
            &i18n::t("Use the network clock", "Usar el reloj de la red"),
            &i18n::t("Don't ask again", "No volver a preguntar"),
            "",
            Pending::Clock,
        );
    }

    fn open_activation(&mut self, new: bool) {
        let ui = self.ui();
        if new {
            ui.set_act_name("".into());
            ui.set_act_code("".into());
            ui.set_act_script("".into());
            ui.set_act_source(0);
            ui.set_act_on(true);
            self.ask(7, &i18n::t("Activation", "Activación"), &i18n::t("It is saved only when you press Save. A trigger without a code is not stored.", "Se guarda solo cuando apretás Guardar. Un disparador sin código no se guarda."), &i18n::t("Save", "Guardar"), &i18n::t("Cancel", "Cancelar"), "", Pending::Activation(None));
            return;
        }
        let Some(act) = self.project.activations.get(self.act_at).cloned() else {
            self.note(i18n::t("There is no activation to edit.", "No hay una activación para editar."));
            return;
        };
        ui.set_act_name(act.name.as_str().into());
        ui.set_act_on(act.enabled);
        ui.set_act_source(if matches!(act.triggers.first().map(|t| t.source), Some(TriggerSource::Cw)) { 1 } else { 0 });
        ui.set_act_code(act.triggers.first().map(|t| t.code.as_str()).unwrap_or("").into());
        ui.set_act_script(script_of(&act.actions).into());
        self.ask(7, &i18n::t("Activation", "Activación"), "", &i18n::t("Save", "Guardar"), &i18n::t("Cancel", "Cancelar"), "", Pending::Activation(Some(act.id)));
    }

    fn save_activation(&mut self, id: Option<u64>) {
        let ui = self.ui();
        let name = ui.get_act_name().to_string().trim().to_string();
        let code = ui.get_act_code().to_string().trim().to_string();
        if name.is_empty() || code.is_empty() {
            self.note(i18n::t("Write a name and the code the radio must hear.", "Escribí un nombre y el código que tiene que oír la radio."));
            return;
        }
        let actions = match parse_script(&ui.get_act_script(), self.project.tx.cw_wpm, self.project.tx.cw_tone_hz) {
            Ok(a) => a,
            Err(e) => {
                self.note(e);
                return;
            }
        };
        let trigger = ActTrigger { source: if ui.get_act_source() == 1 { TriggerSource::Cw } else { TriggerSource::Dtmf }, code };
        if let Some(id) = id {
            if let Some(act) = self.project.activations.iter_mut().find(|a| a.id == id) {
                act.name = name;
                act.enabled = ui.get_act_on();
                act.triggers = vec![trigger];
                act.actions = actions;
            }
        } else {
            let id = self.project.next_id();
            self.project.activations.push(Activation { id, enabled: ui.get_act_on(), name, triggers: vec![trigger], actions });
            self.act_at = self.project.activations.len().saturating_sub(1);
        }
        self.close_dialog();
        self.changed();
    }

    fn transmit(&mut self) {
        if self.status.busy {
            self.note(self.status.busy_reason.clone());
            return;
        }
        let ui = self.ui();
        let path = ui.get_instant_path().to_string();
        if path.trim().is_empty() {
            self.note(i18n::t("Choose a picture first.", "Elegí una imagen primero."));
            return;
        }
        let mode = view::mode_at(ui.get_instant_mode());
        let pre_path = ui.get_instant_pre().to_string();
        let post_path = ui.get_instant_post().to_string();
        let pre = (!pre_path.trim().is_empty()).then(|| Still { path: pre_path, mode: Some(view::mode_at(ui.get_instant_pre_mode())) });
        let post = (!post_path.trim().is_empty()).then(|| Still { path: post_path, mode: Some(view::mode_at(ui.get_instant_post_mode())) });
        let manual = air::Manual {
            cw: ui.get_instant_cw(),
            ax25: ui.get_instant_ax(),
            welcome: self.instant.welcome.clone(),
            pre: pre.into_iter().collect(),
            image: Still { path, mode: Some(mode) },
            post: post.into_iter().collect(),
            bye: self.instant.bye.clone(),
            program_id: None,
            block_id: None,
            name: i18n::t("Instant", "Instantánea"),
            program_name: String::new(),
            range: String::new(),
            date: self.today(),
            weekdays: None,
        };
        self.engine.send(Command::TransmitManual(Box::new(manual)));
    }

    fn use_selected_picture(&mut self) {
        let Some(id) = self.pic else {
            self.note(i18n::t("Select a picture in the block first.", "Elegí primero una imagen del bloque."));
            return;
        };
        let images = self.images_now();
        let Some(image) = images.iter().find(|i| i.id == id) else { return };
        let normal_id = image.role.target().unwrap_or(image.id);
        let Some(normal) = images.iter().find(|i| i.id == normal_id) else { return };
        let pres: Vec<&BlockImage> = images.iter().filter(|i| matches!(i.role, ImageRole::Pre { of } if of == normal_id)).collect();
        let posts: Vec<&BlockImage> = images.iter().filter(|i| matches!(i.role, ImageRole::Post { of } if of == normal_id)).collect();
        let enabled: Vec<&BlockImage> = images.iter().filter(|i| matches!(i.role, ImageRole::Normal) && i.enabled).collect();
        let first = enabled.first().map(|i| i.id) == Some(normal_id);
        let last = enabled.last().map(|i| i.id) == Some(normal_id);
        let program = self.focused_program().and_then(|id| self.project.program(id)).cloned();
        self.instant.welcome = if first { program.as_ref().and_then(|p| p.welcome.clone()) } else { None };
        self.instant.bye = if last { program.as_ref().and_then(|p| p.bye.clone()) } else { None };
        let ui = self.ui();
        ui.set_instant_path(normal.path.as_str().into());
        ui.set_instant_mode(view::mode_index(normal.mode.unwrap_or(self.project.tx.mode)));
        if let Some(pre) = pres.first() {
            ui.set_instant_pre(pre.path.as_str().into());
            ui.set_instant_pre_mode(view::mode_index(pre.mode.unwrap_or(self.project.tx.mode)));
        }
        if let Some(post) = posts.first() {
            ui.set_instant_post(post.path.as_str().into());
            ui.set_instant_post_mode(view::mode_index(post.mode.unwrap_or(self.project.tx.mode)));
        }
        ui.set_side(1);
        self.note(i18n::t("The instant tab has this picture. Transmit when you are ready.", "La pestaña instantánea tiene esta imagen. Transmití cuando quieras."));
    }

    fn on_pic(&mut self, cmd: &str, arg: i32) {
        match cmd {
            "select" => self.pic = Some(arg as u64),
            "nudge" => self.nudge(arg),
            "up" => self.nudge(-1),
            "down" => self.nudge(1),
            "toggle" => self.toggle_pic(),
            "mode" => self.cycle_pic_mode(),
            "remove" => self.remove_pic(),
            "link-up" => self.link(true),
            "link-down" => self.link(false),
            "unlink" => self.unlink_pic(),
            "act" => {
                let n = self.project.activations.len();
                if n > 0 {
                    self.act_at = (self.act_at as i32 + arg).rem_euclid(n as i32) as usize;
                    self.push_lists();
                }
            }
            "act-remove" => {
                if let Some(act) = self.project.activations.get(self.act_at) {
                    let id = act.id;
                    self.project.activations.retain(|a| a.id != id);
                    self.act_at = self.act_at.min(self.project.activations.len().saturating_sub(1));
                    self.changed();
                }
            }
            "prog" => {
                let n = self.project.programs.len();
                if n > 0 {
                    self.prog_at = (self.prog_at as i32 + arg).rem_euclid(n as i32) as usize;
                    self.push_lists();
                }
            }
            "prog-edit" => {
                if let Some(p) = self.project.programs.get(self.prog_at) {
                    let id = p.id;
                    self.focus = Focus::Program(id);
                    self.ui().set_side(0);
                    self.push_block(true);
                }
            }
            _ => {}
        }
    }

    fn nudge(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        let Some(id) = self.pic else { return };
        let images = self.images_now();
        let Some(image) = images.iter().find(|i| i.id == id).cloned() else { return };
        let result = if matches!(image.role, ImageRole::Normal) {
            let from = images.iter().take_while(|i| i.id != id).filter(|i| matches!(i.role, ImageRole::Normal)).count();
            let to = (from as i32 + delta).max(0) as usize;
            playlist::move_group(&images, id, to)
        } else {
            let side: Vec<u64> = images.iter().filter(|i| std::mem::discriminant(&i.role) == std::mem::discriminant(&image.role) && i.role.target() == image.role.target()).map(|i| i.id).collect();
            let pos = side.iter().position(|x| *x == id).unwrap_or(0) as i32;
            playlist::move_bumper(&images, id, (pos + delta).max(0) as usize)
        };
        match result {
            Ok(next) => self.commit_images(next),
            Err(e) => self.note(e.to_string()),
        }
    }

    fn toggle_pic(&mut self) {
        let Some(id) = self.pic else { return };
        let mut images = self.images_now();
        if let Some(im) = images.iter_mut().find(|i| i.id == id) {
            im.enabled = !im.enabled;
        }
        self.commit_images(images);
    }

    fn cycle_pic_mode(&mut self) {
        let Some(id) = self.pic else { return };
        let mut images = self.images_now();
        if let Some(im) = images.iter_mut().find(|i| i.id == id) {
            im.mode = next_mode(im.mode);
        }
        self.commit_images(images);
    }

    fn remove_pic(&mut self) {
        let Some(id) = self.pic else { return };
        let images = self.images_now();
        let Some(image) = images.iter().find(|i| i.id == id).cloned() else { return };
        let next: Vec<BlockImage> = if matches!(image.role, ImageRole::Normal) {
            images.into_iter().filter(|i| i.id != id && i.role.target() != Some(id)).collect()
        } else {
            images.into_iter().filter(|i| i.id != id).collect()
        };
        self.pic = None;
        self.commit_images(next);
    }

    fn link(&mut self, up: bool) {
        let Some(id) = self.pic else { return };
        let images = self.images_now();
        let mode_for = |path: &str| playlist::bumper_mode(&self.resolve(path));
        let result = if up { playlist::link_up(&images, id, mode_for) } else { playlist::link_down(&images, id, mode_for) };
        match result {
            Ok(next) => self.commit_images(next),
            Err(e) => self.note(e.to_string()),
        }
    }

    fn unlink_pic(&mut self) {
        let Some(id) = self.pic else { return };
        match playlist::unlink(&self.images_now(), id) {
            Ok(next) => self.commit_images(next),
            Err(e) => self.note(e.to_string()),
        }
    }

    fn add_files(&mut self, paths: Vec<PathBuf>) {
        let files: Vec<(String, String)> = paths
            .iter()
            .filter(|p| is_image(p))
            .map(|p| {
                let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                (self.relativize(p), name)
            })
            .collect();
        if files.is_empty() {
            return;
        }
        let mut next = self.project.next_id();
        let mode_for = |path: &str| playlist::bumper_mode(&self.resolve(path));
        match playlist::append_files(&self.images_now(), &files, || {
            let id = next;
            next += 1;
            id
        }, mode_for) {
            Ok(edit) => {
                if let Some(w) = edit.warnings.first() {
                    self.note(w.clone());
                }
                self.commit_images(edit.images);
            }
            Err(e) => self.note(e.to_string()),
        }
    }

    fn import_folder(&mut self, path: PathBuf) {
        let Some(id) = self.focused_program() else {
            self.note(i18n::t("Import fills a program. A common block takes pictures with Add.", "Importar llena un programa. Un bloque común recibe fotos con Agregar."));
            return;
        };
        let base = self.dir();
        let now = self.now();
        match playlist::import_program(&mut self.project, id, &path, base.as_deref(), now) {
            Ok(report) => {
                let warn = report.warnings.iter().chain(report.skipped.iter()).cloned().collect::<Vec<_>>().join(" · ");
                self.note(if warn.is_empty() {
                    i18n::t(&format!("{} occurrences filled", report.filled.len()), &format!("{} ocurrencias llenas", report.filled.len()))
                } else {
                    warn
                });
                self.changed();
            }
            Err(e) => self.note(e.to_string()),
        }
    }

    fn set_still(&mut self, welcome: bool, path: Option<PathBuf>) {
        let Some(id) = self.focused_program() else { return };
        let still = path.map(|p| Still { path: self.relativize(&p), mode: Some(self.project.tx.mode) });
        if let Some(p) = self.project.program_mut(id) {
            if welcome { p.welcome = still; } else { p.bye = still; }
        }
        self.changed();
    }

    fn cycle_still(&mut self, welcome: bool) {
        let Some(id) = self.focused_program() else { return };
        if let Some(p) = self.project.program_mut(id) {
            let slot = if welcome { &mut p.welcome } else { &mut p.bye };
            if let Some(still) = slot {
                still.mode = next_mode(still.mode);
            }
        }
        self.changed();
    }

    fn set_ends(&mut self, text: &str) {
        let Some(id) = self.focused_program() else { return };
        let ends = if text.trim().is_empty() {
            None
        } else {
            let Ok(date) = NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d") else {
                self.note(i18n::t("The date is YYYY-MM-DD.", "La fecha es AAAA-MM-DD."));
                self.push_block(true);
                return;
            };
            let next = date + TimeDelta::days(1);
            let Some(naive) = next.and_hms_opt(0, 0, 0) else { return };
            match to_utc(self.project.grid.timezone, naive) {
                Some(t) => Some(t),
                None => {
                    self.note(i18n::t("That instant does not exist on this clock.", "Ese instante no existe en este reloj."));
                    return;
                }
            }
        };
        if let Some(p) = self.project.program_mut(id) {
            p.ends = ends;
        }
        self.changed();
    }

    fn on_field(&mut self, key: &str, value: &str) {
        match key {
            "name" => {
                if let Some(id) = self.focused_program() {
                    if let Some(p) = self.project.program_mut(id) {
                        p.name = value.to_string();
                    }
                    self.changed();
                } else if let Some(card) = self.card().cloned() {
                    if card.block_id >= 0 {
                        if let Some(b) = self.project.block_mut(card.block_id as u64) {
                            b.name = value.to_string();
                        }
                        self.changed();
                    }
                }
            }
            "start" => {
                if parse_hhmm(value).is_some() {
                    self.set_hours(Some(value.to_string()), None);
                }
            }
            "end" => {
                if parse_hhmm(value).is_some() {
                    self.set_hours(None, Some(value.to_string()));
                }
            }
            "enabled" => self.set_day_enabled(value == "1"),
            "program-on" => {
                if let Some(id) = self.focused_program() {
                    if let Some(p) = self.project.program_mut(id) {
                        p.enabled = value == "1";
                    }
                    self.changed();
                }
            }
            "wd" => {
                let mut parts = value.split(':');
                let day: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                let on = parts.next() == Some("1");
                if day < 7 {
                    self.weekday(day, on);
                }
            }
            "ends" => {
                if value.trim().is_empty() || NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d").is_ok() {
                    self.set_ends(value);
                }
            }
            "welcome" => self.set_still(true, None),
            "bye" => self.set_still(false, None),
            "welcome-mode" => self.cycle_still(true),
            "bye-mode" => self.cycle_still(false),
            "delete-program" => self.open_delete_program(),
            "delete-block" => {
                if let Some(card) = self.card().cloned() {
                    if card.program_id < 0 && card.block_id >= 0 {
                        let id = card.block_id as u64;
                        self.ask(1, &i18n::t("Delete block", "Borrar bloque"), &i18n::t("This common block will be deleted.", "Este bloque común se va a borrar."), &i18n::t("Delete", "Borrar"), &i18n::t("Cancel", "Cancelar"), "", Pending::DeleteBlock { id });
                    }
                }
            }
            "new-program" => {
                let id = self.project.next_id();
                let n = self.project.programs.len() + 1;
                self.project.programs.push(Program { id, name: format!("{} {n}", i18n::t("Program", "Programa")), ..Program::default() });
                self.focus = Focus::Program(id);
                self.prog_at = self.project.programs.len() - 1;
                self.ui().set_side(0);
                self.changed();
                self.note(i18n::t("The program has no days and no hours, so it is not on the grid yet.", "El programa no tiene días ni horario, así que todavía no está en la grilla."));
            }
            "export" => self.open_export(),
            "channel" => { self.project.name = value.to_string(); self.ui().set_project_name(value.into()); self.changed(); }
            "call" => { self.project.station.call = value.to_string(); self.ui().set_callsign(value.into()); self.changed(); }
            "op" => { self.project.station.operator = value.to_string(); self.changed(); }
            "qth" => { self.project.station.qth = value.to_string(); self.changed(); }
            "loc" => { self.project.station.locator = value.to_string(); self.changed(); }
            "freq" => {
                if let Some(hz) = view::parse_mhz(value) {
                    self.project.freq_hz = hz;
                    self.changed();
                } else {
                    self.note(i18n::t("Frequency in MHz, or 0.", "Frecuencia en MHz, o 0."));
                }
            }
            "cw-text" => { self.project.tx.cw_text = value.to_string(); self.changed(); }
            "vox" => { self.project.tx.vox_preamble = value == "1"; self.changed(); }
            "cw-pre" => { self.project.tx.cw_preamble = value == "1"; self.changed(); }
            "cw-id" => { self.project.tx.cw_id = value == "1"; self.changed(); }
            "ax-info" => { self.project.ax25.info = value == "1"; self.changed(); }
            "ax-mode" => { self.project.ax25.mode = value == "1"; self.changed(); }
            "dedupe" => { self.project.ax25.dedupe = value == "1"; self.changed(); }
            "port" => { self.config.ptt.port = value.to_string(); self.save_config(); }
            "cat-host" => { self.config.cat.host = value.to_string(); self.save_config(); }
            "cat-freq" => { self.config.cat.set_freq = value == "1"; self.save_config(); }
            "audio-out" => { self.config.audio.output = value.to_string(); self.save_config(); }
            "audio-in" => { self.config.audio.input = value.to_string(); self.save_config(); }
            "ntp" => { self.config.time.enabled = value == "1"; self.config.time.prompted = true; self.save_config(); }
            "ntp-server" => { self.config.time.server = value.to_string(); self.save_config(); }
            _ => {}
        }
    }

    fn on_num(&mut self, key: &str, value: i32) {
        match key {
            "hue" => {
                if let Some(id) = self.focused_program() {
                    if let Some(p) = self.project.program_mut(id) {
                        p.hue = (value.max(0) as u16) % 360;
                    }
                    self.changed();
                }
            }
            "interval" => { self.project.interval_min = value.max(0) as u32; self.changed(); }
            "offset" => { self.project.grid.offset_s = value.max(0) as u32; self.changed(); }
            "mode" => { self.project.tx.mode = view::mode_at(value); self.changed(); }
            "pause" => { self.project.tx.min_pause_s = value.max(0) as u32; self.changed(); }
            "next-count" => { self.project.ax25.next_count = value.max(1) as u32; self.project.ax25.by_count = true; self.changed(); }
            "next-hours" => { self.project.ax25.next_hours = value.max(1) as u32; self.project.ax25.by_count = false; self.changed(); }
            "tz" => {
                self.project.grid.timezone = if value == 1 { GridTz::Local } else { GridTz::Utc };
                self.changed();
            }
            "ptt" => {
                self.config.ptt.method = match value {
                    0 => PttMethod::None,
                    2 => PttMethod::Serial,
                    3 => PttMethod::Cat,
                    _ => PttMethod::Vox,
                };
                self.save_config();
            }
            "lead" => { self.config.ptt.lead_ms = value.max(0) as u32; self.save_config(); }
            "tail" => { self.config.ptt.tail_ms = value.max(0) as u32; self.save_config(); }
            "cat-port" => { self.config.cat.port = value.clamp(1, 65535) as u16; self.save_config(); }
            "lang" => {
                self.config.ui.lang = if value == 1 { "es" } else { "en" }.into();
                i18n::apply_slint(Lang::resolve(&self.config.ui.lang));
                self.save_config();
                self.push_channel();
                self.push_equipment();
                self.refresh_cards();
                self.push_block(true);
                self.push_lists();
            }
            "ntp-hours" => { self.config.time.period_h = value.max(1) as u32; self.save_config(); }
            _ => {}
        }
    }

    fn set_day_enabled(&mut self, on: bool) {
        let Some(card) = self.card().cloned() else {
            self.push_block(true);
            return;
        };
        if card.block_id >= 0 {
            if let Some(b) = self.project.block_mut(card.block_id as u64) {
                b.enabled = on;
            }
            self.changed();
            return;
        }
        if card.program_id >= 0 && !on {
            self.note(i18n::t(
                "An empty day is not stored. Add a picture before disabling that day, or turn the weekday off.",
                "Un día vacío no se guarda. Agregá una foto antes de desactivar ese día, o apagá el día de la semana.",
            ));
            self.push_block(true);
        }
    }

    fn confirm(&mut self, which: i32) {
        match &self.pending {
            Pending::Message => self.close_dialog(),
            Pending::Create => self.confirm_create(which),
            Pending::Weekday { .. } | Pending::DeleteProgram { .. } => self.finish_choice(which),
            Pending::DeleteBlock { id } => {
                let id = *id;
                if which == 0 {
                    self.project.blocks.retain(|b| b.id != id);
                    self.focus = Focus::None;
                    self.close_dialog();
                    self.changed();
                } else {
                    self.close_dialog();
                }
            }
            Pending::Export => {
                if which == 0 {
                    self.start_export_pick();
                } else {
                    self.close_dialog();
                }
            }
            Pending::Quality { .. } => {
                if which == 0 {
                    if let Pending::Quality { req, dir } = std::mem::replace(&mut self.pending, Pending::None) {
                        self.close_dialog();
                        self.launch_export(req, dir);
                    }
                } else {
                    self.close_dialog();
                }
            }
            Pending::Clock => {
                self.config.time.prompted = true;
                self.config.time.enabled = which == 0;
                self.close_dialog();
                self.save_config();
                self.push_equipment();
            }
            Pending::Activation(id) => {
                if which == 0 {
                    let id = *id;
                    self.save_activation(id);
                } else {
                    self.close_dialog();
                }
            }
            Pending::Leave(after) => {
                let after = *after;
                if which == 2 {
                    self.close_dialog();
                    return;
                }
                self.close_dialog();
                if which == 0 {
                    self.after = after;
                    self.save(true);
                } else {
                    self.dirty = false;
                    self.follow(after);
                }
            }
            Pending::None => self.close_dialog(),
        }
    }

    fn start_export_pick(&mut self) {
        let Some(req) = self.read_export() else {
            self.note(i18n::t("Use YYYY-MM-DD HH:MM.", "Usá AAAA-MM-DD HH:MM."));
            return;
        };
        self.close_dialog();
        let main = self.main.clone();
        let es = self.es();
        let stored = req.clone();
        pick_native(NativePick::Directory, move |paths| {
            let Some(ui) = main.upgrade() else { return };
            let Some(dir) = paths.into_iter().next() else {
                ui.invoke_hold_export("".into());
                return;
            };
            if export::quality_warning(req.format, req.rate, es).is_some() {
                ui.invoke_hold_export(SharedString::from(dir.to_string_lossy().into_owned()));
            } else {
                ui.invoke_run_export(SharedString::from(dir.to_string_lossy().into_owned()));
            }
        });
        self.pending = Pending::Quality { req: stored, dir: PathBuf::new() };
    }

    fn follow(&mut self, after: After) {
        match after {
            After::Nothing => {}
            After::Quit => {
                let _ = slint::quit_event_loop();
            }
            After::New => {
                self.project = Project::default();
                self.path = None;
                self.focus = Focus::None;
                self.loaded();
            }
            After::Open => self.pick_open(),
        }
    }

    fn save(&mut self, then_follow: bool) {
        if let Some(path) = self.path.clone() {
            self.write_to(&path);
            if then_follow {
                let after = self.after;
                self.after = After::Nothing;
                self.follow(after);
            }
            return;
        }
        let after = self.after;
        let main = self.main.clone();
        pick_native(NativePick::ProjectSave, move |paths| {
            let Some(ui) = main.upgrade() else { return };
            if let Some(path) = paths.into_iter().next() {
                ui.invoke_saved_as(SharedString::from(path.to_string_lossy().into_owned()));
            } else if then_follow && matches!(after, After::Quit) {
                // stay open
            }
        });
    }

    fn write_to(&mut self, path: &Path) {
        let path = ensure_ext(path);
        match self.project.save(&path) {
            Ok(()) => {
                self.path = Some(path);
                self.dirty = false;
                self.ui().set_dirty(false);
                self.composer.set_project_dir(self.dir().as_deref());
                self.config.ui.last_project = self.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                let _ = self.config.save(&self.config_path);
                self.send_project();
                self.note(i18n::t("Saved.", "Guardado."));
            }
            Err(e) => self.message(e.to_string()),
        }
    }

    fn pick_open(&mut self) {
        let main = self.main.clone();
        pick_native(NativePick::ProjectOpen, move |paths| {
            let Some(ui) = main.upgrade() else { return };
            if let Some(path) = paths.into_iter().next() {
                ui.invoke_opened(SharedString::from(path.to_string_lossy().into_owned()));
            }
        });
    }

    fn open_path(&mut self, path: PathBuf) {
        match Project::load(&path) {
            Ok(p) => {
                self.project = p;
                self.path = Some(path);
                self.focus = Focus::None;
                self.loaded();
            }
            Err(e) => self.message(e.to_string()),
        }
    }

    fn guard_dirty(&mut self, after: After) {
        if self.dirty {
            self.ask(
                1,
                &i18n::t("Save the channel?", "¿Guardar el canal?"),
                &i18n::t("The channel has changes.", "El canal tiene cambios."),
                &i18n::t("Save", "Guardar"),
                &i18n::t("Don't save", "No guardar"),
                &i18n::t("Cancel", "Cancelar"),
                Pending::Leave(after),
            );
        } else {
            self.follow(after);
        }
    }

    fn open_modes(&mut self) {
        if self.modes.is_none() {
            let win = match UtilsDialog::new() {
                Ok(w) => w,
                Err(e) => {
                    self.note(e.to_string());
                    return;
                }
            };
            let items: Vec<ModeCheat> = Mode::ALL.iter().copied().map(|m| {
                let s = m.spec();
                let color = match s.family {
                    estivi_core::modes::Family::RobotBw => i18n::ts("B/W", "B/N"),
                    _ => i18n::ts("Color", "Color"),
                };
                ModeCheat {
                    name: s.name.into(),
                    size: format!("{}×{}", s.width, s.height).into(),
                    color: color.into(),
                    time: view::duration(s.total_ms()).into(),
                    vis: format!("0x{:02X}", s.vis).into(),
                }
            }).collect();
            win.set_modes(ModelRc::new(VecModel::from(items)));
            win.on_dismiss({
                let w = win.as_weak();
                move || { if let Some(w) = w.upgrade() { let _ = w.hide(); } }
            });
            apply_theme_to(&win.global::<Theme>(), self.config.ui.light);
            self.modes = Some(win);
        }
        if let Some(win) = &self.modes {
            apply_theme_to(&win.global::<Theme>(), self.config.ui.light);
            let _ = win.show();
        }
    }
}

fn still_label(still: Option<&Still>) -> String {
    match still {
        Some(s) => format!("{} · {}", view::file_name(&s.path), s.mode.map(|m| m.spec().short).unwrap_or("·")),
        None => "—".into(),
    }
}

fn next_mode(current: Option<Mode>) -> Option<Mode> {
    match current {
        None => Mode::ALL.first().copied(),
        Some(m) => {
            let i = Mode::ALL.iter().position(|x| *x == m).unwrap_or(0);
            if i + 1 >= Mode::ALL.len() { None } else { Mode::ALL.get(i + 1).copied() }
        }
    }
}

fn parse_local(text: &str, tz: GridTz) -> Option<chrono::DateTime<Utc>> {
    let text = text.trim().replace('T', " ");
    let naive = NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M").ok()?;
    to_utc(tz, naive)
}

fn ensure_ext(path: &Path) -> PathBuf {
    if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case(project::EXTENSION)) {
        path.to_path_buf()
    } else {
        path.with_extension(project::EXTENSION)
    }
}

fn script_of(actions: &[Action]) -> String {
    actions.iter().map(|a| match a {
        Action::Cw { text, .. } => format!("cw {text}"),
        Action::Dtmf { sequence } => format!("dtmf {sequence}"),
        Action::Image { path, .. } => format!("image {path}"),
        Action::Pause => "pause".into(),
        Action::Resume => "resume".into(),
        Action::Ax25 { comment, .. } => format!("ax25 {comment}"),
        Action::Command { command, via } => format!("command {}|{command}", via_name(*via)),
        Action::Api { url, via, .. } => format!("api {}|{url}", via_name(*via)),
        Action::Random { paths, .. } => format!("random {}", paths.join("|")),
    }).collect::<Vec<_>>().join("\n")
}

fn via_name(via: Carrier) -> &'static str {
    match via {
        Carrier::Dtmf => "dtmf",
        Carrier::Ax25 => "ax25",
        Carrier::Cw => "cw",
    }
}

fn parse_via(s: &str) -> Carrier {
    match s.trim().to_ascii_lowercase().as_str() {
        "dtmf" => Carrier::Dtmf,
        "ax25" => Carrier::Ax25,
        _ => Carrier::Cw,
    }
}

fn parse_script(text: &str, wpm: f32, tone: f32) -> Result<Vec<Action>, String> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (kind, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match kind.to_ascii_lowercase().as_str() {
            "cw" => out.push(Action::Cw { text: rest.into(), wpm, tone_hz: tone }),
            "dtmf" => out.push(Action::Dtmf { sequence: rest.into() }),
            "image" => out.push(Action::Image { path: rest.into(), mode: None }),
            "pause" => out.push(Action::Pause),
            "resume" => out.push(Action::Resume),
            "ax25" => out.push(Action::Ax25 { lines: Default::default(), comment: rest.into() }),
            "command" => {
                let (via, cmd) = rest.split_once('|').unwrap_or(("cw", rest));
                out.push(Action::Command { command: cmd.trim().into(), via: parse_via(via) });
            }
            "api" => {
                let (via, url) = rest.split_once('|').unwrap_or(("cw", rest));
                out.push(Action::Api { url: url.trim().into(), json_path: String::new(), via: parse_via(via) });
            }
            "random" => {
                let paths = rest.split('|').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                out.push(Action::Random { paths, mode: None });
            }
            other => return Err(i18n::t(&format!("Unknown action: {other}"), &format!("Acción desconocida: {other}"))),
        }
    }
    Ok(out)
}

pub fn run(config: Config, config_path: PathBuf) -> anyhow::Result<()> {
    let ui = MainWindow::new()?;
    i18n::apply_slint(Lang::resolve(&config.ui.lang));
    let api = ApiCache::default();
    let engine = Engine::start(config.clone(), config_path.clone(), config_path.with_file_name("estivi-registro.csv"), api.clone());
    let last = std::env::args_os().nth(1).map(|a| a.to_string_lossy().into_owned()).unwrap_or_else(|| config.ui.last_project.clone());
    let today = chrono::Local::now().date_naive();
    let app: Shared = Rc::new_cyclic(|me| {
        let _ = me;
        RefCell::new(App {
            project: Project::default(),
            path: None,
            dirty: false,
            config,
            config_path,
            engine,
            composer: Composer::new(api),
            status: Status::default(),
            editor: None,
            main: ui.as_weak(),
            modes: None,
            cards: Vec::new(),
            focus: Focus::None,
            pic: None,
            zoom: 0,
            anchor: today,
            pending: Pending::None,
            after: After::Nothing,
            act_at: 0,
            prog_at: 0,
            instant: InstantHold { welcome: None, bye: None },
            seen_air: 0,
            seen_log: 0,
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
            Err(e) => log::warn!("could not open last project: {e:#}"),
        }
    }
    wire(&app, &ui);
    app.borrow_mut().loaded();
    apply_theme_to(&ui.global::<Theme>(), app.borrow().config.ui.light);
    ui.invoke_sync_palette();
    let timer = slint::Timer::default();
    {
        let weak = Rc::downgrade(&app);
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(200), move || with_app(&weak, App::tick));
    }
    {
        let weak = Rc::downgrade(&app);
        ui.window().on_close_requested(move || {
            let Some(app) = weak.upgrade() else { return slint::CloseRequestResponse::HideWindow };
            if app.borrow().dirty {
                app.borrow_mut().guard_dirty(After::Quit);
                slint::CloseRequestResponse::KeepWindowShown
            } else {
                let _ = slint::quit_event_loop();
                slint::CloseRequestResponse::HideWindow
            }
        });
    }
    ui.show()?;
    slint::run_event_loop()?;
    drop(timer);
    drop(app);
    Ok(())
}

fn wire(app: &Shared, ui: &MainWindow) {
    let w = Rc::downgrade(app);
    ui.on_new_project({ let w = w.clone(); move || with_app(&w, |a| a.guard_dirty(After::New)) });
    ui.on_open_project({ let w = w.clone(); move || with_app(&w, |a| a.guard_dirty(After::Open)) });
    ui.on_save_project({ let w = w.clone(); move || with_app(&w, |a| { a.after = After::Nothing; a.save(false); }) });
    ui.on_save_as({
        let w = w.clone();
        move || with_app(&w, |a| { a.path = None; a.after = After::Nothing; a.save(false); })
    });
    ui.on_open_editor({ let w = w.clone(); move || { if let Some(app) = w.upgrade() { editor::open(&app); } } });
    ui.on_open_modes({ let w = w.clone(); move || with_app(&w, App::open_modes) });
    ui.on_set_light({
        let w = w.clone();
        move |light| with_app(&w, |a| {
            a.config.ui.light = light;
            apply_theme_to(&a.ui().global::<Theme>(), light);
            if let Some(ed) = &a.editor {
                ed.apply_theme(light);
            }
            a.save_config();
        })
    });
    ui.on_set_agenda({ let w = w.clone(); move |on| with_app(&w, |a| a.engine.send(Command::SetAgenda(on))) });
    ui.on_stop({ let w = w.clone(); move || with_app(&w, |a| a.engine.send(Command::Stop)) });
    ui.on_set_zoom({
        let w = w.clone();
        move |z| with_app(&w, |a| { a.zoom = z; a.refresh_cards(); a.push_block(false); })
    });
    ui.on_nav({
        let w = w.clone();
        move |dir| with_app(&w, |a| {
            if dir == 0 {
                a.anchor = a.today();
            } else {
                a.anchor = match a.zoom() {
                    Zoom::Month => add_months(a.anchor, dir),
                    Zoom::Week => a.anchor + TimeDelta::days(i64::from(7 * dir)),
                    Zoom::Day => a.anchor + TimeDelta::days(i64::from(dir)),
                };
            }
            a.refresh_cards();
        })
    });
    ui.on_select_card({ let w = w.clone(); move |i| with_app(&w, |a| { a.focus = Focus::Card(i.max(0) as usize); a.pic = None; a.push_block(true); }) });
    ui.on_open_card({ let w = w.clone(); move |i| with_app(&w, |a| { a.focus = Focus::Card(i.max(0) as usize); a.ui().set_side(0); a.push_block(true); }) });
    ui.on_edge({ let w = w.clone(); move |i, end, minute| with_app(&w, |a| a.set_edge(i, end, minute)) });
    ui.on_create_at({ let w = w.clone(); move |cell, minute| with_app(&w, |a| a.open_create(cell, minute)) });
    ui.on_set_field({ let w = w.clone(); move |k, v| with_app(&w, |a| a.on_field(k.as_str(), v.as_str())) });
    ui.on_set_num({ let w = w.clone(); move |k, v| with_app(&w, |a| a.on_num(k.as_str(), v)) });
    ui.on_pic({ let w = w.clone(); move |k, v| with_app(&w, |a| a.on_pic(k.as_str(), v)) });
    ui.on_confirm({ let w = w.clone(); move |which| with_app(&w, |a| a.confirm(which)) });
    ui.on_transmit({ let w = w.clone(); move || with_app(&w, App::transmit) });
    ui.on_edit_activation({ let w = w.clone(); move |n| with_app(&w, |a| a.open_activation(n < 0)) });
    ui.on_pick({
        let w = w.clone();
        let uiw = ui.as_weak();
        move |kind| {
            let kind = kind.to_string();
            if kind == "from-block" {
                with_app(&w, App::use_selected_picture);
                return;
            }
            let pick = match kind.as_str() {
                "images" => NativePick::Images { many: true },
                "folder" => NativePick::Directory,
                "instant" | "pre" | "post" | "welcome" | "bye" => NativePick::Images { many: false },
                _ => return,
            };
            let uiw = uiw.clone();
            let tag = kind.clone();
            pick_native(pick, move |paths| {
                let Some(ui) = uiw.upgrade() else { return };
                ui.invoke_files_ready(SharedString::from(tag), paths_model(&paths));
            });
        }
    });
    ui.on_files_ready({
        let w = w.clone();
        move |kind, model| {
            let paths: Vec<PathBuf> = model.iter().map(|s| PathBuf::from(s.to_string())).collect();
            with_app(&w, |a| match kind.as_str() {
                "images" => a.add_files(paths),
                "folder" => {
                    if let Some(p) = paths.into_iter().next() {
                        a.import_folder(p);
                    }
                }
                "instant" => {
                    if let Some(p) = paths.into_iter().next() {
                        a.instant = InstantHold { welcome: None, bye: None };
                        a.ui().set_instant_path(a.relativize(&p).into());
                        a.ui().set_instant_mode(view::mode_index(a.project.tx.mode));
                    }
                }
                "pre" => {
                    if let Some(p) = paths.into_iter().next() {
                        a.ui().set_instant_pre_mode(view::mode_index(playlist::bumper_mode(&p)));
                        a.ui().set_instant_pre(a.relativize(&p).into());
                    }
                }
                "post" => {
                    if let Some(p) = paths.into_iter().next() {
                        a.ui().set_instant_post_mode(view::mode_index(playlist::bumper_mode(&p)));
                        a.ui().set_instant_post(a.relativize(&p).into());
                    }
                }
                "welcome" => a.set_still(true, paths.into_iter().next()),
                "bye" => a.set_still(false, paths.into_iter().next()),
                _ => {}
            });
        }
    });
    ui.on_saved_as({
        let w = w.clone();
        move |path| with_app(&w, |a| {
            a.write_to(Path::new(path.as_str()));
            let after = a.after;
            a.after = After::Nothing;
            if !a.dirty {
                a.follow(after);
            }
        })
    });
    ui.on_opened({ let w = w.clone(); move |path| with_app(&w, |a| a.open_path(PathBuf::from(path.as_str()))) });
    ui.on_run_export({
        let w = w.clone();
        move |dir| with_app(&w, |a| {
            if let Pending::Quality { req, .. } = std::mem::replace(&mut a.pending, Pending::None) {
                a.launch_export(req, PathBuf::from(dir.as_str()));
            }
        })
    });
    ui.on_hold_export({
        let w = w.clone();
        move |dir| with_app(&w, |a| {
            if dir.is_empty() {
                a.pending = Pending::None;
                return;
            }
            if let Pending::Quality { req, .. } = std::mem::replace(&mut a.pending, Pending::None) {
                let body = export::quality_warning(req.format, req.rate, a.es()).unwrap_or_default();
                a.ask(5, &i18n::t("Audio quality", "Calidad de audio"), &body, &i18n::t("Continue", "Continuar"), &i18n::t("Cancel", "Cancelar"), "", Pending::Quality { req, dir: PathBuf::from(dir.as_str()) });
            }
        })
    });
}

fn add_months(date: NaiveDate, delta: i32) -> NaiveDate {
    let mut month = date.month() as i32 + delta;
    let mut year = date.year();
    while month > 12 { month -= 12; year += 1; }
    while month < 1 { month += 12; year -= 1; }
    let day = date.day().min(days_in(year, month as u32));
    NaiveDate::from_ymd_opt(year, month as u32, day).unwrap_or(date)
}

fn days_in(year: i32, month: u32) -> u32 {
    let next = if month == 12 { NaiveDate::from_ymd_opt(year + 1, 1, 1) } else { NaiveDate::from_ymd_opt(year, month + 1, 1) };
    next.and_then(|n| NaiveDate::from_ymd_opt(year, month, 1).map(|f| (n - f).num_days() as u32)).unwrap_or(28)
}
