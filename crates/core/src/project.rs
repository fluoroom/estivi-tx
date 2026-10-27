//! The channel file (`*.estivi`, TOML, `format = 2`).
//!
//! A project is one channel: one frequency, one interval, one offset. A program is the
//! repeating rule (name, weekdays, hours, welcome and goodbye). A block is one calendar
//! occurrence and owns the pictures. Machine settings stay in `config`.

use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::ax25::LineSet;
use crate::encoder::{ColorRange, CwIdent, TxOptions};
use crate::graphics::Graphics;
use crate::modes::Mode;

pub const EXTENSION: &str = "estivi";
pub const FORMAT: u32 = 2;
/// Card color: only the hue is stored. These two stay fixed so the label reads in both themes.
pub const CARD_SATURATION: f32 = 0.45;
pub const CARD_LIGHTNESS: f32 = 0.42;

fn default_interval() -> u32 {
    6
}
fn default_next_count() -> u32 {
    8
}
fn default_next_hours() -> u32 {
    6
}
fn default_wpm() -> f32 {
    20.0
}
fn default_cw_tone() -> f32 {
    800.0
}
fn default_rx_tone() -> f32 {
    700.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    /// Missing means 0, which `load` rejects. New projects are written as 2.
    #[serde(default)]
    pub format: u32,
    pub name: String,
    pub station: Station,
    pub tx: TxSettings,
    /// 0 = do not retune the radio.
    pub freq_hz: u64,
    /// Minutes between transmission starts, for the whole channel. 0 = one start per block.
    #[serde(default = "default_interval")]
    pub interval_min: u32,
    pub ax25: Ax25Config,
    pub programs: Vec<Program>,
    pub blocks: Vec<Block>,
    pub activations: Vec<Activation>,
    pub grid: Grid,
    pub graphics: Graphics,
    pub listen: Listen,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            format: FORMAT,
            name: "SSTV Channel".into(),
            station: Station::default(),
            tx: TxSettings::default(),
            freq_hz: 0,
            interval_min: default_interval(),
            ax25: Ax25Config::default(),
            programs: Vec::new(),
            blocks: Vec::new(),
            activations: Vec::new(),
            grid: Grid::default(),
            graphics: Graphics::default(),
            listen: Listen::default(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Station {
    pub call: String,
    pub operator: String,
    pub qth: String,
    pub locator: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TxSettings {
    pub mode: Mode,
    pub color_range: ColorRange,
    pub vox_preamble: bool,
    pub cw_preamble: bool,
    pub cw_id: bool,
    pub cw_text: String,
    pub cw_wpm: f32,
    pub cw_tone_hz: f32,
    pub min_pause_s: u32,
}

impl Default for TxSettings {
    fn default() -> Self {
        TxSettings {
            mode: Mode::MartinM1,
            color_range: ColorRange::Full,
            vox_preamble: false,
            cw_preamble: false,
            cw_id: true,
            cw_text: "DE {call}".into(),
            cw_wpm: default_wpm(),
            cw_tone_hz: default_cw_tone(),
            min_pause_s: 60,
        }
    }
}

impl TxSettings {
    pub fn options(&self, expand: impl Fn(&str) -> String) -> TxOptions {
        TxOptions {
            vox_preamble: self.vox_preamble,
            color_range: self.color_range,
            cw_preamble: self.cw_preamble.then(|| CwIdent { text: expand(&self.cw_text), wpm: self.cw_wpm, tone_hz: self.cw_tone_hz }),
            cw_id: self.cw_id.then(|| CwIdent { text: expand(&self.cw_text), wpm: self.cw_wpm, tone_hz: self.cw_tone_hz }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ax25Config {
    /// General info frame before and after the pictures.
    pub info: bool,
    /// Short mode frame in front of each SSTV piece.
    pub mode: bool,
    /// `true`: keep `next_count` upcoming blocks. `false`: keep blocks that start within `next_hours`.
    pub by_count: bool,
    #[serde(default = "default_next_count")]
    pub next_count: u32,
    #[serde(default = "default_next_hours")]
    pub next_hours: u32,
    /// One upcoming block per program, and none from the program on air now.
    pub dedupe: bool,
}

impl Default for Ax25Config {
    fn default() -> Self {
        Ax25Config { info: false, mode: false, by_count: true, next_count: default_next_count(), next_hours: default_next_hours(), dedupe: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Still {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Program {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    /// Monday first. All false: the program exists and occupies no time.
    pub weekdays: [bool; 7],
    pub start: String,
    pub end: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome: Option<Still>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bye: Option<Still>,
    /// Absent = no end date = days-left 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends: Option<DateTime<Utc>>,
    pub hue: u16,
}

impl Default for Program {
    fn default() -> Self {
        Program {
            id: 0,
            name: String::new(),
            enabled: true,
            weekdays: [false; 7],
            start: String::new(),
            end: String::new(),
            welcome: None,
            bye: None,
            ends: None,
            hue: 210,
        }
    }
}

impl Program {
    pub fn scheduled(&self) -> bool {
        self.weekdays.iter().any(|d| *d) && parse_hhmm(&self.start).is_some() && parse_hhmm(&self.end).is_some()
    }
}

/// One calendar occurrence. A program block is stored only while it has pictures.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Block {
    pub id: u64,
    /// Empty = a common block, which does not repeat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<u64>,
    pub date: NaiveDate,
    /// Common blocks only. A program block inherits the program's name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub start: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub end: String,
    pub enabled: bool,
    pub images: Vec<BlockImage>,
}

impl Default for Block {
    fn default() -> Self {
        Block {
            id: 0,
            program: None,
            date: NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid"),
            name: String::new(),
            start: String::new(),
            end: String::new(),
            enabled: true,
            images: Vec::new(),
        }
    }
}

impl Block {
    pub fn has_pictures(&self) -> bool {
        self.images.iter().any(|i| !i.path.trim().is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockImage {
    pub id: u64,
    pub path: String,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(flatten)]
    pub role: ImageRole,
}

impl Default for BlockImage {
    fn default() -> Self {
        BlockImage { id: 0, path: String::new(), enabled: true, mode: None, role: ImageRole::Normal }
    }
}

impl BlockImage {
    pub fn normal(id: u64, path: impl Into<String>) -> Self {
        BlockImage { id, path: path.into(), enabled: true, mode: None, role: ImageRole::Normal }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ImageRole {
    Normal,
    Pre { of: u64 },
    Post { of: u64 },
}

impl Default for ImageRole {
    fn default() -> Self {
        ImageRole::Normal
    }
}

impl ImageRole {
    pub fn target(&self) -> Option<u64> {
        match self {
            ImageRole::Normal => None,
            ImageRole::Pre { of } | ImageRole::Post { of } => Some(*of),
        }
    }

    pub fn is_pre(&self) -> bool {
        matches!(self, ImageRole::Pre { .. })
    }

    pub fn is_post(&self) -> bool {
        matches!(self, ImageRole::Post { .. })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GridTz {
    #[default]
    Utc,
    Local,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Grid {
    pub timezone: GridTz,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub offset_s: u32,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Listen {
    pub cw: bool,
    pub cw_wpm: f32,
    pub cw_tone_hz: f32,
    pub dtmf: bool,
}

impl Default for Listen {
    fn default() -> Self {
        Listen { cw: false, cw_wpm: default_wpm(), cw_tone_hz: default_rx_tone(), dtmf: false }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerSource {
    #[default]
    Dtmf,
    Cw,
}

/// Where a command or an API answer is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Carrier {
    #[default]
    Cw,
    Dtmf,
    Ax25,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ax25Lines {
    pub header: bool,
    pub freq: bool,
    pub call: bool,
    pub name: bool,
    pub range: bool,
    pub days: bool,
    pub daysleft: bool,
    pub next: bool,
}

impl Default for Ax25Lines {
    fn default() -> Self {
        Ax25Lines { header: true, freq: true, call: true, name: true, range: true, days: true, daysleft: true, next: true }
    }
}

impl Ax25Lines {
    pub fn to_set(self) -> LineSet {
        LineSet {
            header: self.header,
            freq: self.freq,
            call: self.call,
            name: self.name,
            range: self.range,
            days: self.days,
            daysleft: self.daysleft,
            next: self.next,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Activation {
    pub id: u64,
    pub enabled: bool,
    pub name: String,
    /// Any one match runs the activation.
    pub triggers: Vec<ActTrigger>,
    pub actions: Vec<Action>,
}

impl Default for Activation {
    fn default() -> Self {
        Activation { id: 0, enabled: true, name: String::new(), triggers: Vec::new(), actions: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActTrigger {
    pub source: TriggerSource,
    pub code: String,
}

impl Default for ActTrigger {
    fn default() -> Self {
        ActTrigger { source: TriggerSource::Dtmf, code: String::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    Cw {
        #[serde(default)]
        text: String,
        #[serde(default = "default_wpm")]
        wpm: f32,
        #[serde(default = "default_cw_tone")]
        tone_hz: f32,
    },
    Ax25 {
        #[serde(default)]
        lines: Ax25Lines,
        #[serde(default)]
        comment: String,
    },
    Dtmf {
        #[serde(default)]
        sequence: String,
    },
    Command {
        #[serde(default)]
        command: String,
        #[serde(default)]
        via: Carrier,
    },
    Image {
        #[serde(default)]
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<Mode>,
    },
    Random {
        #[serde(default)]
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<Mode>,
    },
    Pause,
    Resume,
    Api {
        #[serde(default)]
        url: String,
        #[serde(default)]
        json_path: String,
        #[serde(default)]
        via: Carrier,
    },
}

impl Action {
    pub fn takes_ptt(&self) -> bool {
        !matches!(self, Action::Pause | Action::Resume)
    }
}

impl Project {
    pub fn load(path: &Path) -> anyhow::Result<Project> {
        let text = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let value: toml::Value = toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        explain_reject(&value)?;
        let mut p: Project = toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        p.normalize().map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Ok(p)
    }

    pub fn normalize(&mut self) -> anyhow::Result<()> {
        self.format = FORMAT;
        for p in &mut self.programs {
            p.hue %= 360;
        }
        for b in &self.blocks {
            check_images(&b.images)?;
        }
        // A program occurrence with no pictures is not a stored block.
        self.blocks.retain(|b| b.program.is_none() || b.has_pictures());
        Ok(())
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let mut copy = self.clone();
        copy.normalize()?;
        write_atomic(path, &toml::to_string_pretty(&copy)?)
    }

    pub fn next_id(&self) -> u64 {
        let mut m = 0u64;
        for p in &self.programs {
            m = m.max(p.id);
        }
        for b in &self.blocks {
            m = m.max(b.id);
            for i in &b.images {
                m = m.max(i.id);
            }
        }
        for a in &self.activations {
            m = m.max(a.id);
        }
        m + 1
    }

    pub fn program(&self, id: u64) -> Option<&Program> {
        self.programs.iter().find(|p| p.id == id)
    }

    pub fn program_mut(&mut self, id: u64) -> Option<&mut Program> {
        self.programs.iter_mut().find(|p| p.id == id)
    }

    pub fn block(&self, id: u64) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }

    pub fn block_mut(&mut self, id: u64) -> Option<&mut Block> {
        self.blocks.iter_mut().find(|b| b.id == id)
    }

    pub fn program_block(&self, program: u64, date: NaiveDate) -> Option<&Block> {
        self.blocks.iter().find(|b| b.program == Some(program) && b.date == date)
    }

    pub fn program_block_mut(&mut self, program: u64, date: NaiveDate) -> Option<&mut Block> {
        self.blocks.iter_mut().find(|b| b.program == Some(program) && b.date == date)
    }

    /// Creates the stored row for a program occurrence. It is dropped again on save if it
    /// still has no pictures.
    pub fn ensure_program_block(&mut self, program: u64, date: NaiveDate) -> &mut Block {
        if let Some(i) = self.blocks.iter().position(|b| b.program == Some(program) && b.date == date) {
            return &mut self.blocks[i];
        }
        let id = self.next_id();
        self.blocks.push(Block { id, program: Some(program), date, enabled: true, ..Block::default() });
        self.blocks.last_mut().expect("just pushed")
    }

    pub fn mode_or(&self, mode: Option<Mode>) -> Mode {
        mode.unwrap_or(self.tx.mode)
    }
}

/// The bumper invariant. Editing functions and `load` both refuse a list that breaks it.
pub fn check_images(images: &[BlockImage]) -> anyhow::Result<()> {
    let mut seen = std::collections::HashSet::new();
    for im in images {
        anyhow::ensure!(im.id != 0 && seen.insert(im.id), "cada imagen necesita un id propio distinto de cero");
    }
    let normals: std::collections::HashSet<u64> = images.iter().filter(|i| matches!(i.role, ImageRole::Normal)).map(|i| i.id).collect();
    let mut i = 0;
    while i < images.len() {
        let mut pre_of = None;
        while i < images.len() {
            if let ImageRole::Pre { of } = images[i].role {
                anyhow::ensure!(normals.contains(&of), "una cortina tiene que apuntar a una imagen normal de la misma lista");
                if let Some(prev) = pre_of {
                    anyhow::ensure!(prev == of, "las precortinas de dos imágenes quedaron mezcladas");
                }
                pre_of = Some(of);
                i += 1;
            } else {
                break;
            }
        }
        anyhow::ensure!(i < images.len() && matches!(images[i].role, ImageRole::Normal), "una cortina quedó suelta o entre dos imágenes normales");
        let normal = images[i].id;
        if let Some(of) = pre_of {
            anyhow::ensure!(of == normal, "las precortinas tienen que ir inmediatamente arriba de su imagen");
        }
        i += 1;
        while i < images.len() {
            if let ImageRole::Post { of } = images[i].role {
                anyhow::ensure!(of == normal, "las postcortinas tienen que ir inmediatamente abajo de su imagen");
                i += 1;
            } else {
                break;
            }
        }
    }
    Ok(())
}

fn explain_reject(v: &toml::Value) -> anyhow::Result<()> {
    let format = v.get("format").and_then(toml::Value::as_integer).unwrap_or(0);
    if format > i64::from(FORMAT) {
        anyhow::bail!("este proyecto es de una versión más nueva de Estivi (format = {format})");
    }
    if format != i64::from(FORMAT) {
        anyhow::bail!("este archivo es de una versión anterior de Estivi (format = {format}) y no se convierte");
    }
    if v.get("rotation").is_some() {
        anyhow::bail!("este archivo tiene la forma anterior de Estivi (rotación) y no se convierte");
    }
    if v.get("tx").and_then(|t| t.get("fsk_id")).is_some() {
        anyhow::bail!("este archivo tiene la forma anterior de Estivi (FSK ID) y no se convierte");
    }
    if v.get("grid").and_then(|g| g.get("slots")).is_some() {
        anyhow::bail!("este archivo tiene la forma anterior de Estivi (bloques en la grilla semanal) y no se convierte");
    }
    if v.get("listen").and_then(|l| l.get("triggers")).is_some() {
        anyhow::bail!("este archivo tiene la forma anterior de Estivi (disparadores) y no se convierte");
    }
    if let Some(programs) = v.get("programs").and_then(toml::Value::as_array) {
        for p in programs {
            for key in ["path", "slots", "images", "rotation", "activations", "cw_text", "mode"] {
                if p.get(key).is_some() {
                    anyhow::bail!("este archivo tiene la forma anterior de Estivi (programa con «{key}») y no se convierte");
                }
            }
        }
    }
    if let Some(blocks) = v.get("blocks").and_then(toml::Value::as_array) {
        for b in blocks {
            for key in ["freq_hz", "every_min", "days"] {
                if b.get(key).is_some() {
                    anyhow::bail!("este archivo tiene la forma anterior de Estivi (bloque con «{key}») y no se convierte");
                }
            }
        }
    }
    Ok(())
}

/// Hue 0–359 to the card color. Saturation and lightness are fixed.
pub fn hue_rgb(hue: u16) -> [u8; 3] {
    let h = f32::from(hue % 360);
    let (s, l) = (CARD_SATURATION, CARD_LIGHTNESS);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - ((hp % 2.0) - 1.0).abs());
    let (r, g, b) = match hp as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    [r, g, b].map(|v| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

pub fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp~");
    std::fs::write(&tmp, contents).map_err(|e| anyhow::anyhow!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(())
}

pub fn resolve(base_dir: Option<&Path>, stored: &str) -> PathBuf {
    let p = PathBuf::from(stored);
    match base_dir {
        Some(base) if p.is_relative() => base.join(p),
        _ => p,
    }
}

pub fn relativize(base_dir: Option<&Path>, abs: &Path) -> String {
    if let Some(rel) = base_dir.and_then(|b| abs.strip_prefix(b).ok()) {
        if !rel.components().any(|c| matches!(c, Component::ParentDir)) {
            return rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        }
    }
    abs.to_string_lossy().into_owned()
}

/// "HH:MM" → minutes since midnight.
pub fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn format_hhmm(minutes: u32) -> String {
    let minutes = minutes % (24 * 60);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

pub fn range_label(start: &str, end: &str) -> String {
    format!("{start}-{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rich() -> Project {
        let mut p = Project::default();
        p.station.call = "LU1ABC".into();
        p.freq_hz = 145_825_000;
        p.interval_min = 6;
        p.ax25.info = true;
        p.programs.push(Program {
            id: 1,
            name: "Noticias".into(),
            weekdays: [true, true, true, true, true, false, false],
            start: "14:00".into(),
            end: "14:30".into(),
            welcome: Some(Still { path: "hola.png".into(), mode: Some(Mode::Robot36) }),
            hue: 200,
            ..Program::default()
        });
        p.blocks.push(Block {
            id: 2,
            program: Some(1),
            date: NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(),
            enabled: true,
            images: vec![
                BlockImage::normal(3, "a.png"),
                BlockImage {
                    id: 4,
                    path: "b.png".into(),
                    mode: Some(Mode::Robot12),
                    role: ImageRole::Post { of: 3 },
                    ..BlockImage::default()
                },
            ],
            ..Block::default()
        });
        p.activations.push(Activation {
            id: 5,
            name: "Pausa".into(),
            triggers: vec![ActTrigger { source: TriggerSource::Dtmf, code: "*0#".into() }],
            actions: vec![Action::Pause, Action::Cw { text: "QRX".into(), wpm: 18.0, tone_hz: 700.0 }],
            ..Activation::default()
        });
        p
    }

    #[test]
    fn the_sample_channel_loads() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ejemplo/canal-demo.estivi");
        let p = Project::load(&path).unwrap_or_else(|e| panic!("{e:#}"));
        assert_eq!(p.format, 2);
        assert_eq!(p.freq_hz, 14_230_000);
        assert_eq!(p.programs.len(), 2);
        assert!(p.programs[0].welcome.is_some());
        assert!(p.programs[1].bye.is_some());
        assert_eq!(p.blocks.len(), 1);
        assert!(p.blocks[0].program.is_none());
        assert_eq!(p.activations.len(), 2);
        assert!(p.graphics.layers.iter().any(|l| matches!(l.kind, crate::graphics::LayerKind::Qr(_))));
    }

    #[test]
    fn roundtrip_toml() {
        let p = rich();
        let s = toml::to_string_pretty(&p).unwrap();
        assert!(s.contains("format = 2"), "{s}");
        assert!(s.contains("role = \"post\""), "{s}");
        assert!(!s.contains("fsk_id"), "{s}");
        let back: Project = toml::from_str(&s).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn partial_file_keeps_the_channel_defaults() {
        let p: Project = toml::from_str("format = 2\nname = \"x\"\n[tx]\nmode = \"PD120\"\n").unwrap();
        assert_eq!(p.tx.mode, Mode::Pd120);
        assert_eq!(p.tx.cw_wpm, 20.0);
        assert_eq!(p.interval_min, 6);
        assert_eq!(p.ax25.next_count, 8);
        assert!(p.ax25.by_count);
        assert!(!p.tx.vox_preamble);
    }

    #[test]
    fn old_shape_is_rejected_and_not_rewritten() {
        let dir = std::env::temp_dir().join(format!("estivi-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("viejo.estivi");
        let text = "format = 1\nname = \"viejo\"\n[[programs]]\nid = 1\npath = \"a.png\"\n";
        std::fs::write(&path, text).unwrap();
        let err = Project::load(&path).unwrap_err().to_string();
        assert!(err.contains("no se convierte"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        let newer = "format = 3\nname = \"x\"\n";
        std::fs::write(&path, newer).unwrap();
        let err = Project::load(&path).unwrap_err().to_string();
        assert!(err.contains("más nueva"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);

        let disguised = "format = 2\n[[programs]]\nid = 1\npath = \"a.png\"\n[[blocks]]\nid = 2\nfreq_hz = 1\n";
        std::fs::write(&path, disguised).unwrap();
        let err = Project::load(&path).unwrap_err().to_string();
        assert!(err.contains("no se convierte"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), disguised);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slots_and_fsk_are_the_old_shape() {
        let err = Project::load_str("format = 2\n[grid]\nslots = []\n");
        assert!(err.contains("no se convierte"), "{err}");
        let err = Project::load_str("format = 2\n[tx]\nfsk_id = false\n");
        assert!(err.contains("FSK"), "{err}");
    }

    #[test]
    fn an_empty_program_block_is_not_stored() {
        let mut p = rich();
        p.blocks[0].images.clear();
        let dir = std::env::temp_dir().join(format!("estivi-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.estivi");
        p.save(&path).unwrap();
        let back = Project::load(&path).unwrap();
        assert!(back.blocks.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_common_block_stays() {
        let mut p = Project::default();
        p.blocks.push(Block {
            id: 1,
            name: "Especial".into(),
            date: NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            start: "18:00".into(),
            end: "18:15".into(),
            ..Block::default()
        });
        p.normalize().unwrap();
        assert_eq!(p.blocks.len(), 1);
    }

    #[test]
    fn bumper_invariant_rejects_a_bridge() {
        let images = vec![
            BlockImage::normal(1, "a.png"),
            BlockImage { id: 2, path: "b.png".into(), role: ImageRole::Post { of: 1 }, ..BlockImage::default() },
            BlockImage::normal(3, "c.png"),
        ];
        assert!(check_images(&images).is_ok());
        let bridged = vec![
            BlockImage::normal(1, "a.png"),
            BlockImage { id: 2, path: "b.png".into(), role: ImageRole::Post { of: 3 }, ..BlockImage::default() },
            BlockImage::normal(3, "c.png"),
        ];
        assert!(check_images(&bridged).is_err());
        let to_bumper = vec![
            BlockImage { id: 1, path: "a.png".into(), role: ImageRole::Pre { of: 2 }, ..BlockImage::default() },
            BlockImage { id: 2, path: "b.png".into(), role: ImageRole::Post { of: 1 }, ..BlockImage::default() },
        ];
        assert!(check_images(&to_bumper).is_err());
    }

    #[test]
    fn paths_and_hhmm() {
        let base = Path::new("/media/usb/canal");
        assert_eq!(relativize(Some(base), Path::new("/media/usb/canal/img/a.png")), "img/a.png");
        assert_eq!(relativize(Some(base), Path::new("/home/x/b.png")), "/home/x/b.png");
        assert_eq!(resolve(Some(base), "img/a.png"), base.join("img/a.png"));
        assert_eq!(parse_hhmm("21:05"), Some(21 * 60 + 5));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(format_hhmm(14 * 60 + 3), "14:03");
    }

    impl Project {
        fn load_str(s: &str) -> String {
            let dir = std::env::temp_dir().join(format!("estivi-str-{}-{}", std::process::id(), s.len()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("p.estivi");
            std::fs::write(&path, s).unwrap();
            let err = Project::load(&path).unwrap_err().to_string();
            let _ = std::fs::remove_dir_all(&dir);
            err
        }
    }
}
