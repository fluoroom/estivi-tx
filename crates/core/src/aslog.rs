//! As-run log: one CSV line per transmission, next to the project file.

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

const HEADER: &str = "inicio_utc,fin_utc,serie,modo,programa,frecuencia_hz,origen,resultado";

#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub serial: u64,
    pub mode: String,
    pub program: String,
    pub freq_hz: u64,
    pub origin: String,
    pub result: String,
}

/// `canal.estivi` → `canal-registro.csv`.
pub fn path_for(project: &Path) -> PathBuf {
    let stem = project.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "estivi".into());
    project.with_file_name(format!("{stem}-registro.csv"))
}

fn field(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

pub fn append(path: &Path, e: &LogEntry) -> anyhow::Result<()> {
    let new = !path.exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if new {
        writeln!(f, "{HEADER}")?;
    }
    writeln!(
        f,
        "{},{},{},{},{},{},{},{}",
        e.start.format("%Y-%m-%dT%H:%M:%SZ"),
        e.end.format("%Y-%m-%dT%H:%M:%SZ"),
        e.serial,
        field(&e.mode),
        field(&e.program),
        e.freq_hz,
        field(&e.origin),
        field(&e.result)
    )?;
    Ok(())
}

fn split_csv(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                chars.next();
                out.last_mut().unwrap().push('"');
            }
            ('"', _) => quoted = !quoted,
            (',', false) => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out
}

fn parse(line: &str) -> Option<LogEntry> {
    let f = split_csv(line);
    let time = |s: &str| DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&Utc));
    Some(LogEntry {
        start: time(f.first()?)?,
        end: time(f.get(1)?)?,
        serial: f.get(2)?.parse().ok()?,
        mode: f.get(3)?.clone(),
        program: f.get(4)?.clone(),
        freq_hz: f.get(5)?.parse().unwrap_or(0),
        origin: f.get(6).cloned().unwrap_or_default(),
        result: f.get(7).cloned().unwrap_or_default(),
    })
}

/// The last `n` entries, oldest first.
pub fn tail(path: &Path, n: usize) -> Vec<LogEntry> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut entries: Vec<LogEntry> = text.lines().skip(1).filter_map(parse).collect();
    let skip = entries.len().saturating_sub(n);
    entries.drain(..skip);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_read_back() {
        let dir = std::env::temp_dir().join(format!("estivi-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = path_for(&dir.join("canal.estivi"));
        let _ = std::fs::remove_file(&path);
        let t = DateTime::parse_from_rfc3339("2026-10-23T21:00:00Z").unwrap().with_timezone(&Utc);
        let e = LogEntry {
            start: t,
            end: t,
            serial: 7,
            mode: "Martin M1".into(),
            program: "Logo, \"club\"".into(),
            freq_hz: 14_230_000,
            origin: "grilla".into(),
            result: "OK".into(),
        };
        append(&path, &e).unwrap();
        append(&path, &LogEntry { serial: 8, ..e.clone() }).unwrap();
        let back = tail(&path, 10);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0], e);
        assert_eq!(tail(&path, 1)[0].serial, 8);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
