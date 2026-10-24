//! The channel project: what is transmitted, when and how it looks.
//! Saved by the user wherever they want (`*.estivi`, TOML). Machine-specific settings
//! (audio devices, serial port, PTT) live in `config` instead.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::encoder::{ColorRange, CwIdent, TxOptions};
use crate::graphics::Graphics;
use crate::modes::Mode;

pub const EXTENSION: &str = "estivi";
const FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub format: u32,
    pub name: String,
    pub station: Station,
    pub tx: TxSettings,
    pub rotation: Rotation,
    pub programs: Vec<Program>,
    pub grid: Grid,
    pub graphics: Graphics,
    pub listen: Listen,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            format: FORMAT,
            name: "Canal SSTV".into(),
            station: Station::default(),
            tx: TxSettings::default(),
            rotation: Rotation::Sequential,
            programs: Vec::new(),
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
    pub fsk_id: bool,
    /// CW before the VIS (in addition to the identification after the picture).
    pub cw_preamble: bool,
    pub cw_id: bool,
    /// Accepts `{datos}`, e.g. "DE {call}".
    pub cw_text: String,
    pub cw_wpm: f32,
    pub cw_tone_hz: f32,
    /// Minimum rest between the end of one transmission and the next (duty cycle).
    pub min_pause_s: u32,
}

impl Default for TxSettings {
    fn default() -> Self {
        TxSettings {
            mode: Mode::MartinM1,
            color_range: ColorRange::Full,
            vox_preamble: false,
            fsk_id: false,
            cw_preamble: false,
            cw_id: true,
            cw_text: "DE {call}".into(),
            cw_wpm: 20.0,
            cw_tone_hz: 800.0,
            min_pause_s: 60,
        }
    }
}

impl TxSettings {
    /// Encoder options; `expand` resolves `{datos}` in the identification texts.
    pub fn options(&self, call: &str, expand: impl Fn(&str) -> String) -> TxOptions {
        TxOptions {
            vox_preamble: self.vox_preamble,
            color_range: self.color_range,
            fsk_id: self.fsk_id.then(|| call.to_string()),
            cw_preamble: self.cw_preamble.then(|| CwIdent { text: expand(&self.cw_text), wpm: self.cw_wpm, tone_hz: self.cw_tone_hz }),
            cw_id: self.cw_id.then(|| CwIdent { text: expand(&self.cw_text), wpm: self.cw_wpm, tone_hz: self.cw_tone_hz }),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rotation {
    /// In list order.
    #[default]
    Sequential,
    /// Random, but every program airs once before any repeats.
    Shuffle,
}

/// One picture inside a program.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProgramImage {
    /// Relative to the project file when possible.
    pub path: String,
    pub enabled: bool,
    /// Override the program (or project) SSTV mode. None = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
}

impl Default for ProgramImage {
    fn default() -> Self {
        ProgramImage { path: String::new(), enabled: true, mode: None }
    }
}

impl ProgramImage {
    pub fn new(path: impl Into<String>) -> ProgramImage {
        ProgramImage { path: path.into(), enabled: true, mode: None }
    }
}

/// A program: a group of pictures that go on air under the channel graphics,
/// with their own order, CW and activations. Schedule lives on the channel grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Program {
    pub id: u64,
    pub name: String,
    pub enabled: bool,
    pub images: Vec<ProgramImage>,
    /// Order of pictures inside this program. The channel-level `Project.rotation`
    /// still picks which program airs when a block asks for the rotation.
    pub rotation: Rotation,
    /// Default SSTV mode for this program's pictures. None = the project mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// Legacy per-program windows. Migrated into `grid.slots` on load.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<Slot>,
    /// Override the project CW for this program. None = inherit.
    pub cw_preamble: Option<bool>,
    pub cw_id: Option<bool>,
    /// Empty = inherit the project text.
    pub cw_text: String,
    /// Triggers that transmit this program (or pause / resume the grid).
    pub activations: Vec<Trigger>,
    /// Legacy single-image path from format 1. Migrated into `images` on load.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
}

impl Default for Program {
    fn default() -> Self {
        Program {
            id: 0,
            name: String::new(),
            enabled: true,
            images: Vec::new(),
            rotation: Rotation::Sequential,
            mode: None,
            slots: Vec::new(),
            cw_preamble: None,
            cw_id: None,
            cw_text: String::new(),
            activations: Vec::new(),
            path: String::new(),
        }
    }
}

impl Program {
    pub fn enabled_images(&self) -> impl Iterator<Item = &ProgramImage> {
        self.images.iter().filter(|i| i.enabled && !i.path.trim().is_empty())
    }

    pub fn first_path(&self) -> Option<&str> {
        self.enabled_images().next().map(|i| i.path.as_str())
    }

    pub fn image_count(&self) -> usize {
        self.enabled_images().count()
    }

