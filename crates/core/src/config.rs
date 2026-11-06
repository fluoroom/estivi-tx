//! Station hardware configuration (audio, serial/PTT, CAT). Belongs to the machine, not
//! to the channel: `estivi.toml` next to the executable (portable), or the user config
//! folder when that is not writable.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub audio: AudioConfig,
    pub ptt: PttConfig,
    pub cat: CatConfig,
    pub ui: UiConfig,
    pub time: TimeSync,
}

/// Offset against an NTP server. The operating-system clock is not changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimeSync {
    /// The first-run offer has already been answered.
    pub prompted: bool,
    pub enabled: bool,
    pub server: String,
    pub period_h: u32,
    pub offset_ms: i64,
    pub last_sync: String,
    pub last_error: String,
}

impl Default for TimeSync {
    fn default() -> Self {
        TimeSync {
            prompted: false,
            enabled: false,
            server: "pool.ntp.org".into(),
            period_h: 6,
            offset_ms: 0,
            last_sync: String::new(),
            last_error: String::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutChannel {
    #[default]
    Both,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Device id as listed by the app (or a device name); empty = system default.
    pub output: String,
    pub input: String,
    /// 0 = device default.
    pub sample_rate: u32,
    pub channel: OutChannel,
    /// Output level 0..1.
    pub level: f32,
    /// Sound card clock error in ppm (positive = card runs fast). Corrects slant.
    pub tx_ppm: f64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        AudioConfig { output: String::new(), input: String::new(), sample_rate: 0, channel: OutChannel::Both, level: 0.5, tx_ppm: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PttMethod {
    /// No keying (receive-only tests, or the radio is keyed by other means).
    None,
    /// The radio keys itself on audio.
    #[default]
    Vox,
    /// RTS and/or DTR of a serial port (Digirig, homemade interfaces…).
    Serial,
    /// Hamlib `rigctld` over TCP.
    Cat,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SerialLine {
    #[default]
    Rts,
    Dtr,
    Both,
}

/// How one serial handshake line keys the radio. `Off` leaves it released.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineLevel {
    #[default]
    Off,
    Positive,
    Negative,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PttConfig {
    pub method: PttMethod,
    pub port: String,
    /// RTS while transmitting. `Positive` is high on key, `Negative` is high at rest.
    pub rts: LineLevel,
    /// DTR, independent of RTS.
    pub dtr: LineLevel,
    /// Old files named one line. Not written anymore.
    #[serde(default, skip_serializing)]
    pub line: SerialLine,
    /// Old files used one polarity for every line. Not written anymore.
    #[serde(default, skip_serializing)]
    pub invert: bool,
    /// Wait after keying before audio starts.
    pub lead_ms: u32,
    /// Keep keyed after the audio ends.
    pub tail_ms: u32,
    /// Safety: never stay keyed longer than this.
    pub timeout_s: u32,
}

impl Default for PttConfig {
    fn default() -> Self {
        PttConfig {
            method: PttMethod::Vox,
            port: String::new(),
            rts: LineLevel::Positive,
            dtr: LineLevel::Off,
            line: SerialLine::Rts,
            invert: false,
            lead_ms: 300,
            tail_ms: 200,
            timeout_s: 600,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CatConfig {
    pub host: String,
    pub port: u16,
    /// Set the grid block frequency before transmitting.
    pub set_freq: bool,
}

impl Default for CatConfig {
    fn default() -> Self {
        CatConfig { host: "127.0.0.1".into(), port: 4532, set_freq: false }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    English,
    Spanish,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::English => "en",
            Lang::Spanish => "es",
        }
    }

    /// `None` if empty or not English/Spanish (caller should default to English).
    pub fn parse(s: &str) -> Option<Self> {
        let primary = s.trim().split(['-', '_', '.', '@', ':']).next().unwrap_or("").to_ascii_lowercase();
        match primary.as_str() {
            "en" | "eng" | "english" => Some(Lang::English),
            "es" | "spa" | "spanish" | "español" | "espanol" => Some(Lang::Spanish),
            _ => None,
        }
    }

    /// System locale: Spanish if the OS is es, otherwise English.
    pub fn from_system() -> Self {
        for key in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
            if let Ok(v) = std::env::var(key) {
                for part in v.split(':') {
                    if let Some(lang) = Self::parse(part) {
                        return lang;
                    }
                    let primary = part.split(['-', '_', '.']).next().unwrap_or("").to_ascii_lowercase();
                    if primary.is_empty() || primary == "c" || primary == "posix" {
                        continue;
                    }
                    return Lang::English;
                }
            }
        }
        #[cfg(windows)]
        {
            return windows_ui_lang();
        }
        #[cfg(not(windows))]
        Lang::English
    }

    /// Saved `"en"`/`"es"`, or system language when the setting has never been written.
    pub fn resolve(saved: &str) -> Self {
        Self::parse(saved).unwrap_or_else(Self::from_system)
    }

    pub fn pick(self, en: &'static str, es: &'static str) -> &'static str {
        match self {
            Lang::English => en,
            Lang::Spanish => es,
        }
    }

    pub fn t(self, en: &str, es: &str) -> String {
        match self {
            Lang::English => en.to_string(),
            Lang::Spanish => es.to_string(),
        }
    }
}

#[cfg(windows)]
fn windows_ui_lang() -> Lang {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    let primary = unsafe { GetUserDefaultUILanguage() } & 0x3FF;
    match primary {
        0x0A => Lang::Spanish,
        _ => Lang::English,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub last_project: String,
    /// Draw the UI with OpenGL instead of the CPU renderer. The CPU renderer is the
    /// default: the UI changes about once a second, and it needs ~50 MB of RAM against
    /// ~170 MB with GL, without depending on GPU drivers.
    pub gpu_render: bool,
    /// Start with the grid on air (unattended stations, after a power cut).
    pub autostart: bool,
    /// Light chrome instead of the default dark one. Kept in step with `theme`.
    pub light: bool,
    /// `dark` (older files say `gray`), `light`, `dracula`, `lavender`, `pastel`, `subte`, `subte-light`. Empty follows `light`.
    pub theme: String,
    /// `"en"` / `"es"`. Empty = not chosen yet; first boot picks the system language.
    pub lang: String,
}

pub const FILE_NAME: &str = "estivi.toml";

impl Config {
    /// Where the config lives: next to the executable when possible (portable install),
    /// otherwise the per-user config folder.
    pub fn location() -> PathBuf {
        if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
            let candidate = dir.join(FILE_NAME);
            if candidate.exists() || dir_writable(&dir) {
                return candidate;
            }
        }
        user_config_dir().join("estivi").join(FILE_NAME)
    }

    pub fn load(path: &Path) -> anyhow::Result<Config> {
        Ok(match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(anyhow::anyhow!("{}: {e}", path.display())),
        })
    }

    fn parse(text: &str) -> Result<Config, toml::de::Error> {
        let mut cfg: Config = toml::from_str(text)?;
        let raw: toml::Value = toml::from_str(text)?;
        if let Some(ptt) = raw.get("ptt").and_then(toml::Value::as_table) {
            let legacy = !ptt.contains_key("rts") && !ptt.contains_key("dtr") && (ptt.contains_key("line") || ptt.contains_key("invert"));
            if legacy {
                let level = if cfg.ptt.invert { LineLevel::Negative } else { LineLevel::Positive };
                match cfg.ptt.line {
                    SerialLine::Rts => { cfg.ptt.rts = level; cfg.ptt.dtr = LineLevel::Off; }
                    SerialLine::Dtr => { cfg.ptt.rts = LineLevel::Off; cfg.ptt.dtr = level; }
                    SerialLine::Both => { cfg.ptt.rts = level; cfg.ptt.dtr = level; }
                }
            }
        }
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::project::write_atomic(path, &toml::to_string_pretty(self)?)
    }
}

fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".estivi-probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn user_config_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut c = Config::default();
        c.ptt.method = PttMethod::Serial;
        c.ptt.port = "/dev/ttyUSB0".into();
        c.ui.lang = "es".into();
        let s = toml::to_string_pretty(&c).unwrap();
        assert_eq!(Config::parse(&s).unwrap(), c);
        let old = "method = \"serial\"\nport = \"/dev/ttyUSB0\"\nline = \"dtr\"\ninvert = true\n";
        let migrated = Config::parse(&format!("[ptt]\n{old}")).unwrap();
        assert_eq!(migrated.ptt.rts, LineLevel::Off);
        assert_eq!(migrated.ptt.dtr, LineLevel::Negative);
    }

    #[test]
    fn lang_parse() {
        assert_eq!(Lang::parse("es_AR.UTF-8"), Some(Lang::Spanish));
        assert_eq!(Lang::parse("en_US"), Some(Lang::English));
        assert_eq!(Lang::parse("fr_FR"), None);
        assert_eq!(Lang::parse(""), None);
        assert_eq!(Lang::resolve("es"), Lang::Spanish);
        assert_eq!(Lang::parse("pt_BR"), None);
    }

}
