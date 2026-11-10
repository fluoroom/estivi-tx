//! Self-update from the public GitHub releases.
//!
//! The check is one request to the release API. Installing downloads the
//! archive for this system, verifies it against the `SHA256SUMS.txt` attached
//! to the same release, and replaces the executable (or the whole `Estivi.app`
//! on macOS) after the window has closed.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const REPO: &str = "fluoroom/estivi-tx";
pub const RELEASES_URL: &str = "https://github.com/fluoroom/estivi-tx/releases/latest";
/// Version of this build, from the workspace version in `Cargo.toml`.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

const LATEST_API: &str = "https://api.github.com/repos/fluoroom/estivi-tx/releases/latest";
const CHECKSUMS: &str = "SHA256SUMS.txt";
const MAX_JSON: u64 = 1024 * 1024;
const MAX_CHECKSUMS: u64 = 1024 * 1024;
const MAX_ASSET: u64 = 256 * 1024 * 1024;
const ASK_TIMEOUT: Duration = Duration::from_secs(15);
// A large archive on a slow link still has to finish. Progress is reported to
// the window, so the timeout is only a backstop against a dead connection.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub asset: Asset,
    pub checksums: Asset,
}

/// A verified archive unpacked in the temporary folder, ready to install.
#[derive(Clone, Debug)]
pub struct Staged {
    pub version: String,
    pub dir: PathBuf,
    pub binary: PathBuf,
    /// The `Estivi.app` bundle on macOS, when the archive has one.
    pub bundle: Option<PathBuf>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<ApiAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

impl From<ApiAsset> for Asset {
    fn from(a: ApiAsset) -> Self {
        Asset { name: a.name, url: a.browser_download_url, size: a.size }
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(concat!("estivi/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn http_error(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::StatusCode(403 | 429) => anyhow!("GitHub is limiting update checks; try again later"),
        ureq::Error::StatusCode(404) => anyhow!("there is no published Estivi release"),
        ureq::Error::StatusCode(code) => anyhow!("GitHub answered HTTP {code}"),
        other => anyhow!("could not reach GitHub: {other}"),
    }
}

/// Latest stable release, or `None` when this build is current or newer.
pub fn check() -> Result<Option<Release>> {
    check_at(LATEST_API)
}

fn check_at(url: &str) -> Result<Option<Release>> {
    let body = agent(ASK_TIMEOUT)
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(http_error)?
        .body_mut()
        .with_config()
        .limit(MAX_JSON)
        .read_to_string()
        .map_err(|e| anyhow!("could not read GitHub's answer: {e}"))?;
    parse_latest_for(&body, std::env::consts::OS, std::env::consts::ARCH)
}

fn parse_latest_for(body: &str, os: &str, arch: &str) -> Result<Option<Release>> {
    let raw: ApiRelease = serde_json::from_str(body).context("GitHub sent an unexpected release description")?;
    if raw.draft || raw.prerelease {
        return Ok(None);
    }
    let version = raw.tag_name.trim().trim_start_matches(['v', 'V']).to_string();
    let latest = semver::Version::parse(&version).with_context(|| format!("release tag {:?} is not a version", raw.tag_name))?;
    let current = semver::Version::parse(CURRENT).context("this build has no valid version")?;
    if latest <= current {
        return Ok(None);
    }
    let platform = platform_tag_for(os, arch).ok_or_else(|| anyhow!("there is no Estivi build for {os}/{arch}"))?;
    let wanted = format!("estivi-{version}-{platform}.tar.gz");
    let mut asset = None;
    let mut checksums = None;
    for a in raw.assets {
        if a.name == wanted {
            asset = Some(a);
        } else if a.name == CHECKSUMS {
            checksums = Some(a);
        }
    }
    let asset = asset.ok_or_else(|| anyhow!("release {version} has no {wanted}"))?;
    let checksums = checksums.ok_or_else(|| anyhow!("release {version} has no {CHECKSUMS} file"))?;
    let page = raw.html_url.filter(|u| !u.trim().is_empty()).unwrap_or_else(|| format!("https://github.com/{REPO}/releases/tag/{}", raw.tag_name));
    Ok(Some(Release { version, page, asset: asset.into(), checksums: checksums.into() }))
}

fn platform_tag_for(os: &str, arch: &str) -> Option<String> {
    let tag = match (os, arch) {
        ("linux", "x86_64") => "linux-x86_64",
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", "x86") => "windows-i686",
        ("macos", "x86_64" | "aarch64") => "macos-universal",
        _ => return None,
    };
    Some(tag.to_string())
}

/// Download, verify, and unpack `release` under the temporary folder.
pub fn stage(release: &Release, mut progress: impl FnMut(u64, u64)) -> Result<Staged> {
    let dir = staging_dir();
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let result = stage_in(&dir, release, &mut progress);
    if result.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    result
}

fn stage_in(dir: &Path, release: &Release, progress: &mut impl FnMut(u64, u64)) -> Result<Staged> {
    ensure!(
        !release.asset.name.contains(['/', '\\']) && !release.asset.name.contains(".."),
        "the release has an invalid file name"
    );
    let archive = dir.join(&release.asset.name);
    download(&release.asset, &archive, progress)?;
    verify_checksum(&release.asset, &release.checksums, &archive)?;
    let unpacked = dir.join("unpacked");
    extract_tar_gz(&archive, &unpacked)?;
    let _ = fs::remove_file(&archive);
    let (binary, bundle) = find_payload(&unpacked)?;
    Ok(Staged { version: release.version.clone(), dir: dir.to_path_buf(), binary, bundle })
}

fn staging_dir() -> PathBuf {
    std::env::temp_dir().join(format!("estivi-update-{}", std::process::id()))
}

fn download(asset: &Asset, path: &Path, progress: &mut impl FnMut(u64, u64)) -> Result<()> {
    let mut response = agent(DOWNLOAD_TIMEOUT)
        .get(&asset.url)
        .header("Accept", "application/octet-stream")
        .call()
        .map_err(http_error)?;
    // A proxy may gzip an already compressed archive; then the header counts
    // the compressed transfer and the checksum, not this byte count, is the
    // thing that matters.
    let encoded = response
        .headers()
        .get("Content-Encoding")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.eq_ignore_ascii_case("identity"));
    let total = if encoded {
        0
    } else {
        response
            .headers()
            .get("Content-Length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(asset.size)
    };
    let mut reader = response.body_mut().as_reader();
    let mut file = BufWriter::new(File::create(path).with_context(|| format!("could not create {}", path.display()))?);
    let mut buf = [0u8; 64 * 1024];
    let mut done = 0u64;
    let mut reported = 0u64;
    let mut last = Instant::now();
    loop {
        let n = reader.read(&mut buf).map_err(|e| anyhow!("download failed: {e}"))?;
        if n == 0 {
            break;
        }
        done += n as u64;
        ensure!(done <= MAX_ASSET, "the update is larger than {MAX_ASSET} bytes");
        file.write_all(&buf[..n]).map_err(|e| anyhow!("could not write the update: {e}"))?;
        if last.elapsed() >= Duration::from_millis(250) {
            progress(done, total);
            reported = done;
            last = Instant::now();
        }
    }
    file.flush()?;
    if done != reported {
        progress(done, total);
    }
    ensure!(done > 0, "the update download is empty");
    if total > 0 {
        ensure!(done == total, "the download ended at {done} of {total} bytes");
    }
    Ok(())
}

fn verify_checksum(asset: &Asset, sums: &Asset, path: &Path) -> Result<()> {
    let text = agent(ASK_TIMEOUT)
        .get(&sums.url)
        .header("Accept", "text/plain")
        .call()
        .map_err(http_error)?
        .body_mut()
        .with_config()
        .limit(MAX_CHECKSUMS)
        .read_to_string()
        .map_err(|e| anyhow!("could not read {CHECKSUMS}: {e}"))?;
    let expected = parse_checksums(&text)
        .into_iter()
        .find(|(name, _)| name == &asset.name)
        .map(|(_, hash)| hash)
        .ok_or_else(|| anyhow!("{CHECKSUMS} does not list {}", asset.name))?;
    let actual = sha256_file(path)?;
    ensure!(
        actual.eq_ignore_ascii_case(&expected),
        "the download does not match {CHECKSUMS} (expected {expected}, got {actual})"
    );
    Ok(())
}

fn parse_checksums(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let Some(hash) = parts.next() else { continue };
        let Some(rest) = parts.next() else { continue };
        let hash = hash.trim();
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let name = rest.trim().trim_start_matches('*').trim_start_matches("./");
        if !name.is_empty() {
            out.push((name.to_string(), hash.to_ascii_lowercase()));
        }
    }
    out
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive).with_context(|| format!("could not open {}", archive.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    fs::create_dir_all(dest)?;
    tar.unpack(dest).with_context(|| format!("could not unpack {}", archive.display()))?;
    Ok(())
}

fn find_payload(unpacked: &Path) -> Result<(PathBuf, Option<PathBuf>)> {
    find_payload_for(unpacked, std::env::consts::OS)
}

fn find_payload_for(unpacked: &Path, os: &str) -> Result<(PathBuf, Option<PathBuf>)> {
    if os == "macos" {
        if let Some(bundle) = find_dir(unpacked, "Estivi.app") {
            let binary = bundle.join("Contents").join("MacOS").join("estivi");
            ensure!(binary.is_file(), "the macOS app has no executable");
            make_executable(&binary)?;
            return Ok((binary, Some(bundle)));
        }
    }
    let name = if os == "windows" { "estivi.exe" } else { "estivi" };
    let binary = find_file(unpacked, name).ok_or_else(|| anyhow!("the update archive has no {name}"))?;
    make_executable(&binary)?;
    Ok((binary, None))
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        let Ok(entries) = fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_file() && entry.file_name().to_str() == Some(name) {
                return Some(entry.path());
            }
            if kind.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    None
}

fn find_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(d) = pending.pop() {
        let Ok(entries) = fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if !kind.is_dir() {
                continue;
            }
            if entry.file_name().to_str() == Some(name) {
                return Some(entry.path());
            }
            pending.push(entry.path());
        }
    }
    None
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Whether the folder that holds this copy can be written by this user.
pub fn can_install() -> bool {
    install_dir().is_ok_and(|dir| dir_writable(&dir))
}

fn install_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("could not locate the Estivi executable")?;
    #[cfg(target_os = "macos")]
    if let Some(bundle) = bundle_root(&exe) {
        return bundle.parent().map(Path::to_path_buf).ok_or_else(|| anyhow!("the app bundle has no parent folder"));
    }
    exe.parent().map(Path::to_path_buf).ok_or_else(|| anyhow!("the executable has no parent folder"))
}

fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".estivi-update-{}", std::process::id()));
    let ok = fs::write(&probe, b"").is_ok();
    if ok {
        let _ = fs::remove_file(&probe);
    }
    ok
}

