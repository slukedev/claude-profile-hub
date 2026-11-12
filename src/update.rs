//! Checking GitHub for a newer release and updating in place.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::now;
use crate::ctx::Ctx;

const TARGET: &str = env!("PROFILE_HUB_TARGET");
const CHECK_EVERY: i64 = 24 * 60 * 60;

#[derive(Debug, Deserialize)]
struct ApiRelease {
    tag_name: String,
    assets: Vec<ApiAsset>,
}

#[derive(Debug, Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    assets: Vec<(String, String)>,
}

/// How this copy was installed, which decides how it should be updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Homebrew,
    Winget,
    Cargo,
    /// The install script or a downloaded binary: we update ourselves.
    Standalone,
}

impl InstallMethod {
    pub fn detect(exe: &Path) -> Self {
        let p = exe.to_string_lossy().replace('\\', "/").to_lowercase();
        if p.contains("/cellar/") || p.contains("/homebrew/") || p.contains("/linuxbrew/") {
            Self::Homebrew
        } else if p.contains("/winget/") {
            Self::Winget
        } else if p.contains("/.cargo/bin/") {
            Self::Cargo
        } else {
            Self::Standalone
        }
    }

    /// The command that updates this install, when it isn't us.
    pub fn update_command(self) -> Option<&'static str> {
        match self {
            Self::Homebrew => Some("brew upgrade claude-profile-hub"),
            Self::Winget => Some("winget upgrade slukedev.ClaudeProfileHub"),
            Self::Cargo => Some("cargo install claude-profile-hub --locked"),
            Self::Standalone => None,
        }
    }
}

pub fn current_exe() -> PathBuf {
    std::env::current_exe().and_then(|p| p.canonicalize()).unwrap_or_default()
}

/// Parses "v1.2.3" or "1.2.3" into numbers; anything after a `-` is ignored.
pub fn parse_version(v: &str) -> Option<Vec<u64>> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    core.split('.').map(|p| p.parse().ok()).collect()
}

/// Whether `latest` is newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    // macOS and Windows use the system's TLS (and its trusted certificates); Linux uses rustls.
    let provider = if cfg!(any(windows, target_os = "macos")) {
        ureq::tls::TlsProvider::NativeTls
    } else {
        ureq::tls::TlsProvider::Rustls
    };
    let tls = ureq::tls::TlsConfig::builder().provider(provider).build();
    ureq::Agent::new_with_config(ureq::Agent::config_builder().timeout_global(Some(timeout)).tls_config(tls).build())
}

pub fn fetch_latest(timeout: Duration) -> Result<Release> {
    // An override lets mirrors (and tests) stand in for GitHub.
    let url = std::env::var("CLAUDE_PROFILE_HUB_RELEASES_URL")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{}/releases/latest", crate::REPO));
    let release: ApiRelease = agent(timeout)
        .get(&url)
        .header("User-Agent", &format!("{}/{}", crate::BIN, crate::VERSION))
        .header("Accept", "application/vnd.github+json")
        .call()
        .context("couldn't reach GitHub")?
        .body_mut()
        .read_json()
        .context("GitHub sent something unexpected")?;
    Ok(Release {
        version: release.tag_name.trim_start_matches('v').to_owned(),
        assets: release.assets.into_iter().map(|a| (a.name, a.browser_download_url)).collect(),
    })
}

/// A newer version, if there is one. Asks GitHub at most once a day and remembers the
/// answer, so the TUI never waits on the network more than that.
pub fn check_cached(ctx: &mut Ctx) -> Option<String> {
    if !ctx.config.settings.check_updates || std::env::var_os("CLAUDE_PROFILE_HUB_NO_UPDATE_CHECK").is_some() {
        return None;
    }
    if now() - ctx.state.last_update_check > CHECK_EVERY {
        if let Ok(r) = fetch_latest(Duration::from_secs(5)) {
            ctx.state.latest_version = Some(r.version);
        }
        ctx.state.last_update_check = now();
        let _ = ctx.save_state();
    }
    ctx.state.latest_version.clone().filter(|v| is_newer(v, crate::VERSION))
}