    /// Encoder options for this program: its own CW flags/text when set, else the project's.
    pub fn tx_options(&self, project: &Project, expand: impl Fn(&str) -> String) -> TxOptions {
        let tx = &project.tx;
        let text = if self.cw_text.trim().is_empty() { tx.cw_text.as_str() } else { self.cw_text.as_str() };
        let preamble = self.cw_preamble.unwrap_or(tx.cw_preamble);
        let post = self.cw_id.unwrap_or(tx.cw_id);
        TxOptions {
            vox_preamble: tx.vox_preamble,
            color_range: tx.color_range,
            fsk_id: tx.fsk_id.then(|| project.station.call.clone()),
            cw_preamble: preamble.then(|| CwIdent { text: expand(text), wpm: tx.cw_wpm, tone_hz: tx.cw_tone_hz }),
            cw_id: post.then(|| CwIdent { text: expand(text), wpm: tx.cw_wpm, tone_hz: tx.cw_tone_hz }),
        }
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
    /// Seconds added to every grid start. Blocks stay written at 14:00; this
    /// channel actually keys at 14:00 + offset, so several stations can share a
    /// frequency (the other one occupies 14:00–14:03, this one starts at 14:03).
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub offset_s: u32,
    pub slots: Vec<Slot>,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// A block of the programming grid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Slot {
    pub id: u64,
    pub enabled: bool,
    pub name: String,
    /// Monday first.
    pub days: [bool; 7],
    /// "HH:MM"
    pub start: String,
    /// "HH:MM"; empty = a single transmission at `start`. Earlier than `start` = ends
    /// the next day.
    pub end: String,
    /// Repeat period inside [start, end), in minutes.
    pub every_min: u32,
    /// None = next program of the rotation.
    pub program: Option<u64>,
    /// Frequency to set via CAT before transmitting (0 = leave the radio alone).
    pub freq_hz: u64,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            id: 0,
            enabled: true,
            name: "Bloque".into(),
            days: [true; 7],
            start: "21:00".into(),
            end: "23:00".into(),
            every_min: 5,
            program: None,
            freq_hz: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Listen {
    pub cw: bool,
    pub cw_wpm: f32,
    pub cw_tone_hz: f32,
    pub dtmf: bool,
    pub triggers: Vec<Trigger>,
}

impl Default for Listen {
    fn default() -> Self {
        Listen { cw: false, cw_wpm: 20.0, cw_tone_hz: 700.0, dtmf: false, triggers: Vec::new() }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerSource {
    #[default]
    Dtmf,
    Cw,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerAction {
    /// Transmit now (`program`, or the next one of the rotation).
    #[default]
    Transmit,
    Pause,
    Resume,
}

/// "When this DTMF sequence / CW word is heard, do this."
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trigger {
    pub enabled: bool,
    pub source: TriggerSource,
    pub code: String,
    pub action: TriggerAction,
    pub program: Option<u64>,
}

impl Project {
    pub fn load(path: &Path) -> anyhow::Result<Project> {
        let text = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let mut p: Project = toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        anyhow::ensure!(p.format <= FORMAT, "el proyecto es de una versión más nueva de Estivi");
        p.migrate();
        Ok(p)
    }

    /// Turns the old "one path per program" files into image groups, and moves
    /// per-program schedule windows onto the channel grid.
    pub fn migrate(&mut self) {
        for p in &mut self.programs {
            if p.images.is_empty() && !p.path.trim().is_empty() {
                p.images.push(ProgramImage::new(std::mem::take(&mut p.path)));
            }
            p.path.clear();
        }
        let mut used: std::collections::HashSet<u64> = self.grid.slots.iter().map(|s| s.id).collect();
        let mut extra = Vec::new();
        for p in &mut self.programs {
            for mut s in std::mem::take(&mut p.slots) {
                if s.id == 0 || used.contains(&s.id) {
                    s.id = used.iter().max().copied().unwrap_or(0) + 1;
                }
                used.insert(s.id);
                s.program = Some(p.id);
                if s.name.trim().is_empty() || s.name == "Bloque" {
                    s.name = p.name.clone();
                }
                extra.push(s);
            }
        }
        self.grid.slots.extend(extra);
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        write_atomic(path, &toml::to_string_pretty(self)?)
    }

    pub fn next_program_id(&self) -> u64 {
        self.programs.iter().map(|p| p.id).max().unwrap_or(0) + 1
    }

    pub fn next_slot_id(&self) -> u64 {
        self.grid
            .slots
            .iter()
            .chain(self.programs.iter().flat_map(|p| p.slots.iter()))
            .map(|s| s.id)
            .max()
            .unwrap_or(0)
            + 1
    }

    pub fn program(&self, id: u64) -> Option<&Program> {
        self.programs.iter().find(|p| p.id == id)
    }

    /// Image → program → project.
    pub fn mode_of(&self, program: Option<&Program>, image: Option<&ProgramImage>) -> Mode {
        image.and_then(|i| i.mode).or_else(|| program.and_then(|p| p.mode)).unwrap_or(self.tx.mode)
    }
}

/// Writes through a temporary file so a crash never leaves a half-written file.
pub fn write_atomic(path: &Path, contents: &str) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp~");
    std::fs::write(&tmp, contents).map_err(|e| anyhow::anyhow!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok(())
}

/// Absolute path for a path stored in the project.
pub fn resolve(base_dir: Option<&Path>, stored: &str) -> PathBuf {
    let p = PathBuf::from(stored);
    match base_dir {
        Some(base) if p.is_relative() => base.join(p),
        _ => p,
    }
}

/// What to store for `abs`: relative to the project folder when it lives inside it
/// (so the folder can travel on a USB stick), absolute otherwise.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_toml() {
        let mut p = Project::default();
        p.station.call = "LU1ABC".into();
        p.programs.push(Program { id: 1, name: "Logo".into(), images: vec![ProgramImage::new("img/logo.png")], ..Default::default() });
        p.grid.slots.push(Slot { id: 1, program: Some(1), ..Default::default() });
        p.listen.triggers.push(Trigger { enabled: true, code: "*1#".into(), ..Default::default() });
        let s = toml::to_string_pretty(&p).unwrap();
        assert_eq!(toml::from_str::<Project>(&s).unwrap(), p);
        assert!(s.contains("mode = \"M1\""), "{s}");
    }

    #[test]
    fn partial_files_get_defaults() {
        let p: Project = toml::from_str("name = \"x\"\n[tx]\nmode = \"PD120\"\n").unwrap();
        assert_eq!(p.tx.mode, Mode::Pd120);
        assert_eq!(p.tx.cw_wpm, 20.0);
    }

    #[test]
    fn paths() {
        let base = Path::new("/media/usb/canal");
        assert_eq!(relativize(Some(base), Path::new("/media/usb/canal/img/a.png")), "img/a.png");
        assert_eq!(relativize(Some(base), Path::new("/home/x/b.png")), "/home/x/b.png");
        assert_eq!(resolve(Some(base), "img/a.png"), base.join("img/a.png"));
    }

    #[test]
    fn hhmm() {
        assert_eq!(parse_hhmm("21:05"), Some(21 * 60 + 5));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("x"), None);
    }

    #[test]
    fn old_single_path_becomes_an_image_group() {
        let mut p: Project = toml::from_str(
            r#"
[[programs]]
id = 1
name = "Logo"
path = "img/logo.png"
enabled = true
"#,
        )
        .unwrap();
        p.migrate();
        assert_eq!(p.programs[0].images, vec![ProgramImage::new("img/logo.png")]);
        assert!(p.programs[0].path.is_empty());
    }

    #[test]
    fn old_program_slots_move_to_the_grid() {
        let mut p: Project = toml::from_str(
            r#"
[[programs]]
id = 1
name = "Logo"
[[programs.slots]]
id = 1
start = "12:00"
[[grid.slots]]
id = 1
start = "21:00"
"#,
        )
        .unwrap();
        p.migrate();
        assert!(p.programs[0].slots.is_empty());
        assert_eq!(p.grid.slots.len(), 2);
        assert_eq!(p.grid.slots[1].program, Some(1));
        assert_eq!(p.grid.slots[1].name, "Logo");
        assert_ne!(p.grid.slots[0].id, p.grid.slots[1].id);
    }

    #[test]
    fn grid_offset_roundtrips() {
        let mut p = Project::default();
        p.grid.offset_s = 195;
        let s = toml::to_string_pretty(&p).unwrap();
        assert!(s.contains("offset_s = 195"), "{s}");
        assert_eq!(toml::from_str::<Project>(&s).unwrap().grid.offset_s, 195);
        let empty: Project = toml::from_str("name = \"x\"\n").unwrap();
        assert_eq!(empty.grid.offset_s, 0);
    }

    #[test]
    fn sstv_mode_falls_back_image_then_program_then_project() {
        let mut p = Project::default();
        p.tx.mode = Mode::MartinM1;
        let prog = Program { id: 1, mode: Some(Mode::Robot36), ..Default::default() };
        let img = ProgramImage { path: "a.png".into(), mode: Some(Mode::Robot72), ..Default::default() };
        assert_eq!(p.mode_of(None, None), Mode::MartinM1);
        assert_eq!(p.mode_of(Some(&prog), None), Mode::Robot36);
        assert_eq!(p.mode_of(Some(&prog), Some(&img)), Mode::Robot72);
        let inherit = ProgramImage { path: "b.png".into(), ..Default::default() };
        assert_eq!(p.mode_of(Some(&prog), Some(&inherit)), Mode::Robot36);
    }
}