/// Replace this copy and schedule a restart. Returns once the helper process is
/// running; the caller should quit the event loop right after.
pub fn install(staged: &Staged) -> Result<()> {
    ensure!(can_install(), "the Estivi folder is not writable");
    let exe = std::env::current_exe().context("could not locate the Estivi executable")?;
    #[cfg(target_os = "macos")]
    if let (Some(new), Some(root)) = (&staged.bundle, bundle_root(&exe)) {
        return install_macos_bundle(&root, new, staged);
    }
    install_binary(&exe, &staged.binary, staged)
}

#[cfg(unix)]
fn install_binary(exe: &Path, new_binary: &Path, staged: &Staged) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let dir = exe.parent().ok_or_else(|| anyhow!("the executable has no parent folder"))?;
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or("estivi");
    let temp = dir.join(format!(".{name}.update-{}", std::process::id()));
    let script = staged.dir.join("restart.sh");
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| sh_quote(&a.to_string_lossy())).collect();
    let body = format!(
        "#!/bin/sh\n\
         n=0\n\
         while kill -0 {pid} 2>/dev/null && [ \"$n\" -lt 100 ]; do sleep 0.1; n=$((n+1)); done\n\
         nohup {exe} {args} </dev/null >/dev/null 2>&1 &\n\
         ( sleep 5; rm -rf {stage} ) &\n",
        pid = std::process::id(),
        exe = sh_quote(&exe.to_string_lossy()),
        args = args.join(" "),
        stage = sh_quote(&staged.dir.to_string_lossy()),
    );
    // Write the helper before touching the binary: if the staging folder is
    // gone, the running copy stays as it is.
    fs::write(&script, body).with_context(|| format!("could not write {}", script.display()))?;
    fs::copy(new_binary, &temp).with_context(|| {
        let _ = fs::remove_file(&script);
        format!("could not copy the update to {}", temp.display())
    })?;
    if let Err(e) = fs::set_permissions(&temp, fs::Permissions::from_mode(0o755)) {
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(&script);
        return Err(e).with_context(|| format!("could not mark {} as executable", temp.display()));
    }
    if let Err(e) = fs::rename(&temp, exe) {
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(&script);
        return Err(e).with_context(|| format!("could not replace {}", exe.display()));
    }

    let mut cmd = Command::new("sh");
    cmd.arg(&script).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Err(e) = cmd.spawn() {
        // The binary is already in place; the next launch uses it even if this
        // helper could not start.
        log::warn!("update: could not start the restart helper: {e}");
    }
    Ok(())
}

