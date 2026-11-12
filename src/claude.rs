//! Finding Claude Desktop on this computer.

use std::path::{Path, PathBuf};

/// A Claude Desktop install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// What the user thinks of as "the app": `Claude.app` on macOS, the executable elsewhere.
    pub app: PathBuf,
    /// The program that actually runs.
    pub exe: PathBuf,
    pub version: Option<String>,
}

impl Install {
    /// Builds an install from a path the user gave: an app bundle or an executable.
    pub fn from_path(path: &Path) -> Option<Self> {
        if path.extension().is_some_and(|e| e == "app") {
            let exe = mac_bundle_exe(path)?;
            return Some(Self { app: path.to_path_buf(), exe, version: mac_bundle_version(path) });
        }
        path.is_file().then(|| Self {
            app: path.to_path_buf(),
            exe: path.to_path_buf(),
            version: windows_version(path),
        })
    }
}

/// Looks for Claude Desktop, trying `override_path` first.
pub fn find(override_path: Option<&Path>) -> Option<Install> {
    if let Some(p) = override_path {
        return Install::from_path(p);
    }
    candidates().iter().find_map(|p| Install::from_path(p))
}

#[cfg(target_os = "macos")]
fn candidates() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/Applications/Claude.app")];
    if let Some(home) = home() {
        out.push(home.join("Applications/Claude.app"));
    }
    // Launch Services knows where it is even when someone moved it.
    if let Ok(o) = std::process::Command::new("mdfind")
        .arg("kMDItemCFBundleIdentifier == 'com.anthropic.claudefordesktop'")
        .output()
    {
        out.extend(String::from_utf8_lossy(&o.stdout).lines().map(PathBuf::from));
    }
    out
}

#[cfg(windows)]
fn candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        // The Squirrel stub stays put across updates and forwards its arguments.
        out.push(local.join("AnthropicClaude").join("claude.exe"));
        out.push(local.join("Programs").join("Claude").join("Claude.exe"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = std::env::var_os(var).map(PathBuf::from) {
            out.push(pf.join("Claude").join("Claude.exe"));
        }
    }
    out
}

#[cfg(all(unix, not(target_os = "macos")))]
fn candidates() -> Vec<PathBuf> {
    // There's no official Linux build; these are where the community packages put it.
    // Never pick up `claude` on PATH: that's Claude Code, the terminal app.
    let mut out = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        out.extend(std::env::split_paths(&path).map(|d| d.join("claude-desktop")));
    }
    out.extend(
        [
            "/usr/bin/claude-desktop",
            "/usr/lib/claude-desktop/claude-desktop",
            "/opt/Claude/claude",
            "/opt/claude-desktop/claude-desktop",
            "/var/lib/flatpak/exports/bin/com.anthropic.Claude",
        ]
        .map(PathBuf::from),
    );
    if let Some(home) = home() {
        out.push(home.join(".local/share/flatpak/exports/bin/com.anthropic.Claude"));
        // AppImages people keep in ~/Applications.
        if let Ok(entries) = std::fs::read_dir(home.join("Applications")) {
            out.extend(entries.flatten().map(|e| e.path()).filter(|p| {
                let n = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                n.starts_with("claude") && n.ends_with(".appimage")
            }));
        }
    }
    out
}

pub fn home() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf())
}

fn mac_bundle_exe(app: &Path) -> Option<PathBuf> {
    let macos = app.join("Contents/MacOS");
    let named = macos.join("Claude");
    if named.is_file() {
        return Some(named);
    }
    // A renamed copy (own-app) keeps the binary name, but be lenient anyway.
    std::fs::read_dir(&macos).ok()?.flatten().map(|e| e.path()).find(|p| p.is_file())
}

fn mac_bundle_version(app: &Path) -> Option<String> {
    let text = std::fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
    // Claude's Info.plist is XML, so a tiny scan avoids a plist parser on other platforms.
    let key = text.find("<key>CFBundleShortVersionString</key>")?;
    let rest = &text[key..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")?;
    Some(rest[start..start + end].trim().to_owned())
}

/// Squirrel installs keep each version in an `app-X.Y.Z` folder beside the stub.
fn windows_version(exe: &Path) -> Option<String> {
    let dir = exe.parent()?;
    let mut versions: Vec<Vec<u32>> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let v = name.strip_prefix("app-")?;
            v.split('.').map(|p| p.parse().ok()).collect()
        })
        .collect();
    versions.sort();
    versions.pop().map(|v| v.iter().map(u32::to_string).collect::<Vec<_>>().join("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_path_reads_mac_bundle_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = dir.path().join("Claude.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).expect("mkdir");
        std::fs::write(app.join("Contents/MacOS/Claude"), b"").expect("write");
        std::fs::write(
            app.join("Contents/Info.plist"),
            "<dict><key>CFBundleShortVersionString</key>\n<string>1.2.3</string></dict>",
        )
        .expect("write");
        let install = Install::from_path(&app).expect("install");
        assert_eq!(install.version.as_deref(), Some("1.2.3"));
        assert_eq!(install.exe, app.join("Contents/MacOS/Claude"));
    }

    #[test]
    fn from_path_picks_newest_squirrel_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("claude.exe");
        std::fs::write(&exe, b"").expect("write");
        for v in ["app-0.9.10", "app-0.10.2", "app-0.10.10", "packages"] {
            std::fs::create_dir(dir.path().join(v)).expect("mkdir");
        }
        assert_eq!(Install::from_path(&exe).and_then(|i| i.version).as_deref(), Some("0.10.10"));
    }

    #[test]
    fn from_path_rejects_missing_files() {
        assert!(Install::from_path(Path::new("/nonexistent/claude")).is_none());
    }
}
