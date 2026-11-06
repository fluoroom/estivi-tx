//! The channel file (`*.estivi`, TOML, `format = 2`).
//!
//! A project is one channel: one frequency and one offset. A program repeats by weekday.
//! Each weekday keeps its own hours and image interval. A common block is one date and
//! keeps its own hours and interval. A program block owns the pictures for that date.
//! Machine settings stay in `config`.

use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Datelike, NaiveDate, Utc};
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

/// Hours and image interval of one weekday. Monday is index 0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DayHours {
    pub start: String,
    pub end: String,
    /// Minutes between picture starts. 0 = one start.
    #[serde(default = "default_interval")]
    pub interval_min: u32,
}

impl Default for DayHours {
    fn default() -> Self {
        DayHours { start: String::new(), end: String::new(), interval_min: default_interval() }
    }
}

impl DayHours {
    pub fn ready(&self) -> bool {
        parse_hhmm(&self.start).is_some() && parse_hhmm(&self.end).is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Program {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    /// Monday first. All false: the program exists and occupies no time.
    pub weekdays: [bool; 7],
    /// Hours and interval of each weekday, Monday first. A day that is off keeps its hours.
    pub days: [DayHours; 7],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub welcome: Option<Still>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bye: Option<Still>,
    /// Absent = no end date = days-left 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ends: Option<DateTime<Utc>>,
    pub hue: u16,
    /// Painted after the block's graphics and before the channel's.
    #[serde(default)]
    pub graphics: Graphics,
    /// Absent = the channel's SSTV mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
}

impl Default for Program {
    fn default() -> Self {
        Program {
            id: 0,
            name: String::new(),
            enabled: true,
            weekdays: [false; 7],
            days: std::array::from_fn(|_| DayHours::default()),
            welcome: None,
            bye: None,
            ends: None,
            hue: 210,
            graphics: Graphics::default(),
            mode: None,
        }
    }
}

impl Program {
    /// The same window on every weekday. Callers that still mean "all week" use this.
    pub fn every_day(&mut self, start: &str, end: &str, interval_min: u32) {
        for day in &mut self.days {
            day.start = start.to_string();
            day.end = end.to_string();
            day.interval_min = interval_min;
        }
    }

    pub fn scheduled(&self) -> bool {
        self.weekdays.iter().enumerate().any(|(i, on)| *on && self.days[i].ready())
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
    /// Used by a common block. A program block is rewritten from the program on save.
    #[serde(default = "default_interval")]
    pub interval_min: u32,
    /// Painted on the picture before the program's graphics and the channel's.
    #[serde(default)]
    pub graphics: Graphics,
    /// Absent = the program's mode, or the channel's when the block has no program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
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
            interval_min: default_interval(),
            graphics: Graphics::default(),
            mode: None,
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
        adopt_channel_interval(&value, &mut p);
        adopt_weekday_hours(&value, &mut p);
        p.normalize().map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Ok(p)
    }

    pub fn normalize(&mut self) -> anyhow::Result<()> {
        self.format = FORMAT;
        for p in &mut self.programs {
            p.hue %= 360;
        }
        let intervals: Vec<(u64, [u32; 7])> = self
            .programs
            .iter()
            .map(|p| {
                let mut n = [0; 7];
                for (i, day) in p.days.iter().enumerate() {
                    n[i] = day.interval_min;
                }
                (p.id, n)
            })
            .collect();
        for b in &mut self.blocks {
            let Some(pid) = b.program else { continue };
            let Some((_, days)) = intervals.iter().find(|(id, _)| *id == pid) else { continue };
            let wd = b.date.weekday().num_days_from_monday() as usize;
            b.interval_min = days[wd];
        }
        for b in &self.blocks {
            check_images(&b.images)?;
        }
        // A program occurrence with nothing of its own is not a stored block.
        self.blocks.retain(|b| b.program.is_none() || b.has_pictures() || !b.graphics.layers.is_empty() || b.mode.is_some());
        Ok(())
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let mut copy = self.clone();
        copy.normalize()?;
        write_atomic(path, &toml::to_string_pretty(&copy)?)
    }

    pub fn all_api_sources(&self) -> Vec<crate::graphics::ApiSource> {
        let mut sources: Vec<_> = self.graphics.api_sources().cloned().collect();
        for program in &self.programs {
            sources.extend(program.graphics.api_sources().cloned());
        }
        for block in &self.blocks {
            sources.extend(block.graphics.api_sources().cloned());
        }
        sources
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

    /// A picture's own mode, else the block's, else the program's, else the channel's.
    pub fn picture_mode(&self, image: Option<Mode>, block_id: Option<u64>, program_id: Option<u64>) -> Mode {
        image
            .or_else(|| block_id.and_then(|id| self.block(id)).and_then(|b| b.mode))
            .or_else(|| program_id.and_then(|id| self.program(id)).and_then(|p| p.mode))
            .unwrap_or(self.tx.mode)
    }

    /// A program still: its own mode, else the program's, else the channel's.
    pub fn still_mode(&self, still: Option<Mode>, program_id: Option<u64>) -> Mode {
        still.or_else(|| program_id.and_then(|id| self.program(id)).and_then(|p| p.mode)).unwrap_or(self.tx.mode)
    }
}

/// The bumper invariant. Editing functions and `load` both refuse a list that breaks it.
pub fn check_images(images: &[BlockImage]) -> anyhow::Result<()> {
    let mut seen = std::collections::HashSet::new();
    for im in images {
        anyhow::ensure!(im.id != 0 && seen.insert(im.id), "each image needs its own id, and not zero");
    }
    let normals: std::collections::HashSet<u64> = images.iter().filter(|i| matches!(i.role, ImageRole::Normal)).map(|i| i.id).collect();
    let mut i = 0;
    while i < images.len() {
        let mut pre_of = None;
        while i < images.len() {
            if let ImageRole::Pre { of } = images[i].role {
                anyhow::ensure!(normals.contains(&of), "a bumper has to point at a normal image in the same list");
                if let Some(prev) = pre_of {
                    anyhow::ensure!(prev == of, "pre-bumpers of two images got mixed");
                }
                pre_of = Some(of);
                i += 1;
            } else {
                break;
            }
        }
        anyhow::ensure!(i < images.len() && matches!(images[i].role, ImageRole::Normal), "a bumper was left loose, or between two normal images");
        let normal = images[i].id;
        if let Some(of) = pre_of {
            anyhow::ensure!(of == normal, "pre-bumpers have to sit immediately above their image");
        }
        i += 1;
        while i < images.len() {
            if let ImageRole::Post { of } = images[i].role {
                anyhow::ensure!(of == normal, "post-bumpers have to sit immediately below their image");
                i += 1;
            } else {
                break;
            }
        }
    }
    Ok(())
}

/// Files written when the interval belonged to the channel copy that number onto
/// every program and common block that does not already name its own.
fn adopt_channel_interval(raw: &toml::Value, project: &mut Project) {
    let Some(table) = raw.as_table() else { return };
    let Some(root) = table.get("interval_min").and_then(toml::Value::as_integer) else { return };
    let root = u32::try_from(root.max(0)).unwrap_or(u32::MAX);
    let programs = table.get("programs").and_then(toml::Value::as_array);
    for (i, program) in project.programs.iter_mut().enumerate() {
        let own = programs.and_then(|a| a.get(i)).and_then(toml::Value::as_table);
        let named = own.is_some_and(|t| t.contains_key("interval_min") || t.contains_key("days"));
        if !named {
            for day in &mut program.days {
                day.interval_min = root;
            }
        }
    }
    let blocks = table.get("blocks").and_then(toml::Value::as_array);
    for (i, block) in project.blocks.iter_mut().enumerate() {
        if block.program.is_some() {
            continue;
        }
        let own = blocks.and_then(|a| a.get(i)).and_then(toml::Value::as_table).is_some_and(|t| t.contains_key("interval_min"));
        if !own {
            block.interval_min = root;
        }
    }
}

/// A program that still stores one `start`, `end` and `interval_min` copies them onto every weekday.
fn adopt_weekday_hours(raw: &toml::Value, project: &mut Project) {
    let Some(programs) = raw.get("programs").and_then(toml::Value::as_array) else { return };
    for (i, program) in project.programs.iter_mut().enumerate() {
        let Some(table) = programs.get(i).and_then(toml::Value::as_table) else { continue };
        if table.contains_key("days") {
            continue;
        }
        let start = table.get("start").and_then(toml::Value::as_str).unwrap_or("");
        let end = table.get("end").and_then(toml::Value::as_str).unwrap_or("");
        let interval = table.get("interval_min").and_then(toml::Value::as_integer);
        if start.is_empty() && end.is_empty() && interval.is_none() {
            continue;
        }
        for day in &mut program.days {
            if !start.is_empty() {
                day.start = start.to_string();
            }
            if !end.is_empty() {
                day.end = end.to_string();
            }
            if let Some(n) = interval {
                day.interval_min = u32::try_from(n.max(0)).unwrap_or(u32::MAX);
            }
        }
    }
}

fn explain_reject(v: &toml::Value) -> anyhow::Result<()> {
    let format = v.get("format").and_then(toml::Value::as_integer).unwrap_or(0);
    if format > i64::from(FORMAT) {
        anyhow::bail!("this project is from a newer version of Estivi (format = {format})");
    }
    if format != i64::from(FORMAT) {
        anyhow::bail!("this file is from an older version of Estivi (format = {format}) and is not converted");
    }
    if v.get("rotation").is_some() {
        anyhow::bail!("this file has the old Estivi shape (rotation) and is not converted");
    }
    if v.get("tx").and_then(|t| t.get("fsk_id")).is_some() {
        anyhow::bail!("this file has the old Estivi shape (FSK ID) and is not converted");
    }
    if v.get("grid").and_then(|g| g.get("slots")).is_some() {
        anyhow::bail!("this file has the old Estivi shape (blocks on the weekly grid) and is not converted");
    }
    if v.get("listen").and_then(|l| l.get("triggers")).is_some() {
        anyhow::bail!("this file has the old Estivi shape (triggers) and is not converted");
    }
    if let Some(programs) = v.get("programs").and_then(toml::Value::as_array) {
        for p in programs {
            for key in ["path", "slots", "images", "rotation", "activations", "cw_text"] {
                if p.get(key).is_some() {
                    anyhow::bail!("this file has the old Estivi shape (program with \"{key}\") and is not converted");
                }
            }
        }
    }
    if let Some(blocks) = v.get("blocks").and_then(toml::Value::as_array) {
        for b in blocks {
            for key in ["freq_hz", "every_min", "days"] {
                if b.get(key).is_some() {
                    anyhow::bail!("this file has the old Estivi shape (block with \"{key}\") and is not converted");
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
        p.ax25.info = true;
        let mut news = Program {
            id: 1,
            name: "Noticias".into(),
            weekdays: [true, true, true, true, true, false, false],
            welcome: Some(Still { path: "hola.png".into(), mode: Some(Mode::Robot36) }),
            hue: 200,
            ..Program::default()
        };
        news.every_day("14:00", "14:30", 6);
        p.programs.push(news);
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
        assert!(p.blocks.len() >= 3, "the sample keeps at least the three blocks named below");
        assert!(p.blocks.iter().any(|b| b.program.is_none() && b.name == "Noche en 80 m" && b.interval_min == 14));
        assert!(p.blocks.iter().any(|b| b.program == Some(1) && b.has_pictures()));
        assert_eq!(p.activations.len(), 2);
        assert!(p.graphics.layers.iter().any(|l| matches!(l.kind, crate::graphics::LayerKind::Qr(_))));
        assert_eq!(p.programs[0].days[0].interval_min, 10);
        assert_eq!(p.programs[0].days[0].start, "12:00");
        assert_eq!(p.programs[0].days[4].interval_min, 10);
        assert_eq!(p.programs[1].days[5].interval_min, 5);
        assert_eq!(p.programs[1].days[5].start, "23:45");
    }

    #[test]
    fn a_channel_interval_is_copied_until_each_block_names_its_own() {
        let dir = std::env::temp_dir().join(format!("estivi-interval-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.estivi");
        let text = "\
format = 2
interval_min = 10
[[programs]]
id = 1
name = \"Noticias\"
[[programs]]
id = 2
name = \"Cierre\"
interval_min = 3
[[blocks]]
id = 4
date = \"2026-10-27\"
name = \"Noche\"
start = \"21:00\"
end = \"22:00\"
";
        std::fs::write(&path, text).unwrap();
        let p = Project::load(&path).unwrap();
        assert_eq!(p.programs[0].days[0].interval_min, 10);
        assert_eq!(p.programs[1].days[0].interval_min, 3);
        assert_eq!(p.blocks[0].interval_min, 10);
        p.save(&path).unwrap();
        let saved: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(saved.get("interval_min").is_none(), "the channel no longer stores one interval");
        assert!(saved["programs"][0].get("interval_min").is_none());
        assert_eq!(saved["programs"][0]["days"][0]["interval_min"].as_integer(), Some(10));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_pair_of_hours_fills_every_weekday_and_a_day_can_differ() {
        let dir = std::env::temp_dir().join(format!("estivi-days-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.estivi");
        let legacy = "\
format = 2
[[programs]]
id = 1
name = \"Noticias\"
weekdays = [true, false, false, false, false, true, false]
start = \"14:00\"
end = \"15:00\"
interval_min = 8
";
        std::fs::write(&path, legacy).unwrap();
        let mut p = Project::load(&path).unwrap();
        assert!(p.programs[0].days.iter().all(|d| d.start == "14:00" && d.end == "15:00" && d.interval_min == 8));
        p.programs[0].days[5].start = "22:00".into();
        p.programs[0].days[5].end = "22:30".into();
        p.programs[0].days[5].interval_min = 4;
        p.save(&path).unwrap();
        let back = Project::load(&path).unwrap();
        assert_eq!(back.programs[0].days[0].start, "14:00");
        assert_eq!(back.programs[0].days[5].start, "22:00");
        assert_eq!(back.programs[0].days[5].interval_min, 4);
        let saved: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(saved["programs"][0].get("start").is_none());
        assert!(saved["programs"][0].get("interval_min").is_none());
        let _ = std::fs::remove_dir_all(&dir);
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
    fn picture_mode_follows_the_cascade() {
        let mut p = Project::default();
        p.tx.mode = Mode::MartinM1;
        p.programs.push(Program { id: 1, mode: Some(Mode::ScottieS1), ..Program::default() });
        p.blocks.push(Block { id: 2, program: Some(1), mode: Some(Mode::Robot12), ..Block::default() });
        assert_eq!(p.picture_mode(None, None, None), Mode::MartinM1);
        assert_eq!(p.picture_mode(None, None, Some(1)), Mode::ScottieS1);
        assert_eq!(p.picture_mode(None, Some(2), Some(1)), Mode::Robot12);
        assert_eq!(p.picture_mode(Some(Mode::Pd120), Some(2), Some(1)), Mode::Pd120);
        assert_eq!(p.still_mode(None, Some(1)), Mode::ScottieS1);
        assert_eq!(p.still_mode(Some(Mode::RobotBw8), Some(1)), Mode::RobotBw8);
    }

    #[test]
    fn partial_file_keeps_the_channel_defaults() {
        let p: Project = toml::from_str("format = 2\nname = \"x\"\n[tx]\nmode = \"PD120\"\n").unwrap();
        assert_eq!(p.tx.mode, Mode::Pd120);
        assert_eq!(p.tx.cw_wpm, 20.0);
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
        assert!(err.contains("is not converted"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

        let newer = "format = 3\nname = \"x\"\n";
        std::fs::write(&path, newer).unwrap();
        let err = Project::load(&path).unwrap_err().to_string();
        assert!(err.contains("newer version"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer);

        let disguised = "format = 2\n[[programs]]\nid = 1\npath = \"a.png\"\n[[blocks]]\nid = 2\nfreq_hz = 1\n";
        std::fs::write(&path, disguised).unwrap();
        let err = Project::load(&path).unwrap_err().to_string();
        assert!(err.contains("is not converted"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), disguised);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn slots_and_fsk_are_the_old_shape() {
        let err = Project::load_str("format = 2\n[grid]\nslots = []\n");
        assert!(err.contains("is not converted"), "{err}");
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
