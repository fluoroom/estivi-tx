//! A local command, run as the user who started Estivi. Stdout only, with a hard timeout.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn capture(line: &str, timeout: Duration) -> anyhow::Result<String> {
    let line = line.trim();
    anyhow::ensure!(!line.is_empty(), "comando vacío");
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", line]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", line]);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("no se pudo ejecutar: {e}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut stdout) = child.stdout.take() {
                    stdout.read_to_string(&mut out).ok();
                }
                anyhow::ensure!(status.success(), "el comando terminó con error");
                return Ok(out);
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("el comando superó los {} s", timeout.as_secs());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(e) => anyhow::bail!("{e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdout_only() {
        let out = capture("printf estivi", Duration::from_secs(3)).unwrap();
        assert_eq!(out, "estivi");
    }
}