#[cfg(windows)]
fn install_binary(exe: &Path, new_binary: &Path, staged: &Staged) -> Result<()> {
    use std::os::windows::process::CommandExt;

    let dir = exe.parent().ok_or_else(|| anyhow!("the executable has no parent folder"))?;
    let name = exe.file_name().and_then(|n| n.to_str()).unwrap_or("estivi.exe");
    let temp = dir.join(format!(".{name}.update-{}", std::process::id()));

    const PS: &str = r#"$ErrorActionPreference = 'SilentlyContinue'
$pidToWait = __PID__
$target = __TARGET__
$new = __NEW__
$stage = __STAGE__
$i = 0
while ($i -lt 300 -and (Get-Process -Id $pidToWait -ErrorAction SilentlyContinue)) { Start-Sleep -Milliseconds 200; $i++ }
if (Test-Path -LiteralPath $target) { (Get-Item -LiteralPath $target).IsReadOnly = $false }
$ok = $false
$i = 0
while ($i -lt 60 -and -not $ok) {
    try { Move-Item -LiteralPath $new -Destination $target -Force -ErrorAction Stop; $ok = $true }
    catch { Start-Sleep -Milliseconds 500; $i++ }
}
Start-Sleep -Milliseconds 200
if (Test-Path -LiteralPath $target) { Start-Process -FilePath $target -WorkingDirectory (Split-Path -Parent $target) }
Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
"#;
    let body = PS
        .replace("__PID__", &std::process::id().to_string())
        .replace("__TARGET__", &ps_quote(&exe.to_string_lossy()))
        .replace("__NEW__", &ps_quote(&temp.to_string_lossy()))
        .replace("__STAGE__", &ps_quote(&staged.dir.to_string_lossy()));
    let script = staged.dir.join("restart.ps1");
    // Windows PowerShell 5.1 reads a script without a BOM as ANSI; the BOM keeps
    // accented user folders intact.
    let mut bytes = Vec::with_capacity(body.len() + 3);
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(body.as_bytes());
    fs::write(&script, bytes).with_context(|| format!("could not write {}", script.display()))?;
    fs::copy(new_binary, &temp).with_context(|| {
        let _ = fs::remove_file(&script);
        format!("could not copy the update to {}", temp.display())
    })?;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden"])
        .arg("-File")
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    if let Err(e) = cmd.spawn() {
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(&script);
        return Err(e).with_context(|| format!("could not start the update helper {}", script.display()));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn bundle_root(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    if macos.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let root = contents.parent()?;
    if root.extension()?.to_str()? != "app" {
        return None;
    }
    Some(root.to_path_buf())
}

#[cfg(target_os = "macos")]
fn install_macos_bundle(root: &Path, new: &Path, staged: &Staged) -> Result<()> {
    let script = staged.dir.join("restart.sh");
    let body = format!(
        "#!/bin/sh\n\
         n=0\n\
         while kill -0 {pid} 2>/dev/null && [ \"$n\" -lt 100 ]; do sleep 0.1; n=$((n+1)); done\n\
         target={target}\n\
         new={new}\n\
         old=\"$target.old-{pid}\"\n\
         rm -rf \"$old\"\n\
         if mv \"$target\" \"$old\"; then\n\
           if cp -R \"$new\" \"$target\"; then\n\
             rm -rf \"$old\"\n\
           else\n\
             rm -rf \"$target\"\n\
             mv \"$old\" \"$target\"\n\
           fi\n\
         fi\n\
         xattr -dr com.apple.quarantine \"$target\" 2>/dev/null\n\
         open \"$target\"\n\
         ( sleep 5; rm -rf {stage} ) &\n",
        pid = std::process::id(),
        target = sh_quote(&root.to_string_lossy()),
        new = sh_quote(&new.to_string_lossy()),
        stage = sh_quote(&staged.dir.to_string_lossy()),
    );
    fs::write(&script, body).with_context(|| format!("could not write {}", script.display()))?;
    let mut cmd = Command::new("sh");
    cmd.arg(&script).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().with_context(|| format!("could not start the update helper {}", script.display()))?;
    Ok(())
}

#[cfg(unix)]
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(windows)]
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Remove staging folders left by an old run. Nothing is removed while it is
/// recent, so another open copy (or a helper that is still starting) is safe.
pub fn cleanup_stale() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else { return };
    let now = std::process::id();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        if !name.starts_with("estivi-update-") || name == format!("estivi-update-{now}") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_dir() {
            continue;
        }
        let old = meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(7 * 24 * 3600));
        if old {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(version: &str, platform: &str) -> String {
        format!(
            r#"{{
                "tag_name": "v{version}",
                "html_url": "https://github.com/fluoroom/estivi-tx/releases/tag/v{version}",
                "assets": [
                    {{"name": "estivi-{version}-{platform}.tar.gz", "browser_download_url": "https://example.test/estivi-{version}-{platform}.tar.gz", "size": 12345}},
                    {{"name": "SHA256SUMS.txt", "browser_download_url": "https://example.test/SHA256SUMS.txt", "size": 90}}
                ]
            }}"#
        )
    }

    #[test]
    fn platform_tags_match_the_release_workflow() {
        assert_eq!(platform_tag_for("linux", "x86_64").as_deref(), Some("linux-x86_64"));
        assert_eq!(platform_tag_for("windows", "x86_64").as_deref(), Some("windows-x86_64"));
        assert_eq!(platform_tag_for("windows", "x86").as_deref(), Some("windows-i686"));
        assert_eq!(platform_tag_for("macos", "aarch64").as_deref(), Some("macos-universal"));
        assert_eq!(platform_tag_for("macos", "x86_64").as_deref(), Some("macos-universal"));
        assert_eq!(platform_tag_for("linux", "aarch64"), None);
    }

    #[test]
    fn newer_release_is_selected() {
        let release = parse_latest_for(&sample("9.9.9", "linux-x86_64"), "linux", "x86_64").unwrap().unwrap();
        assert_eq!(release.version, "9.9.9");
        assert_eq!(release.asset.name, "estivi-9.9.9-linux-x86_64.tar.gz");
        assert_eq!(release.checksums.name, "SHA256SUMS.txt");
        assert!(release.page.ends_with("/v9.9.9"));
    }

    #[test]
    fn same_or_older_release_is_not_an_update() {
        assert!(parse_latest_for(&sample(CURRENT, "linux-x86_64"), "linux", "x86_64").unwrap().is_none());
        assert!(parse_latest_for(&sample("0.0.1", "linux-x86_64"), "linux", "x86_64").unwrap().is_none());
    }

    #[test]
    fn release_without_the_platform_asset_or_checksums_is_an_error() {
        let wrong = sample("9.9.9", "windows-x86_64");
        assert!(parse_latest_for(&wrong, "linux", "x86_64").is_err());
        let no_sums = sample("9.9.9", "linux-x86_64").replace("SHA256SUMS.txt", "OTHER.txt");
        assert!(parse_latest_for(&no_sums, "linux", "x86_64").is_err());
        assert!(parse_latest_for("{}", "linux", "x86_64").is_err());
    }

    #[test]
    fn checksums_accept_the_sha256sum_output_of_the_release_job() {
        let text = "\
            abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd  ./estivi-0.2.3-linux-x86_64.tar.gz\n\
            ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD *estivi-0.2.3-windows-x86_64.tar.gz\n\
            not a checksum  ./ignored\n";
        let sums = parse_checksums(text);
        assert_eq!(sums.len(), 2);
        assert_eq!(sums[0].0, "estivi-0.2.3-linux-x86_64.tar.gz");
        assert_eq!(sums[0].1, "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd");
        assert_eq!(sums[1].0, "estivi-0.2.3-windows-x86_64.tar.gz");
    }

    #[test]
    fn download_verifies_and_unpacks_a_local_release() {
        use std::io::{Read as _, Write as _};

        let exe = if cfg!(windows) { "estivi.exe" } else { "estivi" };
        let archive = {
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            {
                let mut builder = tar::Builder::new(&mut encoder);
                let data = b"#!/bin/sh\n";
                let mut header = tar::Header::new_gnu();
                header.set_mode(0o755);
                header.set_size(data.len() as u64);
                header.set_cksum();
                builder.append_data(&mut header, format!("estivi-0.2.3-test/{exe}"), &data[..]).unwrap();
                builder.finish().unwrap();
            }
            encoder.finish().unwrap()
        };
        let archive_len = archive.len() as u64;
        let hash = format!("{:x}", Sha256::digest(&archive));
        let checksums = format!("{hash}  estivi-0.2.3-test.tar.gz\n");
        let checksums_len = checksums.len() as u64;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for body in [archive, checksums.into_bytes()] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(head.as_bytes()).unwrap();
                stream.write_all(&body).unwrap();
            }
        });

        let release = Release {
            version: "0.2.3".into(),
            page: format!("http://127.0.0.1:{port}/release"),
            asset: Asset {
                name: "estivi-0.2.3-test.tar.gz".into(),
                url: format!("http://127.0.0.1:{port}/estivi-0.2.3-test.tar.gz"),
                size: archive_len,
            },
            checksums: Asset {
                name: CHECKSUMS.into(),
                url: format!("http://127.0.0.1:{port}/{CHECKSUMS}"),
                size: checksums_len,
            },
        };
        let mut progress = Vec::new();
        let staged = stage(&release, |done, total| progress.push((done, total))).unwrap();
        assert_eq!(staged.version, "0.2.3");
        assert_eq!(fs::read(&staged.binary).unwrap(), b"#!/bin/sh\n");
        assert!(staged.bundle.is_none());
        assert_eq!(progress.last().map(|p| p.0), Some(archive_len));
        fs::remove_dir_all(&staged.dir).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn archive_payload_is_found_and_made_executable() {
        let root = std::env::temp_dir().join(format!("estivi-update-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("test.tar.gz");
        {
            let file = File::create(&archive).unwrap();
            let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut builder = tar::Builder::new(encoder);
            let data = b"#!/bin/sh\n";
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, "estivi-9.9.9-linux-x86_64/estivi", &data[..]).unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }
        let unpacked = root.join("unpacked");
        extract_tar_gz(&archive, &unpacked).unwrap();
        let (binary, bundle) = find_payload_for(&unpacked, "linux").unwrap();
        assert!(bundle.is_none());
        assert_eq!(fs::read(&binary).unwrap(), b"#!/bin/sh\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(fs::metadata(&binary).unwrap().permissions().mode() & 0o111, 0);
        }
        fs::remove_dir_all(&root).unwrap();
    }
}
