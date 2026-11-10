//! The station window: the grid in the middle, and the panels around it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use crossbeam_channel::{Receiver, Sender};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use estivi_core::air::{self, GraphicsLevel, PieceKind};
use estivi_core::audio::{self, DeviceInfo};
use estivi_core::compose::Composer;
use estivi_core::config::{Config, Lang, LineLevel, PttMethod};
use estivi_core::dynamic::ApiCache;
use estivi_core::engine::{Command, Engine, Phase, Status};
use estivi_core::export::{self, AudioFormat, ExportRequest, Shape};
use estivi_core::graphics::Fit;
use estivi_core::modes::Mode;
use estivi_core::playlist::{self};
use estivi_core::project::{
    self, ActTrigger, Action, Activation, Block, BlockImage, Carrier, GridTz, ImageRole, Program, Project, Still, TriggerSource,
    format_hhmm, parse_hhmm, relativize, resolve,
};
use estivi_core::ptt;
use estivi_core::schedule::{self, Card, Keep, Occurrence, Zoom, future_limit, grid_date, proposed_window, to_utc};
use estivi_core::update;

use crate::editor;
use crate::i18n;
use crate::view;
use crate::{ActionItem, CardItem, Files, MainWindow, ModeCheat, PicItem, ProgramsWindow, StepItem, Theme, TickItem, TxItem, UtilsDialog};

pub type Shared = Rc<RefCell<App>>;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "webp"];
const UPDATE_PERIOD_S: i64 = 24 * 3600;

// Logical sizes of the fixed image widgets in main.slint. Thumbnails are
// pre-scaled to these boxes (times the window scale factor) so the UI can
// paint them 1:1 instead of resampling them with a cheaper filter.
const LIVE_W: f32 = 200.0;
const LIVE_H: f32 = 190.0;
const NEXT_W: f32 = 200.0;
const NEXT_H: f32 = 190.0;
const LIST_W: u32 = 34;
const LIST_H: u32 = 24;
const CLIP_H: u32 = 62;
const INSTANT_H: u32 = 120;

fn weekday_name(day: usize, spanish: bool) -> &'static str {
    let en = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    let es = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
    let names = if spanish { &es } else { &en };
    names.get(day).copied().unwrap_or(names[0])
}

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

pub const THEME_IDS: [&str; 7] = ["dark", "light", "dracula", "lavender", "pastel", "subte", "subte-light"];

/// `gray` is the old name of Dark.
fn theme_id(id: &str) -> &str {
    match id {
        "gray" => "dark",
        other => other,
    }
}

pub fn theme_label(id: &str) -> String {
    match theme_id(id) {
        "light" => i18n::t("Light", "Claro"),
        "dracula" => i18n::t("Dracula", "Drácula"),
        "lavender" => i18n::t("Lavender", "Lavanda"),
        "pastel" => i18n::t("Pastel", "Pastel"),
        "subte" => i18n::t("Subte", "Subte"),
        "subte-light" => i18n::t("Subte light", "Subte claro"),
        _ => i18n::t("Dark", "Oscuro"),
    }
}

pub fn theme_index(id: &str, light: bool) -> i32 {
    let id = theme_id(id);
    let id = if THEME_IDS.contains(&id) { id } else if light { "light" } else { "dark" };
    THEME_IDS.iter().position(|x| *x == id).unwrap_or(0) as i32
}

