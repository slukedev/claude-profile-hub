//! What differs between macOS, Windows and Linux: starting Claude, shortcuts, link
//! handling and native dialogs. Each OS module exposes the same functions.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(all(unix, not(target_os = "macos")))]
mod linux;
#[cfg(all(unix, not(target_os = "macos")))]
pub use linux::*;

use std::path::PathBuf;

/// Result of asking the user to choose a profile in a native dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    Chosen(String),
    Cancelled,
    /// No dialog could be shown (no GUI session, no dialog tool).
    Unavailable,
}

/// The name shortcuts and windows show for a profile.
pub fn shortcut_title(profile_name: &str) -> String {
    format!("Claude {profile_name}")
}

pub fn desktop_dir() -> Option<PathBuf> {
    directories::UserDirs::new().and_then(|d| d.desktop_dir().map(PathBuf::from))
}

/// Human name of the place app shortcuts go on this platform.
pub fn app_menu_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Applications folder"
    } else if cfg!(windows) {
        "Start menu"
    } else {
        "app launcher"
    }
}

/// Removes a shortcut file or bundle we made, ignoring ones that are already gone.
pub(crate) fn remove_quietly(path: &std::path::Path) {
    let result = if path.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) };
    if let Err(e) = result {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("note: couldn't remove {}: {e}", path.display());
        }
    }
}
