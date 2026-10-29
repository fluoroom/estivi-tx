//! Transmitter keying: VOX (nothing to do), serial RTS/DTR, or Hamlib rigctld.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{Context, anyhow, bail};

use crate::config::{CatConfig, Config, LineLevel, PttMethod};

pub fn serial_ports() -> Vec<String> {
    let mut ports: Vec<String> = serialport::available_ports().map(|v| v.into_iter().map(|p| p.port_name).collect()).unwrap_or_default();
    ports.sort();
    ports
}

pub enum Ptt {
    None,
    Serial { port: Box<dyn serialport::SerialPort>, rts: LineLevel, dtr: LineLevel, keyed: bool },
    Cat { cat: CatConfig, keyed: bool },
}

impl Ptt {
    /// Opens the keying device. Serial ports are held open for the whole session:
    /// opening a port asserts RTS/DTR on many systems, which would key the radio.
    pub fn open(cfg: &Config) -> anyhow::Result<Ptt> {
        Ok(match cfg.ptt.method {
            PttMethod::None | PttMethod::Vox => Ptt::None,
            PttMethod::Serial => {
                if cfg.ptt.port.trim().is_empty() {
                    bail!("serial PTT: no port selected");
                }
                let port = serialport::new(cfg.ptt.port.trim(), 9600)
                    .timeout(Duration::from_millis(200))
                    .open()
                    .with_context(|| format!("could not open {}", cfg.ptt.port))?;
                let mut ptt = Ptt::Serial { port, rts: cfg.ptt.rts, dtr: cfg.ptt.dtr, keyed: true };
                ptt.key(false)?;
                ptt
            }
            PttMethod::Cat => Ptt::Cat { cat: cfg.cat.clone(), keyed: false },
        })
    }

    pub fn key(&mut self, on: bool) -> anyhow::Result<()> {
        match self {
            Ptt::None => Ok(()),
            Ptt::Serial { port, rts, dtr, keyed } => {
                port.write_request_to_send(line_high(*rts, on))?;
                port.write_data_terminal_ready(line_high(*dtr, on))?;
                *keyed = on;
                Ok(())
            }
            Ptt::Cat { cat, keyed } => {
                rigctld(cat, if on { "T 1" } else { "T 0" })?;
                *keyed = on;
                Ok(())
            }
        }
    }

    pub fn is_keyed(&self) -> bool {
        match self {
            Ptt::None => false,
            Ptt::Serial { keyed, .. } | Ptt::Cat { keyed, .. } => *keyed,
        }
    }
}

impl Drop for Ptt {
    fn drop(&mut self) {
        if self.is_keyed() {
            if let Err(e) = self.key(false) {
                log::error!("could not release PTT: {e}");
            }
        }
    }
}

/// Sends one command to rigctld and checks its `RPRT` answer.
pub fn rigctld(cat: &CatConfig, command: &str) -> anyhow::Result<()> {
    let addr = (cat.host.trim(), cat.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("rigctld: invalid address {}:{}", cat.host, cat.port))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).with_context(|| format!("rigctld at {addr} did not answer"))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(format!("{command}\n").as_bytes())?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    match line.trim().strip_prefix("RPRT ") {
        Some("0") => Ok(()),
        Some(code) => bail!("rigctld rejected \"{command}\" (RPRT {code})"),
        None => bail!("rigctld answered something unexpected: {}", line.trim()),
    }
}

pub fn set_frequency(cat: &CatConfig, hz: u64) -> anyhow::Result<()> {
    rigctld(cat, &format!("F {hz}"))
}

/// `Positive` is high while keyed. `Negative` is high at rest. `Off` stays low.
fn line_high(level: LineLevel, on: bool) -> bool {
    match level {
        LineLevel::Off => false,
        LineLevel::Positive => on,
        LineLevel::Negative => !on,
    }
}
