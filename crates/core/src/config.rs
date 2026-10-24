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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PttConfig {
    pub method: PttMethod,
    pub port: String,
    pub line: SerialLine,
    /// Active-low interfaces.
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
    /// Light chrome instead of the default dark one.
    pub light: bool,
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
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(anyhow::anyhow!("{}: {e}", path.display())),
        }
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
        let s = toml::to_string_pretty(&c).unwrap();
        assert_eq!(toml::from_str::<Config>(&s).unwrap(), c);
    }
}
