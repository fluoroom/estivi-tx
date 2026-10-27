//! Dynamic text: `{datos}` placeholders and text fetched from an HTTP API.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};

use crate::graphics::ApiSource;
use crate::modes::Mode;

/// Everything a text layer can show, captured at render time.
#[derive(Clone, Debug)]
pub struct DataCtx {
    pub call: String,
    pub operator: String,
    pub qth: String,
    pub locator: String,
    pub now: DateTime<Utc>,
    pub serial: u64,
    pub mode: Mode,
    pub program: String,
    pub freq_hz: u64,
    pub cw: String,
    pub dtmf: String,
}

/// (placeholder, description) for the editor's "insert data" menu.
pub const PLACEHOLDERS: &[(&str, &str)] = &[
    ("{call}", "Callsign"),
    ("{op}", "Operator"),
    ("{qth}", "QTH"),
    ("{loc}", "Locator"),
    ("{utc}", "UTC time (HH:MM)"),
    ("{date}", "UTC date (YYYY-MM-DD)"),
    ("{local}", "Local time (HH:MM)"),
    ("{utc:%d/%m %H:%M}", "UTC with a custom format"),
    ("{serial}", "Transmission number"),
    ("{serial:4}", "Number with zeros (0042)"),
    ("{mode}", "SSTV mode"),
    ("{program}", "Program name"),
    ("{freq}", "Frequency (MHz)"),
    ("{cw}", "Last received CW text"),
    ("{cwcall}", "Last callsign heard in CW"),
    ("{dtmf}", "Last DTMF sequence"),
    ("{api}", "This layer's API response"),
];

fn strftime<Tz: chrono::TimeZone>(t: &DateTime<Tz>, fmt: &str) -> String
where
    Tz::Offset: std::fmt::Display,
{
    // chrono reports bad user formats as fmt::Error; never panic on them.
    let mut s = String::new();
    if write!(s, "{}", t.format(fmt)).is_err() {
        return format!("{{formato inválido: {fmt}}}");
    }
    s
}

/// Expands `{name}` / `{name:arg}` placeholders. Unknown ones are left untouched and
/// `{{` / `}}` produce literal braces.
pub fn expand(template: &str, ctx: &DataCtx, api: Option<&str>) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if let Some(after) = tail.strip_prefix('}') {
            out.push('}');
            rest = after;
            continue;
        }
        let Some(end) = tail.find('}') else {
            out.push_str(tail);
            return out;
        };
        let inner = &tail[1..end];
        let (name, arg) = match inner.split_once(':') {
            Some((n, a)) => (n.trim(), Some(a)),
            None => (inner.trim(), None),
        };
        match resolve(name, arg, ctx, api) {
            Some(v) => out.push_str(&v),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn resolve(name: &str, arg: Option<&str>, ctx: &DataCtx, api: Option<&str>) -> Option<String> {
    Some(match name.to_ascii_lowercase().as_str() {
        "call" => ctx.call.clone(),
        "op" => ctx.operator.clone(),
        "qth" => ctx.qth.clone(),
        "loc" => ctx.locator.clone(),
        "utc" => strftime(&ctx.now, arg.unwrap_or("%H:%M")),
        "date" => strftime(&ctx.now, arg.unwrap_or("%Y-%m-%d")),
        "local" => strftime(&ctx.now.with_timezone(&Local), arg.unwrap_or("%H:%M")),
        "serial" => match arg.and_then(|a| a.trim().parse::<usize>().ok()) {
            Some(width) => format!("{:0width$}", ctx.serial),
            None => ctx.serial.to_string(),
        },
        "mode" => ctx.mode.name().to_string(),
        "program" => ctx.program.clone(),
        "freq" if ctx.freq_hz == 0 => String::new(),
        "freq" => format!("{:.3} MHz", ctx.freq_hz as f64 / 1e6),
        "cw" => ctx.cw.clone(),
        "cwcall" => find_callsign(&ctx.cw).unwrap_or_default(),
        "dtmf" => ctx.dtmf.clone(),
        "api" => api.unwrap_or("").to_string(),
        _ => return None,
    })
}

/// Last token in `text` shaped like an amateur callsign (prefix, digit, 1–4 letters,
/// optional /suffix). Rejects reports like 599 or 5NN.
pub fn find_callsign(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == ',')
        .rev()
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '/').to_ascii_uppercase())
        .find(|t| is_callsign(t.split('/').max_by_key(|p| p.len()).unwrap_or("")))
}

fn is_callsign(s: &str) -> bool {
    let b = s.as_bytes();
    if !(3..=8).contains(&b.len()) || !b.iter().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    // The call-area digit is the last digit; prefix before it, letters after.
    let Some(d) = b.iter().rposition(|c| c.is_ascii_digit()) else { return false };
    let (prefix, suffix) = (&b[..d], &b[d + 1..]);
    (1..=3).contains(&prefix.len())
        && prefix.iter().any(|c| c.is_ascii_alphabetic())
        && (1..=4).contains(&suffix.len())
        && suffix.iter().all(|c| c.is_ascii_alphabetic())
}

const MAX_BODY: u64 = 256 * 1024;
const MAX_VALUE_CHARS: usize = 500;