fn download(url: &str) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    agent(Duration::from_secs(120))
        .get(url)
        .header("User-Agent", &format!("{}/{}", crate::BIN, crate::VERSION))
        .call()
        .with_context(|| format!("couldn't download {url}"))?
        .into_body()
        .into_reader()
        .take(200 * 1024 * 1024)
        .read_to_end(&mut body)?;
    Ok(body)
}

fn asset_url<'a>(release: &'a Release, name: &str) -> Option<&'a str> {
    release.assets.iter().find(|(n, _)| n == name).map(|(_, u)| u.as_str())
}

/// Finds `name`'s hash in a `SHA256SUMS` file.
pub fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name).then(|| hash.to_lowercase())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fetch_verified(release: &Release, sums: &str, name: &str) -> Result<Vec<u8>> {
    let url = asset_url(release, name).with_context(|| format!("the release has no build for {TARGET}"))?;
    let want = expected_hash(sums, name).with_context(|| format!("SHA256SUMS doesn't list {name}"))?;
    let data = download(url)?;
    let got = hex(&Sha256::digest(&data));
    if got != want {
        bail!("the download of {name} is corrupt (checksum mismatch), so nothing was changed");
    }
    Ok(data)
}

/// What [`self_update`] did.
pub enum Updated {
    AlreadyCurrent(String),
    To(String),
    /// Installed by a package manager, which should do the update.
    UseCommand(&'static str),
}

/// Downloads the newest build for this platform, checks it against the release's
/// SHA256SUMS, and swaps it in for the running binary.
pub fn self_update(force: bool) -> Result<Updated> {
    let exe = current_exe();
    if let Some(cmd) = InstallMethod::detect(&exe).update_command() {
        if !force {
            return Ok(Updated::UseCommand(cmd));
        }
    }
    let release = fetch_latest(Duration::from_secs(15))?;
    if !is_newer(&release.version, crate::VERSION) && !force {
        return Ok(Updated::AlreadyCurrent(release.version));
    }
    let sums_url = asset_url(&release, "SHA256SUMS").context("the release has no SHA256SUMS")?;
    let sums = String::from_utf8(download(sums_url)?).context("SHA256SUMS isn't text")?;

    let suffix = std::env::consts::EXE_SUFFIX;
    let main = fetch_verified(&release, &sums, &format!("{}-{TARGET}{suffix}", crate::BIN))?;
    let tmp = exe.with_extension("new");
    std::fs::write(&tmp, &main).with_context(|| format!("couldn't write {}", tmp.display()))?;
    make_executable(&tmp)?;
    self_replace::self_replace(&tmp).context("couldn't replace the running binary")?;
    let _ = std::fs::remove_file(&tmp);

    // Windows ships a windowless twin for shortcuts; keep it in step.
    if cfg!(windows) {
        let twin = exe.with_file_name(format!("{}-w.exe", crate::BIN));
        if twin.exists() {
            let data = fetch_verified(&release, &sums, &format!("{}-w-{TARGET}.exe", crate::BIN))?;
            let old = twin.with_extension("old");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&twin, &old)?;
            std::fs::write(&twin, data)?;
            let _ = std::fs::remove_file(&old);
        }
    }
    Ok(Updated::To(release.version))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_compares_numerically() {
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn expected_hash_finds_binary_mode_entries() {
        let sums = "abc  a.tar.gz\nDEF *claude-profile-hub-x\n";
        assert_eq!(expected_hash(sums, "claude-profile-hub-x").as_deref(), Some("def"));
        assert_eq!(expected_hash(sums, "missing"), None);
    }

    #[test]
    fn install_method_recognizes_package_managers() {
        assert_eq!(
            InstallMethod::detect(Path::new("/opt/homebrew/Cellar/claude-profile-hub/0.1.0/bin/x")),
            InstallMethod::Homebrew
        );
        assert_eq!(
            InstallMethod::detect(Path::new(
                r"C:\Users\a\AppData\Local\Microsoft\WinGet\Packages\x\claude-profile-hub.exe"
            )),
            InstallMethod::Winget
        );
        assert_eq!(
            InstallMethod::detect(Path::new("/home/a/.local/bin/claude-profile-hub")),
            InstallMethod::Standalone
        );
    }
}
