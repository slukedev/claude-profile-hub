//! Linux (and other freedesktop systems): `.desktop` entries for shortcuts and for the
//! `claude://` handler, and zenity or kdialog for picking a profile.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use super::{Pick, desktop_dir, remove_quietly, shortcut_title};
use crate::claude::Install;
use crate::config::Paths;
use crate::ctx::{Ctx, gui_command};
use crate::icons;
use crate::launch::spawn_detached;
use crate::profile::Profile;

const ROUTER_FILE: &str = "claude-profile-hub-links.desktop";

pub fn start(install: &Install, args: &[String], env: &BTreeMap<String, String>, url: Option<&str>) -> Result<()> {
    let mut cmd = Command::new(&install.exe);
    cmd.args(args).envs(env);
    if let Some(url) = url {
        cmd.arg(url);
    }
    spawn_detached(&mut cmd)
}

/// Claude allows one instance per data folder and hands a second start to the running
/// one, so starting again is how to bring it forward.
pub fn focus(_pid: u32, _url: Option<&str>) -> Result<bool> {
    Ok(false)
}

/// Layered icons are a macOS 26 feature.
pub fn compile_layered_icon(_layers: &crate::icons::Layered, _out: &Path) -> bool {
    false
}

/// Claude's icon file is read directly here; there's nothing to export.
pub fn export_app_icon(_app: &Path, _png: &Path) -> bool {
    false
}

pub fn own_app_copy(_paths: &Paths, _p: &Profile, _install: &Install) -> Result<Option<Install>> {
    Ok(None)
}

fn has(tool: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(tool).is_file()))
}

pub fn pick(prompt: &str, options: &[String], preselect: &str) -> Pick {
    let gui = std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some();
    if !gui {
        return Pick::Unavailable;
    }
    let out = if has("zenity") {
        let mut cmd = Command::new("zenity");
        cmd.args(["--list", "--title", crate::NAME, "--text", prompt, "--column", "Profile", "--hide-header"]);
        // zenity has no preselection, so put the likely choice first.
        let mut ordered: Vec<&String> = options.iter().filter(|o| *o == preselect).collect();
        ordered.extend(options.iter().filter(|o| *o != preselect));
        cmd.args(ordered);
        cmd.output()
    } else if has("kdialog") {
        let mut cmd = Command::new("kdialog");
        cmd.args(["--title", crate::NAME, "--menu", prompt]);
        for o in options {
            cmd.arg(o).arg(o);
        }
        cmd.arg("--default").arg(preselect);
        cmd.output()
    } else {
        return Pick::Unavailable;
    };
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if s.is_empty() { Pick::Cancelled } else { Pick::Chosen(s) }
        }
        Ok(_) => Pick::Cancelled,
        Err(_) => Pick::Unavailable,
    }
}

fn applications_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.data_dir().join("applications"))
}

fn entry_name(p_name: &str) -> String {
    format!("claude-profile-hub-{}.desktop", crate::profile::slug(p_name))
}

/// Quotes an argument for a desktop entry's Exec key.
fn exec_quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // `%` is a field code in Exec, so a literal one is doubled.
    out.replace('%', "%%")
}

fn desktop_entry(p: &Profile, icon: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={title}\nComment=Open the {name} Claude profile\nExec={cmd} launch {arg}\nIcon={icon}\nTerminal=false\nCategories=Network;Chat;\nStartupNotify=true\nX-Generated-By={bin}\n",
        title = shortcut_title(&p.name),
        name = p.name,
        cmd = exec_quote(&gui_command().to_string_lossy()),
        arg = exec_quote(&p.name),
        icon = icon.display(),
        bin = crate::BIN,
    )
}

fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