/// Calls the API and returns the text to show.
pub fn fetch(src: &ApiSource) -> anyhow::Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .user_agent(concat!("estivi/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let body = agent
        .get(src.url.trim())
        .call()?
        .body_mut()
        .with_config()
        .limit(MAX_BODY)
        .read_to_string()?;
    let value = if src.json_path.trim().is_empty() { body.trim().to_string() } else { extract_json(&body, &src.json_path)? };
    Ok(value.chars().take(MAX_VALUE_CHARS).collect())
}

/// Follows a dotted path (`a.b.0.c`, `a.items[2].name`) into a JSON document.
pub fn extract_json(body: &str, path: &str) -> anyhow::Result<String> {
    let doc: serde_json::Value = serde_json::from_str(body)?;
    let mut v = &doc;
    let path = path.replace('[', ".").replace(']', "");
    for part in path.split('.').map(str::trim).filter(|p| !p.is_empty()) {
        v = match v {
            serde_json::Value::Array(a) => part.parse::<usize>().ok().and_then(|i| a.get(i)),
            serde_json::Value::Object(o) => o.get(part),
            _ => None,
        }
        .ok_or_else(|| anyhow::anyhow!("no existe «{part}» en la respuesta"))?;
    }
    Ok(match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    })
}

#[derive(Clone, Debug, Default)]
pub struct ApiState {
    pub value: Option<String>,
    pub error: Option<String>,
    pub fetched: Option<Instant>,
}

/// (url, json_path) → (latest state, fetch in flight).
type ApiMap = HashMap<(String, String), (ApiState, bool)>;

/// Latest response per (url, json_path), refreshed in the background.
#[derive(Clone, Default)]
pub struct ApiCache {
    inner: Arc<Mutex<ApiMap>>,
}

impl ApiCache {
    fn key(src: &ApiSource) -> (String, String) {
        (src.url.trim().to_string(), src.json_path.trim().to_string())
    }

    pub fn state(&self, src: &ApiSource) -> ApiState {
        self.inner.lock().unwrap().get(&Self::key(src)).map(|e| e.0.clone()).unwrap_or_default()
    }

    pub fn value(&self, src: &ApiSource) -> Option<String> {
        self.state(src).value
    }

    /// Fetches now, on this thread.
    pub fn fetch_now(&self, src: &ApiSource) -> Result<String, String> {
        let result = fetch(src).map_err(|e| e.to_string());
        let mut map = self.inner.lock().unwrap();
        let entry = map.entry(Self::key(src)).or_default();
        entry.0.fetched = Some(Instant::now());
        match &result {
            Ok(v) => {
                entry.0.value = Some(v.clone());
                entry.0.error = None;
            }
            Err(e) => entry.0.error = Some(e.clone()),
        }
        entry.1 = false;
        result
    }

    /// Starts a background fetch for every source whose value is older than its
    /// refresh period. Never blocks.
    pub fn refresh_due<'a>(&self, sources: impl IntoIterator<Item = &'a ApiSource>) {
        for src in sources {
            let due = {
                let mut map = self.inner.lock().unwrap();
                let entry = map.entry(Self::key(src)).or_default();
                let period = Duration::from_secs(src.refresh_s.max(10) as u64);
                let stale = entry.0.fetched.is_none_or(|t| t.elapsed() >= period);
                if stale && !entry.1 {
                    entry.1 = true;
                    true
                } else {
                    false
                }
            };
            if due {
                let (cache, src) = (self.clone(), src.clone());
                std::thread::spawn(move || {
                    if let Err(e) = cache.fetch_now(&src) {
                        log::warn!("API {}: {e}", src.url);
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> DataCtx {
        DataCtx {
            call: "LU1ABC".into(),
            operator: "Ana".into(),
            qth: "Córdoba".into(),
            locator: "FF78".into(),
            now: DateTime::parse_from_rfc3339("2026-10-23T21:05:09Z").unwrap().with_timezone(&Utc),
            serial: 42,
            mode: Mode::MartinM1,
            program: "Logo".into(),
            freq_hz: 14_230_000,
            cw: "CQ DE LU2XYZ 5NN".into(),
            dtmf: "123".into(),
        }
    }

    #[test]
    fn placeholders() {
        let c = ctx();
        assert_eq!(expand("{call} {utc}Z #{serial:4}", &c, None), "LU1ABC 21:05Z #0042");
        assert_eq!(expand("{freq} {mode}", &c, None), "14.230 MHz Martin M1");
        assert_eq!(expand("{{literal}} {nope} {utc:%d/%m}", &c, None), "{literal} {nope} 23/10");
        assert_eq!(expand("T: {api}°", &c, Some("21.5")), "T: 21.5°");
        assert_eq!(expand("unterminated {call", &c, None), "unterminated {call");
        assert!(expand("{utc:%Q}", &c, None).contains("inválido"));
    }

    #[test]
    fn callsigns() {
        assert_eq!(find_callsign("CQ DE LU2XYZ 5NN"), Some("LU2XYZ".into()));
        assert_eq!(find_callsign("W1AW/P K"), Some("W1AW/P".into()));
        assert_eq!(find_callsign("599 TU"), None);
        assert_eq!(find_callsign("4X1AB"), Some("4X1AB".into()));
    }

    #[test]
    fn json_paths() {
        let body = r#"{"current":{"temperature_2m":21.5,"w":"sol"},"items":[{"name":"a"},{"name":"b"}]}"#;
        assert_eq!(extract_json(body, "current.temperature_2m").unwrap(), "21.5");
        assert_eq!(extract_json(body, "items[1].name").unwrap(), "b");
        assert!(extract_json(body, "current.nope").is_err());
    }
}