fn rgb(hex: u32) -> slint::Color {
    slint::Color::from_argb_u8(0xff, (hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// The header logo at the pixel size of its 28 logical px tile. The default
/// Slint image would be resampled by the renderer; this one is bicubic and 1:1.
fn set_logo(ui: &MainWindow) {
    let Ok(icon) = image::load_from_memory(include_bytes!("../assets/icon.png")) else { return };
    let side = (28.0 * ui.window().scale_factor().max(1.0)).round().max(1.0) as u32;
    let logo = icon.resize_exact(side, side, image::imageops::FilterType::CatmullRom).to_rgba8();
    ui.set_logo(view::to_image(&logo));
}

struct Palette {
    light: bool,
    bg: u32,
    panel: u32,
    raised: u32,
    field: u32,
    border: u32,
    strong: u32,
    text: u32,
    dim: u32,
    faint: u32,
    accent: u32,
    on_accent: u32,
    on_air: u32,
    ready: u32,
    paused: u32,
    danger: u32,
    tx_cw: u32,
    tx_ax: u32,
    tx_welcome: u32,
    tx_bumper: u32,
}

fn palette(id: &str) -> Palette {
    match theme_id(id) {
        // Same neutral ramp as Dark, inverted. No blue in the chrome.
        "light" => Palette {
            light: true, bg: 0xf2f2f2, panel: 0xffffff, raised: 0xe4e4e4, field: 0xffffff, border: 0xc8c8c8, strong: 0x8a8a8a,
            text: 0x111111, dim: 0x3d3d3d, faint: 0x6b6b6b, accent: 0x222222, on_accent: 0xf7f7f7,
            on_air: 0xd42020, ready: 0x1b7f3a, paused: 0xc47a10, danger: 0xd42020,
            tx_cw: 0x0284c7, tx_ax: 0xb45309, tx_welcome: 0x15803d, tx_bumper: 0x7e22ce,
        },
        // Dracula, with the surfaces pulled apart. The pale purple and yellow stay off the chrome.
        "dracula" => Palette {
            light: false, bg: 0x14151c, panel: 0x282a36, raised: 0x383a4a, field: 0x1e1f29, border: 0x44475a, strong: 0x6d7394,
            text: 0xf8f8f2, dim: 0xd5d6e0, faint: 0x9aa0b8, accent: 0xff79c6, on_accent: 0x1e1f29,
            on_air: 0xff5555, ready: 0x50fa7b, paused: 0xffb86c, danger: 0xff5555,
            tx_cw: 0x8be9fd, tx_ax: 0xffb86c, tx_welcome: 0x50fa7b, tx_bumper: 0xbd93f9,
        },
        "lavender" => Palette {
            light: true, bg: 0xf7f2fc, panel: 0xfffbff, raised: 0xf3eafb, field: 0xffffff, border: 0xe6d8f2, strong: 0xd4c2e6,
            text: 0x2c2438, dim: 0x655878, faint: 0x9a8aab, accent: 0x6d4ca8, on_accent: 0xffffff,
            on_air: 0xc43b5a, ready: 0x2f8f5b, paused: 0xb57a18, danger: 0xc43b5a,
            tx_cw: 0x3b82f6, tx_ax: 0xb57a18, tx_welcome: 0x2f8f5b, tx_bumper: 0x8b5cf6,
        },
        "pastel" => Palette {
            light: true, bg: 0xf7f1ea, panel: 0xfffaf6, raised: 0xffe8dc, field: 0xfffaf6, border: 0xf0d9cc, strong: 0xe4c3b4,
            text: 0x3b2f3a, dim: 0x7a6570, faint: 0xb39aa6, accent: 0xff7a90, on_accent: 0x3b1d28,
            on_air: 0xe23d5b, ready: 0x3aa56b, paused: 0xd0892d, danger: 0xe23d5b,
            tx_cw: 0x4c9bd6, tx_ax: 0xd0892d, tx_welcome: 0x3aa56b, tx_bumper: 0xb06fb8,
        },
        // Buenos Aires Subte. Dark keeps the night ground. Subte light is the map:
        // white page, yellow header (H), and the line colors.
        // A #00aedb, B #ee1b2d, C #0467b3, D #008066, E #6c2181, H #fdba31.
        "subte" => Palette {
            light: false, bg: 0x000000, panel: 0x101010, raised: 0x1c1c1c, field: 0x101010, border: 0x303030, strong: 0x4a4a4e,
            text: 0xc0c0c0, dim: 0x9a9a9e, faint: 0x79787d, accent: 0x00aedb, on_accent: 0x041820,
            on_air: 0xee1b2d, ready: 0x008066, paused: 0xfdba31, danger: 0xee1b2d,
            tx_cw: 0x00aedb, tx_ax: 0xfdba31, tx_welcome: 0x008066, tx_bumper: 0x6c2181,
        },
        "subte-light" => Palette {
            light: true, bg: 0xffffff, panel: 0xffffff, raised: 0xf3f3f3, field: 0xffffff, border: 0xe0e0e0, strong: 0xd2d2d2,
            text: 0x303030, dim: 0x5a5a5a, faint: 0x8a8a8a, accent: 0xfdba31, on_accent: 0x1a1a1a,
            on_air: 0xee1b2d, ready: 0x008066, paused: 0x6c2181, danger: 0xee1b2d,
            tx_cw: 0x0467b3, tx_ax: 0xb07a00, tx_welcome: 0x008066, tx_bumper: 0x6c2181,
        },
        // Dark. Wider steps than the old Gray ramp, still with no hue in the chrome.
        _ => Palette {
            light: false, bg: 0x090909, panel: 0x1a1a1a, raised: 0x2e2e2e, field: 0x101010, border: 0x454545, strong: 0x737373,
            text: 0xf7f7f7, dim: 0xc2c2c2, faint: 0x8a8a8a, accent: 0xf0f0f0, on_accent: 0x111111,
            on_air: 0xd42020, ready: 0x1b7f3a, paused: 0xc47a10, danger: 0xd42020,
            tx_cw: 0x38bdf8, tx_ax: 0xfbbf24, tx_welcome: 0x4ade80, tx_bumper: 0xc084fc,
        },
    }
}

pub fn apply_theme_to(theme: &Theme, id: &str, light_fallback: bool) {
    let id = theme_id(id);
    let id = if THEME_IDS.contains(&id) { id } else if light_fallback { "light" } else { "dark" };
    let p = palette(id);
    theme.set_light(p.light);
    theme.set_bg(rgb(p.bg));
    theme.set_panel(rgb(p.panel));
    theme.set_panel_raised(rgb(p.raised));
    theme.set_field(rgb(p.field));
    theme.set_border(rgb(p.border));
    theme.set_border_strong(rgb(p.strong));
    theme.set_text(rgb(p.text));
    theme.set_text_dim(rgb(p.dim));
    theme.set_text_faint(rgb(p.faint));
    theme.set_accent(rgb(p.accent));
    theme.set_on_accent(rgb(p.on_accent));
    theme.set_on_air(rgb(p.on_air));
    theme.set_ready(rgb(p.ready));
    theme.set_paused(rgb(p.paused));
    theme.set_danger(rgb(p.danger));
    theme.set_tx_cw(rgb(p.tx_cw));
    theme.set_tx_ax(rgb(p.tx_ax));
    theme.set_tx_welcome(rgb(p.tx_welcome));
    theme.set_tx_bumper(rgb(p.tx_bumper));
}

/// Which graphics the editor is changing. The picture is painted block, then program, then channel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GfxTarget {
    Channel,
    Program(u64),
    Block(u64),
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

/// Open a URL in the browser the desktop is configured to use.
fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[derive(Clone, Copy)]
enum After {
    Nothing,
    Quit,
    New,
    Open,
    Update,
}

/// Work done by the update thread, handed to the window through a channel so
/// the `Rc<RefCell<App>>` never leaves the UI thread.
enum UpdateEvent {
    Checked { result: Result<Option<update::Release>, String>, manual: bool },
    Progress(u64, u64),
    Staged(Result<update::Staged, String>),
}

#[derive(Default)]
struct UpdateState {
    release: Option<update::Release>,
    staged: Option<update::Staged>,
    checking: bool,
    downloading: bool,
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
    Update { installable: bool },
    RestartUpdate,
    Leave(After),
}

enum Focus {
    None,
    Card(usize),
}

/// What the instant tab keeps from the block it copied a picture from.
#[derive(Clone, Copy, Debug)]
struct InstantHold {
    program_id: Option<u64>,
    block_id: Option<u64>,
    graphics: GraphicsLevel,
}

impl Default for InstantHold {
    fn default() -> Self {
        InstantHold { program_id: None, block_id: None, graphics: GraphicsLevel::Channel }
    }
}

struct NextFrame {
    key: String,
    caption: String,
    frame: image::RgbaImage,
}

/// Thumbnail cache keys: path, requested width, requested height, kind. The
/// dimensions are physical pixels, so a scale-factor change makes a new entry.
type ThumbKey = (String, u32, u32, u8);
const THUMB_BOX: u8 = 0;
const THUMB_HEIGHT: u8 = 1;

struct NextPick {
    at: chrono::DateTime<Utc>,
    path: String,
    mode: Mode,
    name: String,
    block_id: Option<u64>,
    block: Option<estivi_core::graphics::Graphics>,
    program: Option<estivi_core::graphics::Graphics>,
}

pub struct App {
    pub project: Project,
    /// Last loaded or saved project. Undo copies this back.
    baseline: Project,
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
    programs: Option<ProgramsWindow>,
    cards: Vec<Card>,
    focus: Focus,
    pic: Option<u64>,
    zoom: i32,
    anchor: NaiveDate,
    pending: Pending,
    after: After,
    act_at: usize,
    /// Program open in the programs window.
    prog_id: Option<u64>,
    instant: InstantHold,
    seen_air: u64,
    seen_log: u64,
    live_preview: Option<image::RgbaImage>,
    next_key: String,
    next_valid_until: Option<chrono::DateTime<Utc>>,
    next_offset_ms: i64,
    thumbs: HashMap<ThumbKey, slint::Image>,
    /// Graphics choices shown in the instant tab, in the same order as the combo.
    instant_choices: Vec<GraphicsLevel>,
    /// Devices found by the last scan. Equipment maps a combo index back to these.
    audio_outputs: Vec<DeviceInfo>,
    audio_inputs: Vec<DeviceInfo>,
    serial_ports: Vec<String>,
    block_span: Option<(chrono::DateTime<Utc>, chrono::DateTime<Utc>)>,
    mode_sort: (i32, bool),
    pub gfx_target: GfxTarget,
    act_pick: Option<usize>,
    /// Combo callbacks that fire while the UI is being filled are ignored.
    ui_quiet: bool,
    update: UpdateState,
    update_tx: Sender<UpdateEvent>,
    update_rx: Receiver<UpdateEvent>,
}

impl App {
    pub fn ui(&self) -> MainWindow {
        self.main.upgrade().expect("main window")
    }

    fn ui_scale(&self) -> f32 {
        self.ui().window().scale_factor().max(1.0)
    }

    /// The next preview is cached until its turn starts. Any edit that could
    /// change what comes next (or how it is painted) drops that cache.
    fn invalidate_next(&mut self) {
        self.next_key.clear();
        self.next_valid_until = None;
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
        self.invalidate_next();
        if let Err(e) = self.config.save(&self.config_path) {
            log::warn!("config: {e:#}");
        }
        self.engine.send(Command::SetConfig(Box::new(self.config.clone())));
    }

    /// A program occurrence opened only so its graphics could be edited, and left empty.
    pub fn forget_empty_block(&mut self, id: u64) {
        let empty = self.project.block(id).is_some_and(|b| b.program.is_some() && !b.has_pictures() && b.mode.is_none() && b.graphics.layers.is_empty() && b.graphics.is_plain());
        if empty {
            self.project.blocks.retain(|b| b.id != id);
            if self.gfx_target == GfxTarget::Block(id) {
                self.gfx_target = GfxTarget::Channel;
            }
            self.changed();
        }
    }

    pub fn graphics_changed(&mut self, notify: bool) {
        self.dirty = true;
        self.invalidate_next();
        if notify {
        self.send_project();
        }
        self.ui().set_dirty(true);
    }

    fn changed(&mut self) {
        self.dirty = true;
        self.invalidate_next();
            self.send_project();
        self.ui().set_dirty(true);
        self.refresh_cards();
        self.push_block(false);
        self.push_lists();
        self.push_programs(false);
    }

    pub fn loaded(&mut self) {
        self.invalidate_next();
        self.thumbs.clear();
        self.baseline = self.project.clone();
        self.dirty = false;
        self.composer.set_project_dir(self.dir().as_deref());
        self.send_project();
        self.config.ui.last_project = self.path.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let _ = self.config.save(&self.config_path);
        self.push_channel();
        self.scan_devices();
        self.push_equipment();
        self.refresh_cards();
        self.push_block(true);
        self.push_lists();
        self.push_programs(true);
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
        let _ = ui.show();
    }

    fn message(&mut self, body: impl Into<String>) {
        let body = body.into();
        self.ask(1, &i18n::t("Notice", "Aviso"), &body, &i18n::t("OK", "Aceptar"), "", "", Pending::Message);
    }

    fn tick(&mut self) {
        self.status = self.engine.status();
        self.paint_status();
        self.poll_updates();
        // Unattended stations can run for weeks, so the daily check is also
        // repeated from the timer, not only at startup.
        self.maybe_check_updates();
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
        let tz = self.project.grid.timezone;
        let next = match &s.next {
            Some(n) => format!("{} · {}", n.name, view::when(n.at, s.clock_offset_ms, tz)),
            None => i18n::t("Nothing queued", "Nada en cola"),
        };
        ui.set_next_up(next.into());
        ui.set_agenda(s.agenda);
        ui.set_busy(s.busy);
        ui.set_busy_reason(s.busy_reason.as_str().into());
        ui.set_instant_ok(!s.busy);
        ui.set_instant_why(s.busy_reason.as_str().into());
        ui.set_clock_note(view::clock_line(s.clock_offset_ms, &s.clock_error, tz).into());
        if let Some(err) = &s.error {
            ui.set_note(err.as_str().into());
        }
        if s.on_air_rev != self.seen_air {
            self.seen_air = s.on_air_rev;
            let scale = self.ui_scale();
            let box_w = (LIVE_W * scale).round().max(1.0) as u32;
            let box_h = (LIVE_H * scale).round().max(1.0) as u32;
            self.live_preview = s.on_air.as_ref().and_then(|a| a.image.as_ref()).map(|img| view::fit_contain(img, box_w, box_h));
        }
        if let (Some(preview), Some(air)) = (self.live_preview.as_ref(), s.on_air.as_ref()) {
            let progress = if air.scan_ms <= 1.0 { 1.0 } else { ((air.elapsed_ms - air.scan_at) / air.scan_ms).clamp(0.0, 1.0) };
            ui.set_on_air_image(view::to_image(&view::reveal(preview, progress as f32)));
            ui.set_has_image(true);
        } else if s.on_air.is_none() {
            self.live_preview = None;
            ui.set_has_image(false);
        }
        let now = Utc::now() + TimeDelta::milliseconds(s.clock_offset_ms);
        let head = self.block_span.and_then(|(start, end)| {
            if now < start || now > end {
                return None;
            }
            let span = (end - start).num_milliseconds().max(1) as f32;
            Some((now - start).num_milliseconds().max(0) as f32 / span)
        });
        ui.set_playhead(head.unwrap_or(-1.0));
        if s.log_rev != self.seen_log {
            self.seen_log = s.log_rev;
            let line = s.last_log.as_ref().map(|e| format!("#{} {} {}", e.serial, e.program, log_result(&e.result))).unwrap_or_default();
            ui.set_log_text(line.as_str().into());
            ui.set_last_log(line.into());
        }
        let listen = format!(
            "CW {}   DTMF {}   {:+.0} dB",
            if s.listen.cw_text.is_empty() { "—" } else { s.listen.cw_text.as_str() },
            if s.listen.dtmf_text.is_empty() { "—" } else { s.listen.dtmf_text.as_str() },
            s.listen.level_db
        );
        ui.set_listen_text(listen.into());
        self.refresh_next();
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
                    selected: matches!(self.focus, Focus::Card(n) if n == i),
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

    /// The program that owns the card on the grid, when the card is one of its occurrences.
    fn card_program(&self) -> Option<u64> {
        self.card().and_then(|c| (c.program_id >= 0).then_some(c.program_id as u64))
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
        let Some(card) = self.card().cloned() else {
            ui.set_has_block(false);
            ui.set_instant_can_use_block(false);
            ui.set_inherited(false);
            ui.set_ghost(false);
            ui.set_form_when("".into());
            ui.set_block_title("".into());
            ui.set_day_hint("".into());
            ui.set_pics(ModelRc::new(VecModel::from(Vec::<PicItem>::new())));
            ui.set_steps(ModelRc::new(VecModel::from(Vec::<StepItem>::new())));
            ui.set_timing_caption("".into());
            ui.set_tx_total("".into());
            ui.set_clips(ModelRc::new(VecModel::from(Vec::<TxItem>::new())));
            ui.set_ticks(ModelRc::new(VecModel::from(Vec::<TickItem>::new())));
            self.block_span = None;
            ui.set_playhead(-1.0);
            return;
        };
        let inherited = card.program_id >= 0;
        ui.set_has_block(true);
        ui.set_instant_can_use_block(self.pic.is_some());
        ui.set_inherited(inherited);
        ui.set_ghost(inherited && card.block_id < 0);
        let wd = card.date.weekday().num_days_from_monday() as usize;
        let interval = if inherited {
            self.project.program(card.program_id as u64).map(|p| p.days[wd].interval_min as i32).unwrap_or(6)
        } else if card.block_id >= 0 {
            self.project.block(card.block_id as u64).map(|b| b.interval_min as i32).unwrap_or(6)
        } else {
            6
        };
        ui.set_interval(interval);
        if inherited {
            let en_day = weekday_name(wd, false);
            let es_day = weekday_name(wd, true);
            let (every_en, every_es) = if interval == 0 {
                ("one start".to_string(), "un arranque".to_string())
            } else {
                (format!("every {interval} min"), format!("cada {interval} min"))
            };
            ui.set_form_when(i18n::t(
                &format!("{en_day} {}–{}, {every_en}. The day and the hours come from the program.", card.start_hm, card.end_hm),
                &format!("{es_day} {}–{}, {every_es}. El día y el horario vienen del programa.", card.start_hm, card.end_hm),
            ).into());
        } else {
            ui.set_form_when("".into());
            if texts {
                ui.set_form_name(card.name.as_str().into());
                ui.set_form_start(card.start_hm.as_str().into());
                ui.set_form_end(card.end_hm.as_str().into());
            }
        }
        ui.set_block_title(format!("{}  {}  {}", card.name, card.date.format("%Y-%m-%d"), format!("{}–{}", card.start_hm, card.end_hm)).into());
        ui.set_day_hint(if inherited {
            i18n::t("Turns this date off. The other days of the program stay as they are.", "Apaga esta fecha. Los otros días del programa siguen igual.").into()
        } else {
            i18n::t("Turns this block off. It stays on the grid, in gray.", "Apaga este bloque. Sigue en la grilla, en gris.").into()
        });
        let occ = self.occurrence_of(&card);
        ui.set_block_on(occ.as_ref().map(|o| o.block_enabled).unwrap_or(true));
        let turns = if let Some(occ) = &occ {
            air::plan_occurrence(&self.project, occ, self.today(), self.config.ptt.lead_ms, self.config.ptt.tail_ms, self.status.serial, self.now())
        } else {
            Vec::new()
        };
        let pics = occ.as_ref().map(|o| self.picture_rows(o)).unwrap_or_default();
        let clips = occ.as_ref().map(|o| self.clip_rows(&turns, o.start, o.end)).unwrap_or_default();
        let (steps, timing_caption, tx_total) = self.timing_rows(&turns);
        let mode = self.pic.and_then(|id| occ.as_ref().and_then(|o| o.images.iter().find(|i| i.id == id))).and_then(|i| i.mode);
        ui.set_pic_mode(mode.map(|m| view::mode_index(m) + 1).unwrap_or(0));
        let block_mode = card.block_id.try_into().ok().and_then(|id: u64| self.project.block(id)).and_then(|b| b.mode);
        self.ui_quiet = true;
        ui.set_block_mode(block_mode.map(|m| view::mode_index(m) + 1).unwrap_or(0));
        self.ui_quiet = false;
        ui.set_pics(ModelRc::new(VecModel::from(pics)));
        ui.set_clips(ModelRc::new(VecModel::from(clips)));
        let ticks = occ.as_ref().map(|o| time_ticks(&o.start_hm, &o.end_hm, o.interval_min)).unwrap_or_default();
        ui.set_ticks(ModelRc::new(VecModel::from(ticks)));
        ui.set_steps(ModelRc::new(VecModel::from(steps)));
        ui.set_timing_caption(timing_caption.into());
        ui.set_tx_total(tx_total.into());
        self.block_span = occ.as_ref().map(|o| (o.start, o.end));
    }

    /// The block's pictures, in stored order. Disabled rows stay here so they can be
    /// turned back on; the time rows live in the timing section instead.
    fn picture_rows(&mut self, occ: &Occurrence) -> Vec<PicItem> {
        let mut rows = Vec::new();
        for image in &occ.images {
            if image.path.trim().is_empty() {
                continue;
            }
            let mode = self.project.picture_mode(image.mode, occ.block_id, occ.program_id);
            let role = match image.role {
                ImageRole::Normal => 0,
                ImageRole::Pre { .. } => 1,
                ImageRole::Post { .. } => 2,
            };
            let mut detail = format!("{} · {}", mode.spec().short, view::duration(mode.spec().total_ms()));
            if !image.enabled {
                detail.push_str(" · ");
                detail.push_str(i18n::ts("off", "apagada"));
            }
            let thumb = self.thumb(&image.path, LIST_W, LIST_H, THUMB_BOX);
            rows.push(PicItem {
                id: image.id as i32,
                label: view::file_name(&image.path).into(),
                detail: detail.into(),
                on: image.enabled,
                role,
                selected: self.pic == Some(image.id),
                has_thumb: thumb.is_some(),
                thumb: thumb.unwrap_or_default(),
            });
        }
        rows
    }

    fn clip_rows(&mut self, turns: &[air::Turn], start: chrono::DateTime<Utc>, end: chrono::DateTime<Utc>) -> Vec<TxItem> {
        let span = (end - start).num_milliseconds().max(1) as f64;
        let mut rows = Vec::new();
        for turn in turns {
            let base = (turn.at - start).num_milliseconds().max(0) as f64;
            let mut cursor = base;
            let mut after_picture = false;
            for piece in &turn.pieces {
                let id = if piece.image_id > 0 { piece.image_id as i32 } else { turn.image_id as i32 };
                let kind = match piece.kind {
                    PieceKind::Vox => 0,
                    PieceKind::Cw => 1,
                    PieceKind::Ax25 | PieceKind::Dtmf => 2,
                    PieceKind::Ax25Mode => 3,
                    PieceKind::Vis => 4,
                    PieceKind::Welcome => 5,
                    PieceKind::Bumper => 6,
                    PieceKind::Image => 7,
                    PieceKind::Bye => 8,
                };
                if piece.kind == PieceKind::Image {
                    after_picture = true;
                }
                let label = match piece.kind {
                    PieceKind::Vox => "VOX".to_string(),
                    PieceKind::Cw => "CW".to_string(),
                    PieceKind::Ax25 | PieceKind::Dtmf => "AX.25".to_string(),
                    PieceKind::Ax25Mode => format!("AX.25 {}", piece.mode.map(|m| m.spec().short).unwrap_or("")),
                    PieceKind::Vis => "VIS".to_string(),
                    PieceKind::Welcome => i18n::t("Welcome", "Bienvenida"),
                    PieceKind::Bumper => format!("{}  {}", i18n::t(if after_picture { "Post" } else { "Pre" }, if after_picture { "Después" } else { "Antes" }), view::file_name(&piece.path)),
                    PieceKind::Image => format!("{}  ·  {}", view::file_name(&piece.path), view::span(piece.ms)),
                    PieceKind::Bye => i18n::t("Bye", "Despedida"),
                };
                let thumb = if piece.kind == PieceKind::Image { self.thumb(&piece.path, 0, CLIP_H, THUMB_HEIGHT) } else { None };
                let x = (cursor / span).clamp(0.0, 1.0) as f32;
                let w = (piece.ms / span).clamp(0.0015, 1.0) as f32;
                rows.push(TxItem {
                    x,
                    w: w.min(1.0 - x).max(0.0015),
                    label: label.into(),
                    detail: view::span(piece.ms).into(),
                    id,
                    selected: self.pic == Some(id as u64),
                    has_thumb: thumb.is_some(),
                    thumb: thumb.unwrap_or_default(),
                    kind,
                });
                cursor += piece.ms;
            }
        }
        rows
    }

    /// The selected transmission, or the first one, as the full time calculation:
    /// start, every piece, PTT, the silence after it, and the total.
    fn timing_rows(&self, turns: &[air::Turn]) -> (Vec<StepItem>, String, String) {
        let Some(turn) = self
            .pic
            .and_then(|id| turns.iter().find(|t| t.pieces.iter().any(|p| p.image_id == id as i64)))
            .or_else(|| turns.first())
        else {
            return (
                Vec::new(),
                i18n::t("No picture in this block is ready to transmit.", "Ninguna imagen de este bloque está lista para salir.").into(),
                String::new(),
            );
        };
        let tz = self.project.grid.timezone;
        let mut steps = Vec::new();
        steps.push(StepItem { label: i18n::t("Start", "Comienzo").into(), time: view::clock_hms(turn.at, tz).into(), dim: false });
        let mut after_picture = false;
        for piece in &turn.pieces {
            if piece.kind == PieceKind::Image {
                after_picture = true;
            }
            let label = match piece.kind {
                PieceKind::Image | PieceKind::Welcome | PieceKind::Bye => {
                    format!("{}  {}", piece_name(piece.kind), view::file_name(&piece.path))
                }
                PieceKind::Bumper => format!(
                    "{}  {}",
                    i18n::t(if after_picture { "Post bumper" } else { "Pre bumper" }, if after_picture { "Cortina posterior" } else { "Cortina previa" }),
                    view::file_name(&piece.path)
                ),
                PieceKind::Ax25Mode => format!("{}  {}", piece_name(piece.kind), piece.mode.map(|m| m.spec().short).unwrap_or("")),
                _ => piece_name(piece.kind),
            };
            steps.push(StepItem { label: label.into(), time: view::span(piece.ms).into(), dim: !turn.airs });
        }
        let ptt = f64::from(turn.lead_ms + turn.tail_ms);
        if ptt > 0.0 {
            steps.push(StepItem {
                label: i18n::t("PTT lead + tail", "PTT entrada + cola").into(),
                time: view::span(ptt).into(),
                dim: !turn.airs,
            });
        }
        let until = turn.next_at.unwrap_or(turn.block_end);
        let gap = (until - turn.at).num_milliseconds() as f64 - turn.total_ms();
        if gap > 500.0 {
            steps.push(StepItem { label: i18n::t("Silence", "Silencio").into(), time: view::span(gap).into(), dim: false });
        }
        let index = turns.iter().position(|t| t.at == turn.at).unwrap_or(0) + 1;
        let name = turn
            .pieces
            .iter()
            .find(|p| p.kind == PieceKind::Image)
            .map(|p| view::file_name(&p.path))
            .unwrap_or_else(|| turn.name.clone());
        let mut caption = format!("{name}  ·  {}  ·  {index}/{}", view::clock_hms(turn.at, tz), turns.len());
        if !turn.airs {
            caption.push_str("  ·  ");
            caption.push_str(i18n::ts("does not fit", "no entra"));
        }
        let end = turn.at + TimeDelta::milliseconds(turn.total_ms().round() as i64);
        let mut total = format!("{}  {}", i18n::t("Total", "Total"), view::duration(turn.total_ms()));
        if turn.airs {
            total.push_str("  ·  ");
            total.push_str(i18n::ts("ends", "termina"));
            total.push(' ');
            total.push_str(&view::clock_hms(end, tz));
        } else {
            total.push_str("  ·  ");
            total.push_str(i18n::ts("does not fit", "no entra"));
        }
        (steps, caption, total)
    }

    /// A thumbnail pre-scaled with a bicubic filter to the physical size the
    /// widget uses, so the renderer does not resample it again. `kind` is a box
    /// (contain) or a fixed height (aspect kept).
    fn thumb(&mut self, path: &str, box_w: u32, box_h: u32, kind: u8) -> Option<slint::Image> {
        if path.trim().is_empty() {
            return None;
        }
        let scale = self.ui_scale();
        let (tw, th) = match kind {
            THUMB_BOX => (((box_w as f32) * scale).round().max(1.0) as u32, ((box_h as f32) * scale).round().max(1.0) as u32),
            _ => (0, ((box_h as f32) * scale).round().max(1.0) as u32),
        };
        let key = (path.to_string(), tw, th, kind);
        if let Some(img) = self.thumbs.get(&key) {
            return Some(img.clone());
        }
        let path_buf = self.resolve(path);
        let fitted = if kind == THUMB_BOX {
            self.composer.cache.fitted(&path_buf, tw, th, Fit::Contain).ok()?
        } else {
            let src = self.composer.cache.original(&path_buf).ok()?;
            let tw = ((src.width() as f32 * th as f32) / src.height().max(1) as f32).round().max(1.0) as u32;
            self.composer.cache.fitted(&path_buf, tw, th, Fit::Stretch).ok()?
        };
        let img = view::to_image(&fitted);
        self.thumbs.insert(key, img.clone());
        Some(img)
    }

    fn refresh_next(&mut self) {
        let now = self.now();
        if self.next_offset_ms != self.status.clock_offset_ms {
            self.next_valid_until = None;
        }
        if self.next_valid_until.is_some_and(|until| now < until) {
            return;
        }
        self.next_offset_ms = self.status.clock_offset_ms;
        let Some(best) = self.upcoming_pick() else {
            self.next_valid_until = Some(now + TimeDelta::seconds(1));
            let ui = self.ui();
            if !self.next_key.is_empty() || ui.get_has_next() || ui.get_next_caption().is_empty() {
                self.next_key.clear();
                ui.set_has_next(false);
                ui.set_next_caption(i18n::t("Nothing queued", "Nada en cola").into());
            }
            return;
        };
        self.next_valid_until = Some(best.at);
        let key = self.next_key(&best);
        if key == self.next_key {
            return;
        }
        let scale = self.ui_scale();
        let box_w = (NEXT_W * scale).round().max(1.0) as u32;
        let box_h = (NEXT_H * scale).round().max(1.0) as u32;
        let found = self.next_frame(&best, key, box_w, box_h);
        self.next_key = found.key;
        let ui = self.ui();
        ui.set_next_caption(found.caption.into());
        ui.set_next_image(view::to_image(&found.frame));
        ui.set_has_next(true);
    }

    /// The next picture that will actually go out: the first future turn, with
    /// the graphics of that block, its program, and the channel. Editing any of
    /// those, or moving another block in front, changes the key and redraws the pane.
    fn upcoming_pick(&self) -> Option<NextPick> {
        let today = self.today();
        let now = self.now();
        let occs = schedule::occurrences(&self.project, today - TimeDelta::days(1), today + TimeDelta::days(14), today);
        let lead = self.config.ptt.lead_ms;
        let tail = self.config.ptt.tail_ms;
        // The earliest future turn, across every block. The first block that is
        // still open can have its next picture later than a block that starts sooner.
        let mut best: Option<NextPick> = None;
        for occ in occs {
            if !occ.transmits() {
                continue;
            }
            let turns = air::plan_occurrence(&self.project, &occ, today, lead, tail, self.status.serial + 1, now);
            let Some(turn) = turns.into_iter().find(|t| t.airs && t.at > now) else { continue };
            if best.as_ref().is_some_and(|b| b.at <= turn.at) {
                continue;
            }
            let Some(image) = turn.pieces.iter().find(|p| p.kind == PieceKind::Image && !p.path.trim().is_empty()) else { continue };
            let mode = image.mode.unwrap_or_else(|| self.project.picture_mode(None, occ.block_id, occ.program_id));
            best = Some(NextPick {
                at: turn.at,
                path: image.path.clone(),
                mode,
                name: occ.name.clone(),
                block_id: occ.block_id,
                block: occ.block_id.and_then(|id| self.project.block(id)).map(|b| b.graphics.clone()),
                program: occ.program_id.and_then(|id| self.project.program(id)).map(|p| p.graphics.clone()),
            });
        }
        best
    }

    fn next_key(&self, best: &NextPick) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            best.path,
            best.mode.name(),
            best.at,
            best.block_id.unwrap_or(0),
            best.block.as_ref().map(gfx_stamp).unwrap_or(0),
            best.program.as_ref().map(gfx_stamp).unwrap_or(0),
            gfx_stamp(&self.project.graphics)
        )
    }

    fn next_frame(&mut self, best: &NextPick, key: String, box_w: u32, box_h: u32) -> NextFrame {
        let live = estivi_core::compose::LiveText { program: best.name.clone(), block: best.name.clone(), ..Default::default() };
        let ctx = estivi_core::compose::data_ctx(
            &self.project,
            best.at,
            self.status.serial + 1,
            best.mode,
            &live,
            self.project.freq_hz,
            &self.status.last_cw,
            &self.status.last_dtmf,
        );
        let dir = self.dir();
        let full = self.composer.frame_chain(&self.project, dir.as_deref(), Some(&best.path), None, best.block.as_ref(), best.program.as_ref(), best.mode, &ctx);
        let frame = view::fit_contain(&full, box_w, box_h);
        let caption = format!("{}  {}  {}", best.name, view::clock_hm(best.at, self.project.grid.timezone), view::file_name(&best.path));
        NextFrame { key, caption, frame }
    }

    fn undo(&mut self) {
        self.project = self.baseline.clone();
        self.dirty = false;
        self.thumbs.clear();
        self.invalidate_next();
        self.composer.set_project_dir(self.dir().as_deref());
        self.send_project();
        self.push_channel();
        self.push_equipment();
        self.refresh_cards();
        self.push_block(true);
        self.push_lists();
        self.push_programs(true);
        self.ui().set_dirty(false);
        self.note(i18n::t("Changes discarded.", "Cambios descartados."));
    }

    fn clear_selection(&mut self) {
        self.focus = Focus::None;
        self.pic = None;
        self.refresh_cards();
        self.push_block(false);
    }

    fn prepare_block_graphics(&mut self) -> bool {
        let Some(card) = self.card().cloned() else {
            self.note(i18n::t("Pick a block on the grid first.", "Elegí un bloque en la grilla primero."));
            return false;
        };
        let id = if card.block_id >= 0 {
            card.block_id as u64
        } else if card.program_id >= 0 {
            let id = self.project.ensure_program_block(card.program_id as u64, card.date).id;
            self.changed();
            id
        } else {
            return false;
        };
        self.gfx_target = GfxTarget::Block(id);
        true
    }

    fn prepare_instant_graphics(&mut self) -> bool {
        match self.instant.graphics {
            GraphicsLevel::None => {
                self.note(i18n::t("Choose Channel, Program or Block graphics first.", "Elegí primero gráfica de canal, programa o bloque."));
                false
            }
            GraphicsLevel::Channel => {
                self.gfx_target = GfxTarget::Channel;
                true
            }
            GraphicsLevel::Program => match self.instant.program_id {
                Some(id) if self.project.program(id).is_some() => {
                    self.gfx_target = GfxTarget::Program(id);
                    true
                }
                _ => {
                    self.note(i18n::t("That program no longer exists. Use the selected picture again.", "Ese programa ya no existe. Volvé a usar la imagen elegida."));
                    false
                }
            },
            GraphicsLevel::Block => match self.instant.block_id {
                Some(id) if self.project.block(id).is_some() => {
                    self.gfx_target = GfxTarget::Block(id);
                    true
                }
                _ => {
                    self.note(i18n::t("That block no longer exists. Use the selected picture again.", "Ese bloque ya no existe. Volvé a usar la imagen elegida."));
                    false
                }
            },
        }
    }

    fn choose_theme(&mut self, index: i32) {
        let id = THEME_IDS.get(index.max(0) as usize).copied().unwrap_or("dark");
        if self.config.ui.theme == id {
            return;
        }
        self.config.ui.theme = id.to_string();
        self.config.ui.light = palette(id).light;
        self.paint_theme();
        self.save_config();
    }

    fn paint_theme(&self) {
        let id = self.config.ui.theme.as_str();
        let light = self.config.ui.light;
        apply_theme_to(&self.ui().global::<Theme>(), id, light);
        self.ui().invoke_sync_palette();
        if let Some(ed) = &self.editor {
            ed.apply_theme(id, light);
        }
        if let Some(win) = &self.programs {
            apply_theme_to(&win.global::<Theme>(), id, light);
            win.invoke_sync_palette();
        }
        if let Some(win) = &self.modes {
            apply_theme_to(&win.global::<Theme>(), id, light);
            win.invoke_sync_palette();
        }
    }

    fn push_lists(&mut self) {
        let act: Vec<String> = self.project.activations.iter().enumerate().map(|(i, a)| {
            let mark = if i == self.act_at { "▸ " } else { "  " };
            let code = a.triggers.first().map(|t| t.code.as_str()).unwrap_or("—");
            format!("{mark}{} {}", a.name, code)
        }).collect();
        let ui = self.ui();
        ui.set_act_list(if act.is_empty() { i18n::t("No activations yet.", "Todavía no hay activaciones.").into() } else { act.join("\n").into() });
        self.push_activation();
        self.push_instant();
    }

    /// Names, thumbnails and the graphics choices of the instant tab. The paths and
    /// checkboxes are owned by the window; this only reflects them.
    fn push_instant(&mut self) {
        let ui = self.ui();
        let path = ui.get_instant_path().to_string();
        let thumb = self.thumb(&path, 0, INSTANT_H, THUMB_HEIGHT);
        ui.set_instant_file(view::file_name(&path).into());
        ui.set_instant_has_thumb(thumb.is_some());
        ui.set_instant_thumb(thumb.unwrap_or_default());
        ui.set_instant_pre_name(view::file_name(&ui.get_instant_pre()).into());
        ui.set_instant_post_name(view::file_name(&ui.get_instant_post()).into());
        ui.set_instant_welcome_name(view::file_name(&ui.get_instant_welcome()).into());
        ui.set_instant_bye_name(view::file_name(&ui.get_instant_bye()).into());
        let mut levels = vec![GraphicsLevel::None, GraphicsLevel::Channel];
        if self.instant.program_id.is_some() {
            levels.push(GraphicsLevel::Program);
        }
        if self.instant.block_id.is_some() {
            levels.push(GraphicsLevel::Block);
        }
        if !levels.contains(&self.instant.graphics) {
            self.instant.graphics = GraphicsLevel::Channel;
        }
        let labels: Vec<SharedString> = levels.iter().map(|l| SharedString::from(graphics_label(*l))).collect();
        let index = levels.iter().position(|l| *l == self.instant.graphics).unwrap_or(1) as i32;
        ui.set_instant_graphics_choices(ModelRc::new(VecModel::from(labels)));
        ui.set_instant_graphics_index(index);
        ui.set_instant_graphics_hint(graphics_hint(self.instant.graphics).into());
        ui.set_instant_graphics_edit_ok(self.instant.graphics != GraphicsLevel::None);
        self.instant_choices = levels;
    }

    fn push_activation(&mut self) {
        let ui = self.ui();
        let labels = action_labels();
        ui.set_action_labels(ModelRc::new(VecModel::from(labels.iter().cloned().collect::<Vec<_>>())));
        let mut kinds = vec![SharedString::from(i18n::t("Add an action", "Agregar una acción"))];
        kinds.extend(labels.iter().cloned());
        ui.set_action_kinds(ModelRc::new(VecModel::from(kinds)));
        ui.set_carriers(ModelRc::new(VecModel::from(vec![
            SharedString::from("CW"),
            SharedString::from("DTMF"),
            SharedString::from("AX.25"),
        ])));
        if self.project.activations.is_empty() {
            ui.set_act_names(ModelRc::new(VecModel::from(vec![SharedString::from("—")])));
            ui.set_act_index(0);
            ui.set_actions(ModelRc::new(VecModel::from(Vec::<ActionItem>::new())));
            return;
        }
        self.act_at = self.act_at.min(self.project.activations.len() - 1);
        let names: Vec<SharedString> = self.project.activations.iter().map(|a| SharedString::from(a.name.as_str())).collect();
        ui.set_act_names(ModelRc::new(VecModel::from(names)));
        ui.set_act_index(self.act_at as i32);
        let act = &self.project.activations[self.act_at];
        ui.set_act_on(act.enabled);
        ui.set_act_name(act.name.as_str().into());
        let trigger = act.triggers.first();
        ui.set_act_source(match trigger.map(|t| t.source) { Some(TriggerSource::Cw) => 1, _ => 0 });
        ui.set_act_code(trigger.map(|t| t.code.as_str()).unwrap_or("").into());
        let items: Vec<ActionItem> = act.actions.iter().map(action_item).collect();
        ui.set_actions(ModelRc::new(VecModel::from(items)));
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
        ui.set_offset_s(p.grid.offset_s as i32);
        ui.set_mode_index(view::mode_index(p.tx.mode));
        ui.set_modes(ModelRc::new(VecModel::from(view::mode_names())));
        let mut choices = vec![SharedString::from(i18n::t("Inherited", "Heredado"))];
        choices.extend(view::mode_names());
        ui.set_mode_choices(ModelRc::new(VecModel::from(choices)));
        ui.set_themes(ModelRc::new(VecModel::from(THEME_IDS.iter().map(|id| SharedString::from(theme_label(id))).collect::<Vec<_>>())));
        ui.set_theme_index(theme_index(&self.config.ui.theme, self.config.ui.light));
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

    fn scan_devices(&mut self) {
        self.audio_outputs = audio::output_devices();
        self.audio_inputs = audio::input_devices();
        self.serial_ports = ptt::serial_ports();
    }

    pub fn refresh_devices(&mut self) {
        self.scan_devices();
        self.push_equipment();
    }

    fn maybe_check_updates(&mut self) {
        if !self.config.update.enabled {
            return;
        }
        if Utc::now().timestamp().saturating_sub(self.config.update.last_check) < UPDATE_PERIOD_S {
            return;
        }
        self.start_update_check(false);
    }

    fn start_update_check(&mut self, manual: bool) {
        if self.update.checking || self.update.downloading {
            if manual {
                self.note(i18n::t("An update check is already running.", "Ya hay una búsqueda de actualizaciones en curso."));
            }
            return;
        }
        self.update.checking = true;
        if manual {
            self.note(i18n::t("Checking for updates…", "Buscando actualizaciones…"));
        }
        let tx = self.update_tx.clone();
        std::thread::spawn(move || {
            let result = update::check().map_err(|e| format!("{e:#}"));
            let _ = tx.send(UpdateEvent::Checked { result, manual });
        });
    }

    fn poll_updates(&mut self) {
        while let Ok(event) = self.update_rx.try_recv() {
            match event {
                UpdateEvent::Progress(done, total) => {
                    let text = i18n::t("Downloading update…", "Descargando actualización…");
                    if total == 0 {
                        self.note(text);
                    } else {
                        let pct = done.saturating_mul(100).checked_div(total).unwrap_or(0).min(100);
                        self.note(format!("{text} {pct}%"));
                    }
                }
                UpdateEvent::Checked { result, manual } => self.update_checked(result, manual),
                UpdateEvent::Staged(result) => self.update_staged(result),
            }
        }
    }

    fn update_checked(&mut self, result: Result<Option<update::Release>, String>, manual: bool) {
        self.update.checking = false;
        self.config.update.last_check = Utc::now().timestamp();
        if let Err(e) = self.config.save(&self.config_path) {
            log::warn!("config: {e:#}");
        }
        match result {
            Ok(Some(release)) => {
                log::info!("update: version {} is available", release.version);
                self.ui().set_update_version(release.version.as_str().into());
                // A staged older version must not hide the new one.
                if self.update.staged.as_ref().is_some_and(|s| s.version != release.version) {
                    self.discard_staged();
                }
                self.update.release = Some(release);
                if manual || self.should_announce_update() {
                    self.show_update_dialog();
                }
            }
            Ok(None) => {
                log::info!("update: this build is current");
                self.update.release = None;
                if self.update.staged.is_none() {
                    self.ui().set_update_version("".into());
                }
                if manual {
                    self.message(format!(
                        "{} {}.",
                        i18n::t("Estivi is up to date, version", "Estivi está actualizado, versión"),
                        update::CURRENT
                    ));
                }
            }
            Err(e) => {
                log::warn!("update check: {e}");
                if manual {
                    self.message(format!(
                        "{} {e}",
                        i18n::t("Could not check for updates:", "No se pudo buscar actualizaciones:")
                    ));
                }
            }
        }
    }

    fn should_announce_update(&self) -> bool {
        let Some(release) = &self.update.release else { return false };
        self.config.update.enabled
            && !self.status.busy
            && matches!(&self.pending, Pending::None)
            && self.config.update.skipped != release.version
    }

    fn update_staged(&mut self, result: Result<update::Staged, String>) {
        self.update.downloading = false;
        match result {
            Ok(staged) => {
                log::info!("update: version {} is downloaded", staged.version);
                self.update.staged = Some(staged);
                self.note(i18n::t("Update ready to install.", "Actualización lista para instalar."));
                if !self.status.busy && matches!(&self.pending, Pending::None) {
                    self.ask_restart_update();
                }
            }
            Err(e) => {
                log::warn!("update download: {e}");
                self.note(i18n::t("The update could not be downloaded.", "No se pudo descargar la actualización."));
                self.message(format!(
                    "{} {e}",
                    i18n::t("Could not download the update:", "No se pudo descargar la actualización:")
                ));
            }
        }
    }

    fn show_update_dialog(&mut self) {
        if self.update.staged.is_some() {
            self.ask_restart_update();
            return;
        }
        let Some(release) = self.update.release.clone() else { return };
        let can = update::can_install();
        let mb = release.asset.size as f64 / (1024.0 * 1024.0);
        let body = if can {
            i18n::t(
                &format!(
                    "Estivi {} is installed. Version {} is available ({mb:.1} MB).\n\nEstivi will download it, replace this copy and restart. Save your work and finish any transmission first.",
                    update::CURRENT, release.version
                ),
                &format!(
                    "Estivi {} está instalado. La versión {} está disponible ({mb:.1} MB).\n\nEstivi la va a descargar, reemplazar esta copia y reiniciar. Guardá tu trabajo y terminá cualquier transmisión primero.",
                    update::CURRENT, release.version
                ),
            )
        } else {
            i18n::t(
                &format!(
                    "Estivi {} is installed. Version {} is available.\n\nThis copy is in a folder that cannot be written, so it cannot update itself. Download the new version from the release page.",
                    update::CURRENT, release.version
                ),
                &format!(
                    "Estivi {} está instalado. La versión {} está disponible.\n\nEsta copia está en una carpeta que no se puede escribir, así que no puede actualizarse sola. Descargá la versión nueva desde la página de la versión.",
                    update::CURRENT, release.version
                ),
            )
        };
        let title = i18n::t("Update available", "Actualización disponible");
        let (ok, alt, extra) = if can {
            (
                i18n::t("Download and install", "Descargar e instalar"),
                i18n::t("Later", "Más tarde"),
                i18n::t("Release page", "Página de la versión"),
            )
        } else {
            (i18n::t("Release page", "Página de la versión"), i18n::t("Close", "Cerrar"), String::new())
        };
        self.ask(7, &title, &body, &ok, &alt, &extra, Pending::Update { installable: can });
    }

    fn ask_restart_update(&mut self) {
        let Some(staged) = self.update.staged.as_ref() else { return };
        let body = i18n::t(
            &format!("Version {} is downloaded and ready. Restart Estivi to install it.", staged.version),
            &format!("La versión {} está descargada y lista. Reiniciá Estivi para instalarla.", staged.version),
        );
        self.ask(
            7,
            &i18n::t("Update ready", "Actualización lista"),
            &body,
            &i18n::t("Restart now", "Reiniciar ahora"),
            &i18n::t("Later", "Más tarde"),
            &i18n::t("Release page", "Página de la versión"),
            Pending::RestartUpdate,
        );
    }

    fn start_update_download(&mut self) {
        if self.update.downloading {
            self.note(i18n::t("The update is already downloading.", "La actualización ya se está descargando."));
            return;
        }
        let Some(release) = self.update.release.clone() else { return };
        if self.engine.status().busy {
            self.message(i18n::t(
                "Estivi is on the air. Try again when the transmission ends.",
                "Estivi está al aire. Probá de nuevo cuando termine la transmisión.",
            ));
            return;
        }
        self.update.downloading = true;
        self.note(i18n::t("Downloading update…", "Descargando actualización…"));
        let tx = self.update_tx.clone();
        std::thread::spawn(move || {
            let result = update::stage(&release, |done, total| {
                let _ = tx.send(UpdateEvent::Progress(done, total));
            })
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(UpdateEvent::Staged(result));
        });
    }

    fn install_update(&mut self) {
        if self.dirty {
            self.guard_dirty(After::Update);
            return;
        }
        if self.engine.status().busy {
            self.message(i18n::t(
                "Estivi is on the air. Try again when the transmission ends.",
                "Estivi está al aire. Probá de nuevo cuando termine la transmisión.",
            ));
            return;
        }
        let Some(staged) = self.update.staged.as_ref() else {
            self.show_update_dialog();
            return;
        };
        match update::install(staged) {
            Ok(()) => {
                log::info!("update: installing {}", staged.version);
                self.update.staged = None;
                let _ = slint::quit_event_loop();
            }
            Err(e) => {
                log::warn!("update install: {e:#}");
                self.message(format!(
                    "{} {e:#}",
                    i18n::t("Estivi could not install the update:", "Estivi no pudo instalar la actualización:")
                ));
            }
        }
    }

    fn discard_staged(&mut self) {
        if let Some(staged) = self.update.staged.take() {
            let _ = std::fs::remove_dir_all(&staged.dir);
        }
    }

    fn open_release_page(&self) {
        let url = self.update.release.as_ref().map(|r| r.page.as_str()).unwrap_or(update::RELEASES_URL);
        open_url(url);
    }

    fn push_equipment(&mut self) {
        let c = &self.config;
        let (outs, out_index) = audio_choices(&self.audio_outputs, &c.audio.output);
        let (ins, in_index) = audio_choices(&self.audio_inputs, &c.audio.input);
        let (ports, serial_index) = serial_choices(&self.serial_ports, &c.ptt.port);
        let ui = self.ui();
        ui.set_audio_outs(ModelRc::new(VecModel::from(outs)));
        ui.set_audio_out_index(out_index);
        ui.set_audio_ins(ModelRc::new(VecModel::from(ins)));
        ui.set_audio_in_index(in_index);
        ui.set_serial_ports(ModelRc::new(VecModel::from(ports)));
        ui.set_serial_index(serial_index);
        ui.set_ptt_index(match c.ptt.method {
            PttMethod::None => 0,
            PttMethod::Vox => 1,
            PttMethod::Serial => 2,
            PttMethod::Cat => 3,
        });
        self.ui_quiet = true;
        ui.set_rts_index(level_index(c.ptt.rts));
        ui.set_dtr_index(level_index(c.ptt.dtr));
        self.ui_quiet = false;
        ui.set_lead_ms(c.ptt.lead_ms as i32);
        ui.set_tail_ms(c.ptt.tail_ms as i32);
        ui.set_cat_host(c.cat.host.as_str().into());
        ui.set_cat_port(i32::from(c.cat.port));
        ui.set_cat_freq(c.cat.set_freq);
        ui.set_lang_index(if matches!(Lang::resolve(&c.ui.lang), Lang::Spanish) { 1 } else { 0 });
        ui.set_ntp_on(c.time.enabled);
        ui.set_ntp_server(c.time.server.as_str().into());
        ui.set_ntp_hours(c.time.period_h as i32);
        ui.set_app_version(update::CURRENT.into());
        ui.set_update_auto(c.update.enabled);
        ui.set_ptt_note(i18n::t(
            "Audio, PTT and the clock belong to this computer. The channel lives in the .estivi file.",
            "El audio, el PTT y el reloj son de esta computadora. El canal vive en el archivo .estivi.",
        ).into());
    }

    fn set_hours(&mut self, start: Option<String>, end: Option<String>) {
        let today = self.today();
        let Some(card) = self.card().cloned() else { return };
        if card.program_id >= 0 {
            return;
        }
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
        if card.program_id >= 0 {
            self.note(i18n::t(
                "These hours belong to the program. Change them in Programs.",
                "Este horario es del programa. Se cambia en Programas.",
            ));
            return;
        }
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
        let (start, end) = proposed_window(minute.max(0) as u32, 6);
        let ui = self.ui();
        ui.set_dlg_interval(6);
        ui.set_dlg_name(i18n::t("New block", "Bloque nuevo").into());
        ui.set_dlg_date(date.format("%Y-%m-%d").to_string().into());
        ui.set_dlg_start(start.into());
        ui.set_dlg_end(end.into());
        self.ask(
            2,
            &i18n::t("Empty hour", "Hora libre"),
            &i18n::t(
                "This is one common block, on this date only. It does not repeat. A program's days and hours are set in Programs.",
                "Este es un bloque común, solo esta fecha. No se repite. Los días y las horas de un programa se definen en Programas.",
            ),
            &i18n::t("Create", "Crear"),
            &i18n::t("Cancel", "Cancelar"),
            "",
            Pending::Create,
        );
    }

    fn confirm_create(&mut self, which: i32) {
        if which != 0 {
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
        let interval = ui.get_dlg_interval().max(0) as u32;
        if parse_hhmm(&start).is_none() || parse_hhmm(&end).is_none() {
            self.note(i18n::t("Use HH:MM.", "Usá HH:MM."));
            return;
        }
        let today = self.today();
        let es = self.es();
        if let Err(c) = schedule::can_place_common(&self.project, date, &start, &end, today) {
            self.message(c.text(es));
            return;
        }
        let id = self.project.next_id();
        self.project.blocks.push(Block { id, program: None, date, name, start, end, enabled: true, interval_min: interval, images: Vec::new(), ..Block::default() });
        self.close_dialog();
        self.changed();
    }

    fn weekday(&mut self, id: u64, day: usize, on: bool) {
        let today = self.today();
        let now = self.now();
        match schedule::set_weekday(&mut self.project, id, day, on, None, now, today) {
            Ok(()) => {
                self.changed();
                self.push_programs(true);
            }
            Err(schedule::EditError::NeedChoice) => {
                self.push_programs(true);
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
                self.push_programs(true);
            }
            Err(schedule::EditError::Missing) => self.push_programs(true),
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
                if self.card_program() == Some(id) {
                    self.focus = Focus::None;
                }
                if self.prog_id == Some(id) {
                    self.prog_id = None;
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
        self.push_programs(true);
    }

    fn horizon(&self) -> Option<NaiveDate> {
        let text = self.ui().get_dlg_horizon().to_string();
        let date = NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d").ok()?;
        let cap = future_limit(self.today());
        Some(if date > cap { cap } else { date })
    }

    fn open_delete_program(&mut self) {
        let Some(id) = self.prog_id else { return };
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
        if !new {
            return;
        }
        let id = self.project.next_id();
        let n = self.project.activations.len() + 1;
        self.project.activations.push(Activation {
            id,
            name: format!("{} {n}", i18n::t("Activation", "Activación")),
            triggers: vec![ActTrigger::default()],
            ..Activation::default()
        });
        self.act_at = self.project.activations.len() - 1;
        self.ui().set_side(2);
        self.changed();
    }

    fn select_activation(&mut self, index: i32) {
        if self.project.activations.is_empty() {
            return;
        }
        self.act_at = (index.max(0) as usize).min(self.project.activations.len() - 1);
        self.push_activation();
    }

    fn add_action(&mut self, kind: i32) {
        let action = match kind {
            0 => Action::Cw { text: self.project.station.call.clone(), wpm: self.project.tx.cw_wpm, tone_hz: self.project.tx.cw_tone_hz },
            1 => Action::Dtmf { sequence: String::new() },
            2 => Action::Image { path: String::new(), mode: None },
            3 => Action::Pause,
            4 => Action::Resume,
            5 => Action::Ax25 { lines: Default::default(), comment: String::new() },
            6 => Action::Command { command: String::new(), via: Carrier::Cw },
            7 => Action::Api { url: String::new(), json_path: String::new(), via: Carrier::Cw },
            8 => Action::Random { paths: Vec::new(), mode: None },
            _ => return,
        };
        let Some(act) = self.project.activations.get_mut(self.act_at) else { return };
        act.actions.push(action);
        self.changed();
    }

    fn drop_action(&mut self, index: i32) {
        let Some(act) = self.project.activations.get_mut(self.act_at) else { return };
        if (index as usize) < act.actions.len() {
            act.actions.remove(index as usize);
            self.changed();
        }
    }

    fn edit_activation(&mut self, index: i32, key: &str, value: &str) {
        if index < 0 {
            let Some(act) = self.project.activations.get_mut(self.act_at) else { return };
            match key {
                "on" => act.enabled = value == "1",
                "name" => act.name = value.to_string(),
                "code" => {
                    if act.triggers.is_empty() {
                        act.triggers.push(ActTrigger::default());
                    }
                    if let Some(t) = act.triggers.first_mut() {
                        t.code = value.to_string();
                    }
                }
                "source" => {
                    if act.triggers.is_empty() {
                        act.triggers.push(ActTrigger::default());
                    }
                    if let Some(t) = act.triggers.first_mut() {
                        t.source = if value == "cw" { TriggerSource::Cw } else { TriggerSource::Dtmf };
                    }
                }
                _ => return,
            }
            self.changed();
            return;
        }
        let i = index as usize;
        let Some(action) = self.project.activations.get_mut(self.act_at).and_then(|a| a.actions.get_mut(i)) else { return };
        match (key, action) {
            ("text", Action::Cw { text, .. } | Action::Dtmf { sequence: text } | Action::Ax25 { comment: text, .. } | Action::Command { command: text, .. }) => *text = value.to_string(),
            ("url", Action::Api { url, .. }) => *url = value.to_string(),
            ("json", Action::Api { json_path, .. }) => *json_path = value.to_string(),
            _ => return,
        }
        self.changed();
    }

    fn edit_activation_num(&mut self, index: i32, key: &str, value: i32) {
        let i = index as usize;
        let Some(action) = self.project.activations.get_mut(self.act_at).and_then(|a| a.actions.get_mut(i)) else { return };
        match (key, action) {
            ("wpm", Action::Cw { wpm, .. }) => *wpm = value as f32,
            ("tone", Action::Cw { tone_hz, .. }) => *tone_hz = value as f32,
            ("mode", Action::Image { mode, .. } | Action::Random { mode, .. }) => *mode = Some(view::mode_at(value)),
            ("via", Action::Command { via, .. } | Action::Api { via, .. }) => *via = carrier_at(value),
            _ => return,
        }
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
        let pre = (ui.get_instant_pre_on() && !pre_path.trim().is_empty())
            .then(|| Still { path: pre_path, mode: Some(view::mode_at(ui.get_instant_pre_mode())) });
        let post = (ui.get_instant_post_on() && !post_path.trim().is_empty())
            .then(|| Still { path: post_path, mode: Some(view::mode_at(ui.get_instant_post_mode())) });
        let welcome_path = ui.get_instant_welcome().to_string();
        let bye_path = ui.get_instant_bye().to_string();
        let welcome = (ui.get_instant_welcome_on() && !welcome_path.trim().is_empty())
            .then(|| Still { path: welcome_path, mode: Some(view::mode_at(ui.get_instant_welcome_mode())) });
        let bye = (ui.get_instant_bye_on() && !bye_path.trim().is_empty())
            .then(|| Still { path: bye_path, mode: Some(view::mode_at(ui.get_instant_bye_mode())) });
        let manual = air::Manual {
            cw: ui.get_instant_cw(),
            ax25: ui.get_instant_ax(),
            graphics: self.instant.graphics,
            welcome,
            pre: pre.into_iter().collect(),
            image: Still { path, mode: Some(mode) },
            post: post.into_iter().collect(),
            bye,
            program_id: self.instant.program_id,
            block_id: self.instant.block_id,
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
        let program_id = self.card_program();
        let block_id = self.card().and_then(|c| (c.block_id >= 0).then_some(c.block_id as u64));
        let program = program_id.and_then(|id| self.project.program(id)).cloned();
        let image_mode = self.project.picture_mode(normal.mode, block_id, program_id);
        self.instant.program_id = program_id;
        self.instant.block_id = block_id;
        self.instant.graphics = if block_id.is_some() {
            GraphicsLevel::Block
        } else if program_id.is_some() {
            GraphicsLevel::Program
        } else {
            GraphicsLevel::Channel
        };
        let ui = self.ui();
        ui.set_instant_path(normal.path.as_str().into());
        ui.set_instant_mode(view::mode_index(image_mode));
        ui.set_instant_pre("".into());
        ui.set_instant_post("".into());
        ui.set_instant_pre_on(false);
        ui.set_instant_post_on(false);
        ui.set_instant_welcome("".into());
        ui.set_instant_bye("".into());
        ui.set_instant_welcome_on(false);
        ui.set_instant_bye_on(false);
        if let Some(pre) = pres.first().filter(|p| !p.path.trim().is_empty()) {
            ui.set_instant_pre(pre.path.as_str().into());
            ui.set_instant_pre_mode(view::mode_index(self.project.picture_mode(pre.mode, block_id, program_id)));
            ui.set_instant_pre_on(true);
        }
        if let Some(post) = posts.first().filter(|p| !p.path.trim().is_empty()) {
            ui.set_instant_post(post.path.as_str().into());
            ui.set_instant_post_mode(view::mode_index(self.project.picture_mode(post.mode, block_id, program_id)));
            ui.set_instant_post_on(true);
        }
        if first {
            if let Some(welcome) = program.as_ref().and_then(|p| p.welcome.as_ref()).filter(|s| !s.path.trim().is_empty()) {
                ui.set_instant_welcome(welcome.path.as_str().into());
                ui.set_instant_welcome_mode(view::mode_index(self.project.still_mode(welcome.mode, program_id)));
                ui.set_instant_welcome_on(true);
            }
        }
        if last {
            if let Some(bye) = program.as_ref().and_then(|p| p.bye.as_ref()).filter(|s| !s.path.trim().is_empty()) {
                ui.set_instant_bye(bye.path.as_str().into());
                ui.set_instant_bye_mode(view::mode_index(self.project.still_mode(bye.mode, program_id)));
                ui.set_instant_bye_on(true);
            }
        }
        ui.set_side(1);
        self.push_instant();
        self.note(i18n::t("The instant tab has this picture. Transmit when you are ready.", "La pestaña instantánea tiene esta imagen. Transmití cuando quieras."));
    }

    fn on_pic(&mut self, cmd: &str, arg: i32) {
        match cmd {
            "select" => {
                self.pic = Some(arg as u64);
                self.push_block(false);
            }
            "nudge" => self.nudge(arg),
            "up" => self.nudge(-1),
            "down" => self.nudge(1),
            "toggle" => self.toggle_pic(),
            "mode" => self.set_pic_mode(arg),
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

    fn set_block_mode(&mut self, index: i32) {
        if self.ui_quiet {
            return;
        }
        let mode = if index <= 0 { None } else { Some(view::mode_at(index - 1)) };
        let Some(card) = self.card().cloned() else { return };
        let id = if card.block_id >= 0 {
            card.block_id as u64
        } else if mode.is_some() && card.program_id >= 0 {
            self.project.ensure_program_block(card.program_id as u64, card.date).id
        } else {
            return;
        };
        let Some(block) = self.project.block_mut(id) else { return };
        if block.mode == mode {
            return;
        }
        block.mode = mode;
        self.changed();
        self.refresh_cards();
        self.push_block(false);
    }

    fn set_pic_mode(&mut self, index: i32) {
        let Some(id) = self.pic else { return };
        let mut images = self.images_now();
        let next = if index <= 0 { None } else { Some(view::mode_at(index - 1)) };
        let Some(im) = images.iter_mut().find(|i| i.id == id) else { return };
        if im.mode == next {
            return;
        }
        im.mode = next;
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
        let Some(id) = self.card_program() else {
            self.note(i18n::t("Import fills a program. A common block takes pictures with Add.", "Importar llena un programa. Un bloque común recibe fotos con Agregar."));
            return;
        };
        self.import_into(id, path);
    }

    fn import_into(&mut self, id: u64, path: PathBuf) {
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
        let Some(id) = self.prog_id else { return };
        let still = path.map(|p| Still { path: self.relativize(&p), mode: Some(self.project.tx.mode) });
        if let Some(p) = self.project.program_mut(id) {
            if welcome { p.welcome = still; } else { p.bye = still; }
        }
        self.changed();
        self.push_programs(true);
    }

    fn cycle_still(&mut self, welcome: bool) {
        let Some(id) = self.prog_id else { return };
        if let Some(p) = self.project.program_mut(id) {
            let slot = if welcome { &mut p.welcome } else { &mut p.bye };
            if let Some(still) = slot {
                still.mode = next_mode(still.mode);
            }
        }
        self.changed();
        self.push_programs(true);
    }

    fn set_ends(&mut self, text: &str) {
        let Some(id) = self.prog_id else { return };
        let ends = if text.trim().is_empty() {
            None
        } else {
            let Ok(date) = NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d") else {
                self.note(i18n::t("The date is YYYY-MM-DD.", "La fecha es AAAA-MM-DD."));
                self.push_programs(true);
                return;
            };
            let next = date + TimeDelta::days(1);
            let Some(naive) = next.and_hms_opt(0, 0, 0) else { return };
            match to_utc(self.project.grid.timezone, naive) {
                Some(t) => Some(t),
                None => {
                    self.note(i18n::t("That instant does not exist on this clock.", "Ese instante no existe en este reloj."));
                    self.push_programs(true);
                    return;
                }
            }
        };
        if let Some(p) = self.project.program_mut(id) {
            p.ends = ends;
        }
        self.changed();
        self.push_programs(true);
    }

    fn on_field(&mut self, key: &str, value: &str) {
        match key {
            "name" => {
                if let Some(card) = self.card().cloned() {
                    if card.program_id < 0 && card.block_id >= 0 {
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
            "delete-block" => {
                if let Some(card) = self.card().cloned() {
                    if card.program_id < 0 && card.block_id >= 0 {
                        let id = card.block_id as u64;
                        self.ask(1, &i18n::t("Delete block", "Borrar bloque"), &i18n::t("This common block will be deleted.", "Este bloque común se va a borrar."), &i18n::t("Delete", "Borrar"), &i18n::t("Cancel", "Cancelar"), "", Pending::DeleteBlock { id });
                    }
                }
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
            "cat-host" => { self.config.cat.host = value.to_string(); self.save_config(); }
            "cat-freq" => { self.config.cat.set_freq = value == "1"; self.save_config(); }
            "ntp" => { self.config.time.enabled = value == "1"; self.config.time.prompted = true; self.save_config(); }
            "ntp-server" => { self.config.time.server = value.to_string(); self.save_config(); }
            "update-auto" => {
                self.config.update.enabled = value == "1";
                self.save_config();
                self.push_equipment();
                if self.config.update.enabled {
                    self.maybe_check_updates();
                }
            }
            "instant-clear" => {
                self.instant = InstantHold::default();
                let ui = self.ui();
                ui.set_instant_path("".into());
                ui.set_instant_pre("".into());
                ui.set_instant_post("".into());
                ui.set_instant_pre_on(false);
                ui.set_instant_post_on(false);
                ui.set_instant_welcome("".into());
                ui.set_instant_bye("".into());
                ui.set_instant_welcome_on(false);
                ui.set_instant_bye_on(false);
                self.push_instant();
            }
            "instant-pre-clear" => {
                let ui = self.ui();
                ui.set_instant_pre("".into());
                ui.set_instant_pre_on(false);
                self.push_instant();
            }
            "instant-post-clear" => {
                let ui = self.ui();
                ui.set_instant_post("".into());
                ui.set_instant_post_on(false);
                self.push_instant();
            }
            "instant-welcome-clear" => {
                let ui = self.ui();
                ui.set_instant_welcome("".into());
                ui.set_instant_welcome_on(false);
                self.push_instant();
            }
            "instant-bye-clear" => {
                let ui = self.ui();
                ui.set_instant_bye("".into());
                ui.set_instant_bye_on(false);
                self.push_instant();
            }
                _ => {}
            }
    }

    fn on_num(&mut self, key: &str, value: i32) {
        match key {
            "interval" => {
                let v = value.max(0) as u32;
                if let Some(card) = self.card().cloned() {
                    if card.program_id < 0 && card.block_id >= 0 {
                        if let Some(b) = self.project.block_mut(card.block_id as u64) {
                            b.interval_min = v;
                        }
                        self.changed();
                    }
                }
            }
            "dlg-interval" => {
                let v = value.max(0);
                self.ui().set_dlg_interval(v);
                if matches!(self.pending, Pending::Create) {
                    let start = self.ui().get_dlg_start().to_string();
                    if let Some(s) = parse_hhmm(&start) {
                        let (_, end) = proposed_window(s, v as u32);
                        self.ui().set_dlg_end(end.into());
                    }
                }
            }
            "offset" => { self.project.grid.offset_s = value.max(0) as u32; self.changed(); }
            "mode" => { self.project.tx.mode = view::mode_at(value); self.changed(); }
            "block-mode" => self.set_block_mode(value),
            "pause" => { self.project.tx.min_pause_s = value.max(0) as u32; self.changed(); }
            "next-count" => { self.project.ax25.next_count = value.max(1) as u32; self.project.ax25.by_count = true; self.changed(); }
            "next-hours" => { self.project.ax25.next_hours = value.max(1) as u32; self.project.ax25.by_count = false; self.changed(); }
            "tz" => {
                let tz = if value == 1 { GridTz::Local } else { GridTz::Utc };
                if self.project.grid.timezone == tz {
                    return;
                }
                self.project.grid.timezone = tz;
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
            "serial" => {
                let port = if value <= 0 { Some(String::new()) } else { self.serial_ports.get(value as usize - 1).cloned() };
                if let Some(port) = port {
                    self.config.ptt.port = port;
                    self.save_config();
                }
            }
            "audio-out" => {
                let device = if value <= 0 { Some(String::new()) } else { self.audio_outputs.get(value as usize - 1).map(|d| d.id.clone()) };
                if let Some(id) = device {
                    self.config.audio.output = id;
                    self.save_config();
                }
            }
            "audio-in" => {
                let device = if value <= 0 { Some(String::new()) } else { self.audio_inputs.get(value as usize - 1).map(|d| d.id.clone()) };
                if let Some(id) = device {
                    self.config.audio.input = id;
                    self.save_config();
                }
            }
            "lead" => { self.config.ptt.lead_ms = value.max(0) as u32; self.save_config(); }
            "tail" => { self.config.ptt.tail_ms = value.max(0) as u32; self.save_config(); }
            "rts" => {
                if self.ui_quiet { return; }
                self.config.ptt.rts = level_at(value);
                self.save_config();
            }
            "dtr" => {
                if self.ui_quiet { return; }
                self.config.ptt.dtr = level_at(value);
                self.save_config();
            }
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
                self.push_programs(true);
            }
            "ntp-hours" => { self.config.time.period_h = value.max(1) as u32; self.save_config(); }
            "instant-graphics" => {
                let Some(level) = self.instant_choices.get(value.max(0) as usize).copied() else { return };
                self.instant.graphics = level;
                let ui = self.ui();
                ui.set_instant_graphics_index(value);
                ui.set_instant_graphics_hint(graphics_hint(level).into());
                ui.set_instant_graphics_edit_ok(level != GraphicsLevel::None);
            }
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
            Pending::Update { installable } => {
                let installable = *installable;
                match which {
                    0 if installable => {
                        self.close_dialog();
                        self.start_update_download();
                    }
                    0 => {
                        self.close_dialog();
                        self.open_release_page();
                    }
                    1 if installable => {
                        let version = self.update.release.as_ref().map(|r| r.version.clone()).unwrap_or_default();
                        self.config.update.skipped = version;
                        if let Err(e) = self.config.save(&self.config_path) {
                            log::warn!("config: {e:#}");
                        }
                        self.close_dialog();
                        self.note(i18n::t("The update was postponed.", "La actualización quedó para más tarde."));
                    }
                    1 => self.close_dialog(),
                    2 => {
                        self.close_dialog();
                        self.open_release_page();
                    }
                    _ => self.close_dialog(),
                }
            }
            Pending::RestartUpdate => {
                match which {
                    0 => {
                        self.close_dialog();
                        self.install_update();
                    }
                    1 => self.close_dialog(),
                    2 => {
                        self.close_dialog();
                        self.open_release_page();
                    }
                    _ => self.close_dialog(),
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
            After::Update => self.install_update(),
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
                self.baseline = self.project.clone();
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

    fn open_modes(&mut self, app: Weak<RefCell<App>>) {
        if self.modes.is_none() {
            let win = match UtilsDialog::new() {
                Ok(w) => w,
                Err(e) => {
                    self.note(e.to_string());
                    return;
                }
            };
            win.on_dismiss({
                let w = win.as_weak();
                move || { if let Some(w) = w.upgrade() { let _ = w.hide(); } }
            });
            win.on_sort({
                let w = app;
                move |col| with_app(&w, |a| {
                    if a.mode_sort.0 == col {
                        a.mode_sort.1 = !a.mode_sort.1;
                    } else {
                        a.mode_sort = (col, false);
                    }
                    a.fill_modes();
                })
            });
            apply_theme_to(&win.global::<Theme>(), &self.config.ui.theme, self.config.ui.light);
            win.invoke_sync_palette();
            self.modes = Some(win);
            self.fill_modes();
        }
        if let Some(win) = &self.modes {
            apply_theme_to(&win.global::<Theme>(), &self.config.ui.theme, self.config.ui.light);
            win.invoke_sync_palette();
            let _ = win.show();
        }
    }

    fn fill_modes(&mut self) {
        let Some(win) = &self.modes else { return };
        let mut modes: Vec<Mode> = Mode::ALL.to_vec();
        let (col, desc) = self.mode_sort;
        modes.sort_by(|a, b| {
            let (sa, sb) = (a.spec(), b.spec());
            let (ca, cb) = (family_name(sa.family), family_name(sb.family));
            let ord = match col {
                1 => (sa.width, sa.height).cmp(&(sb.width, sb.height)),
                2 => ca.cmp(&cb),
                3 => sa.total_ms().total_cmp(&sb.total_ms()),
                4 => sa.vis.cmp(&sb.vis),
                _ => sa.name.cmp(sb.name),
            };
            if desc { ord.reverse() } else { ord }
        });
        let items: Vec<ModeCheat> = modes.into_iter().map(|m| {
            let s = m.spec();
            ModeCheat {
                name: s.name.into(),
                size: format!("{}×{}", s.width, s.height).into(),
                color: family_name(s.family).into(),
                time: view::duration(s.total_ms()).into(),
                vis: format!("0x{:02X}", s.vis).into(),
            }
        }).collect();
        win.set_modes(ModelRc::new(VecModel::from(items)));
    }

    fn open_programs(&mut self, from_card: bool, app: Weak<RefCell<App>>) {
        if from_card {
            if let Some(id) = self.card_program() {
                self.prog_id = Some(id);
            }
        }
        if self.programs.is_none() {
            let win = match ProgramsWindow::new() {
                Ok(w) => w,
                Err(e) => {
                    self.note(e.to_string());
                    return;
                }
            };
            win.window().on_close_requested(|| slint::CloseRequestResponse::HideWindow);
            wire_programs(&app, &self.main, &win);
            apply_theme_to(&win.global::<Theme>(), &self.config.ui.theme, self.config.ui.light);
            win.invoke_sync_palette();
            self.programs = Some(win);
        }
        if let Some(win) = &self.programs {
            apply_theme_to(&win.global::<Theme>(), &self.config.ui.theme, self.config.ui.light);
            win.invoke_sync_palette();
        }
        self.push_programs(true);
        if let Some(win) = &self.programs {
            let _ = win.show();
        }
    }

    fn create_program(&mut self) {
        let id = self.project.next_id();
        let n = self.project.programs.len() + 1;
        self.project.programs.push(Program { id, name: format!("{} {n}", i18n::t("Program", "Programa")), ..Program::default() });
        self.prog_id = Some(id);
        self.changed();
        self.push_programs(true);
        self.note(i18n::t(
            "The program has no days and no hours, so it is not on the grid yet.",
            "El programa no tiene días ni horario, así que todavía no está en la grilla.",
        ));
    }

    fn set_program_name(&mut self, value: String) {
        let Some(id) = self.prog_id else { return };
        if let Some(p) = self.project.program_mut(id) {
            p.name = value;
        }
        self.changed();
    }

    fn set_program_enabled(&mut self, on: bool) {
        let Some(id) = self.prog_id else { return };
        if let Some(p) = self.project.program_mut(id) {
            p.enabled = on;
        }
        self.changed();
    }

    fn set_program_hue(&mut self, value: i32) {
        let Some(id) = self.prog_id else { return };
        if let Some(p) = self.project.program_mut(id) {
            p.hue = (value.max(0) as u16) % 360;
        }
        self.changed();
    }

    fn set_program_mode(&mut self, index: i32) {
        if self.ui_quiet {
            return;
        }
        let mode = if index <= 0 { None } else { Some(view::mode_at(index - 1)) };
        let Some(id) = self.prog_id else { return };
        let Some(program) = self.project.program_mut(id) else { return };
        if program.mode == mode {
            return;
        }
        program.mode = mode;
        self.changed();
        self.refresh_cards();
        self.push_block(false);
    }

    fn set_program_hours(&mut self, day: usize, end: bool, value: String) {
        let Some(id) = self.prog_id else { return };
        if day > 6 {
            return;
        }
        if parse_hhmm(&value).is_none() {
            self.note(i18n::t("Use HH:MM.", "Usá HH:MM."));
            self.push_programs(true);
            return;
        }
        let Some(current) = self.project.program(id).cloned() else { return };
        let mut next = current;
        if end {
            next.days[day].end = value;
        } else {
            next.days[day].start = value;
        }
        if let Some(c) = schedule::program_conflict(&self.project, &next, Some(id), self.today()) {
            self.message(c.text(self.es()));
            self.push_programs(true);
            return;
        }
        if let Some(p) = self.project.program_mut(id) {
            p.days[day].start = next.days[day].start.clone();
            p.days[day].end = next.days[day].end.clone();
        }
        self.changed();
    }

    fn set_program_interval(&mut self, day: usize, value: i32) {
        let Some(id) = self.prog_id else { return };
        if day > 6 {
            return;
        }
        if let Some(p) = self.project.program_mut(id) {
            p.days[day].interval_min = value.max(0) as u32;
        }
        self.changed();
    }

    fn push_programs(&mut self, texts: bool) {
        if self.prog_id.is_some_and(|id| self.project.program(id).is_none()) {
            self.prog_id = None;
        }
        if self.prog_id.is_none() {
            self.prog_id = self.project.programs.first().map(|p| p.id);
        }
        let Some(win) = self.programs.take() else { return };
        let names: Vec<SharedString> = self.project.programs.iter().map(|p| {
            let label = if p.scheduled() { p.name.clone() } else { format!("{} · {}", p.name, i18n::t("no schedule", "sin horario")) };
            SharedString::from(label)
        }).collect();
        let selected = self.project.programs.iter().position(|p| Some(p.id) == self.prog_id).map(|i| i as i32).unwrap_or(-1);
        win.set_names(ModelRc::new(VecModel::from(names)));
        win.set_selected(selected);
        win.set_has_program(selected >= 0);
        let program = self.prog_id.and_then(|id| self.project.program(id)).cloned();
        let Some(p) = program.as_ref() else {
            self.programs = Some(win);
            return;
        };
        win.set_enabled(p.enabled);
        win.set_hue(i32::from(p.hue % 360));
        self.ui_quiet = true;
        win.set_modes(ModelRc::new(VecModel::from({
            let mut choices = vec![SharedString::from(i18n::t("Inherited", "Heredado"))];
            choices.extend(view::mode_names());
            choices
        })));
        win.set_mode_index(p.mode.map(|m| view::mode_index(m) + 1).unwrap_or(0));
        self.ui_quiet = false;
        win.set_on0(p.weekdays[0]);
        win.set_on1(p.weekdays[1]);
        win.set_on2(p.weekdays[2]);
        win.set_on3(p.weekdays[3]);
        win.set_on4(p.weekdays[4]);
        win.set_on5(p.weekdays[5]);
        win.set_on6(p.weekdays[6]);
        win.set_every0(p.days[0].interval_min as i32);
        win.set_every1(p.days[1].interval_min as i32);
        win.set_every2(p.days[2].interval_min as i32);
        win.set_every3(p.days[3].interval_min as i32);
        win.set_every4(p.days[4].interval_min as i32);
        win.set_every5(p.days[5].interval_min as i32);
        win.set_every6(p.days[6].interval_min as i32);
        if texts {
            win.set_pname(p.name.as_str().into());
            win.set_ends(p.ends.map(|t| grid_date(self.project.grid.timezone, t - TimeDelta::seconds(1)).format("%Y-%m-%d").to_string()).unwrap_or_default().into());
            win.set_welcome(still_label(p.welcome.as_ref()).into());
            win.set_bye(still_label(p.bye.as_ref()).into());
            win.set_start0(p.days[0].start.as_str().into());
            win.set_start1(p.days[1].start.as_str().into());
            win.set_start2(p.days[2].start.as_str().into());
            win.set_start3(p.days[3].start.as_str().into());
            win.set_start4(p.days[4].start.as_str().into());
            win.set_start5(p.days[5].start.as_str().into());
            win.set_start6(p.days[6].start.as_str().into());
            win.set_end0(p.days[0].end.as_str().into());
            win.set_end1(p.days[1].end.as_str().into());
            win.set_end2(p.days[2].end.as_str().into());
            win.set_end3(p.days[3].end.as_str().into());
            win.set_end4(p.days[4].end.as_str().into());
            win.set_end5(p.days[5].end.as_str().into());
            win.set_end6(p.days[6].end.as_str().into());
        }
        self.programs = Some(win);
    }
}

fn still_label(still: Option<&Still>) -> String {
    match still {
        Some(s) => format!("{} · {}", view::file_name(&s.path), s.mode.map(|m| m.spec().short).unwrap_or("·")),
        None => "—".into(),
    }
}

fn level_index(level: LineLevel) -> i32 {
    match level {
        LineLevel::Off => 0,
        LineLevel::Positive => 1,
        LineLevel::Negative => 2,
    }
}

fn level_at(index: i32) -> LineLevel {
    match index {
        1 => LineLevel::Positive,
        2 => LineLevel::Negative,
        _ => LineLevel::Off,
    }
}

/// Combo labels for a sound card. Index 0 is the system default. A configured device
/// that is not connected stays visible and selected, so opening the panel never
/// silently changes the radio setup.
fn audio_choices(devices: &[DeviceInfo], key: &str) -> (Vec<SharedString>, i32) {
    let key = key.trim();
    let mut labels = vec![SharedString::from(i18n::t("System default", "Predeterminado del sistema"))];
    let mut index = 0;
    for device in devices {
        labels.push(SharedString::from(device.label()));
        if index == 0 && device.same_device(key) {
            index = labels.len() as i32 - 1;
        }
    }
    if !key.is_empty() && index == 0 {
        labels.push(SharedString::from(format!("{key} ({})", i18n::t("not connected", "no conectado"))));
        index = labels.len() as i32 - 1;
    }
    (labels, index)
}

/// Combo labels for serial PTT. Index 0 is no port.
fn serial_choices(ports: &[String], key: &str) -> (Vec<SharedString>, i32) {
    let key = key.trim();
    let mut labels = vec![SharedString::from(i18n::t("None", "Ninguno"))];
    let mut index = 0;
    for port in ports {
        labels.push(SharedString::from(port.as_str()));
        if index == 0 && port == key {
            index = labels.len() as i32 - 1;
        }
    }
    if !key.is_empty() && index == 0 {
        labels.push(SharedString::from(format!("{key} ({})", i18n::t("not connected", "no conectado"))));
        index = labels.len() as i32 - 1;
    }
    (labels, index)
}

fn log_result(result: &str) -> String {
    match result {
        "Cut" | "Cortada" => i18n::t("Cut", "Cortada"),
        other => other.to_string(),
    }
}

fn time_ticks(start_hm: &str, end_hm: &str, interval_min: u32) -> Vec<TickItem> {
    let Some(start) = parse_hhmm(start_hm) else { return Vec::new() };
    let Some(end_raw) = parse_hhmm(end_hm) else { return Vec::new() };
    let end = if end_raw <= start { end_raw + 24 * 60 } else { end_raw };
    let span = (end - start).max(1) as f32;
    let mut out = vec![
        TickItem { x: 0.0, label: start_hm.into() },
        TickItem { x: 1.0, label: end_hm.into() },
    ];
    if interval_min == 0 {
        return out;
    }
    let mut step = interval_min.max(1);
    let count = (end - start) / step;
    if count > 8 {
        step = step.saturating_mul((count / 6).max(1));
    }
    let mut at = start + step;
    while at < end && out.len() < 14 {
        out.push(TickItem { x: (at - start) as f32 / span, label: format_hhmm(at).into() });
        at += step;
    }
    out
}

fn gfx_stamp(g: &estivi_core::graphics::Graphics) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in format!("{g:?}").bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn graphics_label(level: GraphicsLevel) -> String {
    match level {
        GraphicsLevel::None => i18n::t("No graphics", "Sin gráfica"),
        GraphicsLevel::Channel => i18n::t("Channel only", "Solo canal"),
        GraphicsLevel::Program => i18n::t("Program + channel", "Programa + canal"),
        GraphicsLevel::Block => i18n::t("Block + program + channel", "Bloque + programa + canal"),
    }
}

fn graphics_hint(level: GraphicsLevel) -> String {
    match level {
        GraphicsLevel::None => i18n::t("Only the picture goes out.", "Solo sale la imagen."),
        GraphicsLevel::Channel => i18n::t("The channel graphics are painted on the picture.", "La gráfica del canal se pinta sobre la imagen."),
        GraphicsLevel::Program => i18n::t("The program and channel graphics are painted on the picture.", "La gráfica del programa y la del canal se pintan sobre la imagen."),
        GraphicsLevel::Block => i18n::t("The block, program and channel graphics are painted on the picture.", "La gráfica del bloque, la del programa y la del canal se pintan sobre la imagen."),
    }
}

fn piece_name(kind: PieceKind) -> String {
    match kind {
        PieceKind::Vox => i18n::t("VOX", "VOX"),
        PieceKind::Cw => i18n::t("CW", "CW"),
        PieceKind::Ax25 | PieceKind::Dtmf => "AX.25".into(),
        PieceKind::Ax25Mode => i18n::t("AX.25 mode", "AX.25 modo"),
        PieceKind::Vis => "VIS".into(),
        PieceKind::Welcome => i18n::t("Welcome", "Bienvenida"),
        PieceKind::Bumper => i18n::t("Bumper", "Cortina"),
        PieceKind::Image => i18n::t("Image", "Imagen"),
        PieceKind::Bye => i18n::t("Bye", "Despedida"),
    }
}

fn action_labels() -> Vec<SharedString> {
    [
        i18n::t("CW", "CW"),
        "DTMF".into(),
        i18n::t("Picture", "Imagen"),
        i18n::t("Pause", "Pausa"),
        i18n::t("Resume", "Reanudar"),
        "AX.25".into(),
        i18n::t("Command", "Comando"),
        "API".into(),
        i18n::t("Random picture", "Imagen al azar"),
    ]
    .into_iter()
    .map(SharedString::from)
    .collect()
}

fn action_item(action: &Action) -> ActionItem {
    let mut item = ActionItem { wpm: 20, tone: 700, ..Default::default() };
    match action {
        Action::Cw { text, wpm, tone_hz } => {
            item.kind = 0;
            item.text = text.as_str().into();
            item.wpm = *wpm as i32;
            item.tone = *tone_hz as i32;
        }
        Action::Dtmf { sequence } => {
            item.kind = 1;
            item.text = sequence.as_str().into();
        }
        Action::Image { path, mode } => {
            item.kind = 2;
            item.path = path.as_str().into();
            item.mode = mode.map(view::mode_index).unwrap_or(0);
        }
        Action::Pause => item.kind = 3,
        Action::Resume => item.kind = 4,
        Action::Ax25 { comment, .. } => {
            item.kind = 5;
            item.text = comment.as_str().into();
        }
        Action::Command { command, via } => {
            item.kind = 6;
            item.text = command.as_str().into();
            item.via = via_index(*via);
        }
        Action::Api { url, json_path, via } => {
            item.kind = 7;
            item.url = url.as_str().into();
            item.json = json_path.as_str().into();
            item.via = via_index(*via);
        }
        Action::Random { paths, mode } => {
            item.kind = 8;
            item.path = paths.first().map(|s| s.as_str()).unwrap_or("").into();
            item.paths = paths.join(" | ").into();
            item.mode = mode.map(view::mode_index).unwrap_or(0);
        }
    }
    item
}

fn via_index(via: Carrier) -> i32 {
    match via {
        Carrier::Dtmf => 1,
        Carrier::Ax25 => 2,
        Carrier::Cw => 0,
    }
}

fn carrier_at(index: i32) -> Carrier {
    match index {
        1 => Carrier::Dtmf,
        2 => Carrier::Ax25,
        _ => Carrier::Cw,
    }
}

fn family_name(family: estivi_core::modes::Family) -> String {
    match family {
        estivi_core::modes::Family::RobotBw => i18n::t("B/W", "B/N"),
        _ => i18n::t("Color", "Color"),
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

pub fn run(config: Config, config_path: PathBuf) -> anyhow::Result<()> {
    let ui = MainWindow::new()?;
    set_logo(&ui);
    i18n::apply_slint(Lang::resolve(&config.ui.lang));
    let (update_tx, update_rx) = crossbeam_channel::unbounded();
    let api = ApiCache::default();
    let engine = Engine::start(config.clone(), config_path.clone(), config_path.with_file_name("estivi-registro.csv"), api.clone());
    let last = std::env::args_os().nth(1).map(|a| a.to_string_lossy().into_owned()).unwrap_or_else(|| config.ui.last_project.clone());
    let today = chrono::Local::now().date_naive();
    let app: Shared = Rc::new_cyclic(|me| {
        let _ = me;
        RefCell::new(App {
        project: Project::default(),
        baseline: Project::default(),
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
            programs: None,
            cards: Vec::new(),
            focus: Focus::None,
            pic: None,
            zoom: 0,
            anchor: today,
            pending: Pending::None,
            after: After::Nothing,
            act_at: 0,
            prog_id: None,
            instant: InstantHold::default(),
            seen_air: 0,
        seen_log: 0,
            live_preview: None,
            next_key: String::new(),
            next_valid_until: None,
            next_offset_ms: 0,
            thumbs: HashMap::new(),
            instant_choices: Vec::new(),
            audio_outputs: Vec::new(),
            audio_inputs: Vec::new(),
            serial_ports: Vec::new(),
            block_span: None,
            mode_sort: (0, false),
            gfx_target: GfxTarget::Channel,
            act_pick: None,
            ui_quiet: false,
            update: UpdateState::default(),
            update_tx,
            update_rx,
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
    apply_theme_to(&ui.global::<Theme>(), &app.borrow().config.ui.theme, app.borrow().config.ui.light);
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
    app.borrow_mut().maybe_check_updates();
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
    ui.on_open_editor({
        let w = w.clone();
        move || {
            with_app(&w, |a| a.gfx_target = GfxTarget::Channel);
            if let Some(app) = w.upgrade() {
                editor::open(&app);
            }
        }
    });
    ui.on_open_block_graphics({
        let w = w.clone();
        move || {
            let mut ready = false;
            with_app(&w, |a| ready = a.prepare_block_graphics());
            if ready {
                if let Some(app) = w.upgrade() {
                    editor::open(&app);
                }
            }
        }
    });
    ui.on_open_instant_graphics({
        let w = w.clone();
        move || {
            let mut ready = false;
            with_app(&w, |a| ready = a.prepare_instant_graphics());
            if ready {
                if let Some(app) = w.upgrade() {
                    editor::open(&app);
                }
            }
        }
    });
    ui.on_undo({ let w = w.clone(); move || with_app(&w, App::undo) });
    ui.on_set_theme({
        let w = w.clone();
        move |index| with_app(&w, |a| a.choose_theme(index))
    });
    ui.on_clear_selection({ let w = w.clone(); move || with_app(&w, App::clear_selection) });
    ui.on_pick_clip({
        let w = w.clone();
        move |id| with_app(&w, |a| {
            if id < 0 {
                a.pic = None;
            } else {
                a.pic = Some(id as u64);
            }
            a.push_block(false);
        })
    });
    ui.on_act_select({ let w = w.clone(); move |i| with_app(&w, |a| a.select_activation(i)) });
    ui.on_act_add({ let w = w.clone(); move |k| with_app(&w, |a| a.add_action(k)) });
    ui.on_act_drop({ let w = w.clone(); move |i| with_app(&w, |a| a.drop_action(i)) });
    ui.on_act_edit({
        let w = w.clone();
        let uiw = ui.as_weak();
        move |index, key, value| {
            if key.as_str() == "pick" {
                with_app(&w, |a| a.act_pick = Some(index.max(0) as usize));
                let uiw = uiw.clone();
                pick_native(NativePick::Images { many: false }, move |paths| {
                    let Some(ui) = uiw.upgrade() else { return };
                    ui.invoke_files_ready(SharedString::from("act-image"), paths_model(&paths));
                });
                return;
            }
            with_app(&w, |a| a.edit_activation(index, key.as_str(), value.as_str()));
        }
    });
    ui.on_act_num({ let w = w.clone(); move |index, key, value| with_app(&w, |a| a.edit_activation_num(index, key.as_str(), value)) });
    ui.on_open_modes({ let w = w.clone(); move || { let app = w.clone(); with_app(&w, |a| a.open_modes(app)); } });
    ui.on_open_programs({
        let w = w.clone();
        move |from_card| {
            let app = w.clone();
            with_app(&w, |a| a.open_programs(from_card, app));
        }
    });
    ui.on_set_light({
        let w = w.clone();
        move |light| with_app(&w, |a| a.choose_theme(if light { 1 } else { 0 }))
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
    ui.on_select_card({ let w = w.clone(); move |i| with_app(&w, |a| { a.focus = Focus::Card(i.max(0) as usize); a.pic = None; a.refresh_cards(); a.push_block(true); }) });
    ui.on_open_card({ let w = w.clone(); move |i| with_app(&w, |a| { a.focus = Focus::Card(i.max(0) as usize); a.ui().set_side(0); a.refresh_cards(); a.push_block(true); }) });
    ui.on_edge({ let w = w.clone(); move |i, end, minute| with_app(&w, |a| a.set_edge(i, end, minute)) });
    ui.on_create_at({ let w = w.clone(); move |cell, minute| with_app(&w, |a| a.open_create(cell, minute)) });
    ui.on_set_field({ let w = w.clone(); move |k, v| with_app(&w, |a| a.on_field(k.as_str(), v.as_str())) });
    ui.on_set_num({ let w = w.clone(); move |k, v| with_app(&w, |a| a.on_num(k.as_str(), v)) });
    ui.on_pic({ let w = w.clone(); move |k, v| with_app(&w, |a| a.on_pic(k.as_str(), v)) });
    ui.on_confirm({ let w = w.clone(); move |which| with_app(&w, |a| a.confirm(which)) });
    ui.on_transmit({ let w = w.clone(); move || with_app(&w, App::transmit) });
    ui.on_edit_activation({ let w = w.clone(); move |n| with_app(&w, |a| a.open_activation(n < 0)) });
    ui.on_refresh_devices({ let w = w.clone(); move || with_app(&w, App::refresh_devices) });
    ui.on_check_updates({ let w = w.clone(); move || with_app(&w, |a| a.start_update_check(true)) });
    ui.on_show_update({ let w = w.clone(); move || with_app(&w, App::show_update_dialog) });
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
                "instant" | "pre" | "post" | "welcome" | "bye" | "instant-welcome" | "instant-bye" => NativePick::Images { many: false },
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
                "prog-welcome" => a.set_still(true, paths.into_iter().next()),
                "prog-bye" => a.set_still(false, paths.into_iter().next()),
                "prog-folder" => {
                    if let Some(id) = a.prog_id {
                        if let Some(p) = paths.into_iter().next() {
                            a.import_into(id, p);
                        }
                    }
                }
                "instant" => {
                    if let Some(p) = paths.into_iter().next() {
                        a.instant = InstantHold::default();
                        let ui = a.ui();
                        ui.set_instant_path(a.relativize(&p).into());
                        ui.set_instant_file(view::file_name(&a.relativize(&p)).into());
                        ui.set_instant_mode(view::mode_index(a.project.tx.mode));
                        ui.set_instant_pre("".into());
                        ui.set_instant_post("".into());
                        ui.set_instant_pre_on(false);
                        ui.set_instant_post_on(false);
                        ui.set_instant_welcome("".into());
                        ui.set_instant_bye("".into());
                        ui.set_instant_welcome_on(false);
                        ui.set_instant_bye_on(false);
                        a.push_instant();
                    }
                }
                "pre" => {
                    if let Some(p) = paths.into_iter().next() {
                        let stored = a.relativize(&p);
                        let ui = a.ui();
                        ui.set_instant_pre_mode(view::mode_index(playlist::bumper_mode(&p)));
                        ui.set_instant_pre(stored.into());
                        ui.set_instant_pre_on(true);
                        a.push_instant();
                    }
                }
                "post" => {
                    if let Some(p) = paths.into_iter().next() {
                        let stored = a.relativize(&p);
                        let ui = a.ui();
                        ui.set_instant_post_mode(view::mode_index(playlist::bumper_mode(&p)));
                        ui.set_instant_post(stored.into());
                        ui.set_instant_post_on(true);
                        a.push_instant();
                    }
                }
                "instant-welcome" => {
                    if let Some(p) = paths.into_iter().next() {
                        let stored = a.relativize(&p);
                        let ui = a.ui();
                        ui.set_instant_welcome_mode(view::mode_index(playlist::bumper_mode(&p)));
                        ui.set_instant_welcome(stored.into());
                        ui.set_instant_welcome_on(true);
                        a.push_instant();
                    }
                }
                "instant-bye" => {
                    if let Some(p) = paths.into_iter().next() {
                        let stored = a.relativize(&p);
                        let ui = a.ui();
                        ui.set_instant_bye_mode(view::mode_index(playlist::bumper_mode(&p)));
                        ui.set_instant_bye(stored.into());
                        ui.set_instant_bye_on(true);
                        a.push_instant();
                    }
                }
                "welcome" => a.set_still(true, paths.into_iter().next()),
                "bye" => a.set_still(false, paths.into_iter().next()),
                "act-image" => {
                    if let (Some(i), Some(p)) = (a.act_pick.take(), paths.into_iter().next()) {
                        let stored = a.relativize(&p);
                        if let Some(action) = a.project.activations.get_mut(a.act_at).and_then(|act| act.actions.get_mut(i)) {
                            match action {
                                Action::Image { path, .. } => *path = stored,
                                Action::Random { paths, .. } => {
                                    if paths.is_empty() { paths.push(stored); } else { paths[0] = stored; }
                                }
                                _ => {}
                            }
                            a.changed();
                        }
                    }
                }
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

fn wire_programs(app: &Weak<RefCell<App>>, main: &slint::Weak<MainWindow>, win: &ProgramsWindow) {
    let w = app.clone();
    win.on_select({
        let w = w.clone();
        move |i| with_app(&w, |a| {
            if let Some(p) = a.project.programs.get(i.max(0) as usize) {
                a.prog_id = Some(p.id);
            }
            a.push_programs(true);
        })
    });
    win.on_create({
        let w = w.clone();
        move || with_app(&w, App::create_program)
    });
    win.on_remove({
        let w = w.clone();
        move || with_app(&w, App::open_delete_program)
    });
    win.on_set_name({
        let w = w.clone();
        move |v| with_app(&w, |a| a.set_program_name(v.to_string()))
    });
    win.on_set_enabled({
        let w = w.clone();
        move |on| with_app(&w, |a| a.set_program_enabled(on))
    });
    win.on_set_hue({
        let w = w.clone();
        move |v| with_app(&w, |a| a.set_program_hue(v))
    });
    win.on_set_mode({
        let w = w.clone();
        move |v| with_app(&w, |a| a.set_program_mode(v))
    });
    win.on_set_ends({
        let w = w.clone();
        move |v| with_app(&w, |a| a.set_ends(&v))
    });
    win.on_toggle_day({
        let w = w.clone();
        move |day, on| with_app(&w, |a| {
            let Some(id) = a.prog_id else { return };
            if (0..7).contains(&day) {
                a.weekday(id, day as usize, on);
            }
        })
    });
    win.on_set_start({
        let w = w.clone();
        move |day, v| with_app(&w, |a| {
            if (0..7).contains(&day) {
                a.set_program_hours(day as usize, false, v.to_string());
            }
        })
    });
    win.on_set_end({
        let w = w.clone();
        move |day, v| with_app(&w, |a| {
            if (0..7).contains(&day) {
                a.set_program_hours(day as usize, true, v.to_string());
            }
        })
    });
    win.on_set_every({
        let w = w.clone();
        move |day, v| with_app(&w, |a| {
            if (0..7).contains(&day) {
                a.set_program_interval(day as usize, v);
            }
        })
    });
    win.on_pick_still({
        let main = main.clone();
        move |welcome| {
            let main = main.clone();
            pick_native(NativePick::Images { many: false }, move |paths| {
                let Some(ui) = main.upgrade() else { return };
                let tag = if welcome { "prog-welcome" } else { "prog-bye" };
                ui.invoke_files_ready(SharedString::from(tag), paths_model(&paths));
            });
        }
    });
    win.on_clear_still({
        let w = w.clone();
        move |welcome| with_app(&w, |a| a.set_still(welcome, None))
    });
    win.on_mode_still({
        let w = w.clone();
        move |welcome| with_app(&w, |a| a.cycle_still(welcome))
    });
    win.on_import_folder({
        let main = main.clone();
        move || {
            let main = main.clone();
            pick_native(NativePick::Directory, move |paths| {
                let Some(ui) = main.upgrade() else { return };
                ui.invoke_files_ready(SharedString::from("prog-folder"), paths_model(&paths));
            });
        }
    });
    win.on_edit_graphics({
        let w = w.clone();
        move || {
            let mut ready = false;
            with_app(&w, |a| {
                if let Some(id) = a.prog_id {
                    a.gfx_target = GfxTarget::Program(id);
                    ready = true;
                }
            });
            if ready {
                if let Some(app) = w.upgrade() {
                    editor::open(&app);
                }
            }
        }
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