pub fn install_shortcuts(ctx: &Ctx, p: &Profile) -> Result<Vec<PathBuf>> {
    let settings = &ctx.config.settings;
    let set = icons::write_for_ctx(ctx, p)?;
    let body = desktop_entry(p, &set.png);
    let mut made = Vec::new();
    for (dir, wanted) in [(applications_dir(), settings.app_shortcuts), (desktop_dir(), settings.desktop_shortcuts)] {
        let Some(dir) = dir else { continue };
        let file = dir.join(entry_name(&p.name));
        if wanted {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&file, &body)?;
            make_executable(&file)?;
            // GNOME won't run a Desktop launcher until it's marked trusted.
            let _ = Command::new("gio")
                .args(["set", "-t", "string"])
                .arg(&file)
                .args(["metadata::trusted", "true"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            made.push(file);
        } else {
            remove_quietly(&file);
        }
    }
    refresh_menu();
    Ok(made)
}

fn refresh_menu() {
    if let Some(dir) = applications_dir() {
        let _ = Command::new("update-desktop-database").arg(dir).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}

pub fn remove_shortcuts(_ctx: &Ctx, name: &str) -> Result<()> {
    for dir in [applications_dir(), desktop_dir()].into_iter().flatten() {
        remove_quietly(&dir.join(entry_name(name)));
    }
    refresh_menu();
    Ok(())
}

pub fn existing_shortcuts(name: &str) -> Vec<PathBuf> {
    [applications_dir(), desktop_dir()]
        .into_iter()
        .flatten()
        .map(|d| d.join(entry_name(name)))
        .filter(|p| p.exists())
        .collect()
}

pub fn claim_links(_ctx: &Ctx) -> Result<()> {
    let dir = applications_dir().context("no data folder")?;
    std::fs::create_dir_all(&dir)?;
    let mime: String = crate::SCHEMES.iter().map(|s| format!("x-scheme-handler/{s};")).collect();
    std::fs::write(
        dir.join(ROUTER_FILE),
        format!(
            "[Desktop Entry]\nType=Application\nName=Claude Profile Hub Links\nExec={} route %u\nNoDisplay=true\nTerminal=false\nMimeType={mime}\n",
            exec_quote(&gui_command().to_string_lossy())
        ),
    )?;
    for s in crate::SCHEMES {
        Command::new("xdg-mime")
            .args(["default", ROUTER_FILE, &format!("x-scheme-handler/{s}")])
            .status()
            .context("couldn't run xdg-mime")?;
    }
    refresh_menu();
    Ok(())
}

pub fn release_links(_ctx: &Ctx) -> Result<()> {
    if let Some(dir) = applications_dir() {
        remove_quietly(&dir.join(ROUTER_FILE));
    }
    refresh_menu();
    Ok(())
}

pub fn links_are_ours() -> Option<bool> {
    let out = Command::new("xdg-mime").args(["query", "default", "x-scheme-handler/claude"]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim() == ROUTER_FILE)
}

pub fn open_path(path: &Path) -> Result<()> {
    spawn_detached(Command::new("xdg-open").arg(path))
}

fn autostart_file() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|d| d.config_dir().join("autostart/claude-profile-hub-watcher.desktop"))
}

/// Starts the watcher at login (and now) through XDG autostart, or removes it.
pub fn set_autostart(_ctx: &Ctx, on: bool) -> Result<()> {
    let file = autostart_file().context("no config folder")?;
    if !on {
        remove_quietly(&file);
        return Ok(());
    }
    std::fs::create_dir_all(file.parent().context("no parent")?)?;
    std::fs::write(
        &file,
        format!(
            "[Desktop Entry]\nType=Application\nName=Claude Profile Hub watcher\nExec={} watch\nNoDisplay=true\nX-GNOME-Autostart-enabled=true\n",
            exec_quote(&gui_command().to_string_lossy())
        ),
    )?;
    spawn_detached(Command::new(gui_command()).arg("watch"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_quote_escapes_specials_and_percent() {
        assert_eq!(exec_quote(r#"a "b" $c 100%"#), r#""a \"b\" \$c 100%%""#);
    }
}
