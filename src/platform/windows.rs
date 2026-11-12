//! Windows: Start menu and Desktop shortcuts (.lnk), `claude://` in the registry, and a
//! small WinForms dialog for picking a profile.

use std::collections::BTreeMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use super::{Pick, desktop_dir, remove_quietly, shortcut_title};
use crate::claude::Install;
use crate::config::Paths;
use crate::ctx::{Ctx, gui_command};
use crate::icons;
use crate::launch::spawn_detached;
use crate::profile::Profile;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Runs a PowerShell script with no window. Values go in through environment variables,
/// so nothing needs quoting inside the script.
fn powershell(script: &str, env: &[(&str, &str)]) -> Result<String> {
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().context("couldn't run PowerShell")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn start(install: &Install, args: &[String], env: &BTreeMap<String, String>, url: Option<&str>) -> Result<()> {
    let mut cmd = Command::new(&install.exe);
    cmd.args(args).envs(env);
    if let Some(url) = url {
        cmd.arg(url);
    }
    spawn_detached(&mut cmd)
}

/// Claude allows one instance per data folder and hands a second start (and its link) to
/// the running one, which comes to the front. So starting again is how to focus it.
pub fn focus(_pid: u32, _url: Option<&str>) -> Result<bool> {
    Ok(false)
}

pub fn own_app_copy(_paths: &Paths, _p: &Profile, _install: &Install) -> Result<Option<Install>> {
    Ok(None)
}

const PICK_PS: &str = r#"
Add-Type -AssemblyName System.Windows.Forms
$d = $env:CPH_PICK | ConvertFrom-Json
$f = New-Object Windows.Forms.Form
$f.Text = 'Claude Profile Hub'; $f.StartPosition = 'CenterScreen'; $f.TopMost = $true
$f.Width = 340; $f.Height = 320; $f.FormBorderStyle = 'FixedDialog'; $f.MaximizeBox = $false; $f.MinimizeBox = $false
$l = New-Object Windows.Forms.Label; $l.Text = $d.prompt; $l.Dock = 'Top'; $l.Height = 32; $l.Padding = '8,10,8,0'
$lb = New-Object Windows.Forms.ListBox; $lb.Dock = 'Fill'; $lb.IntegralHeight = $false
foreach ($o in $d.options) { [void]$lb.Items.Add($o) }
$lb.SelectedItem = $d.preselect
$b = New-Object Windows.Forms.Button; $b.Text = 'Open'; $b.Dock = 'Bottom'; $b.Height = 32; $b.DialogResult = 'OK'
$f.AcceptButton = $b
$lb.Add_DoubleClick({ $f.DialogResult = 'OK'; $f.Close() })
$f.Controls.Add($lb); $f.Controls.Add($b); $f.Controls.Add($l)
if ($f.ShowDialog() -eq 'OK' -and $lb.SelectedItem) { [Console]::Out.Write($lb.SelectedItem) }
"#;

pub fn pick(prompt: &str, options: &[String], preselect: &str) -> Pick {
    let payload = serde_json::json!({ "prompt": prompt, "options": options, "preselect": preselect }).to_string();
    match powershell(PICK_PS, &[("CPH_PICK", &payload)]) {
        Ok(s) if s.is_empty() => Pick::Cancelled,
        Ok(s) => Pick::Chosen(s),
        Err(_) => Pick::Unavailable,
    }
}

fn start_menu_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|a| a.join(r"Microsoft\Windows\Start Menu\Programs").join(crate::NAME))
}

const LNK_PS: &str = r#"
$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:CPH_LNK)
$s.TargetPath = $env:CPH_TARGET
$s.Arguments = $env:CPH_ARGS
$s.IconLocation = $env:CPH_ICON + ',0'
$s.Description = $env:CPH_DESC
$s.WorkingDirectory = $env:CPH_WD
$s.Save()
"#;

fn write_lnk(lnk: &Path, target: &Path, args: &str, icon: &Path, desc: &str) -> Result<()> {
    let wd = target.parent().unwrap_or(target);
    powershell(
        LNK_PS,
        &[
            ("CPH_LNK", &lnk.to_string_lossy()),
            ("CPH_TARGET", &target.to_string_lossy()),
            ("CPH_ARGS", args),
            ("CPH_ICON", &icon.to_string_lossy()),
            ("CPH_DESC", desc),
            ("CPH_WD", &wd.to_string_lossy()),
        ],
    )
    .map(drop)
}

fn lnk_name(name: &str) -> String {
    format!("{}.lnk", shortcut_title(name))
}

pub fn install_shortcuts(ctx: &Ctx, p: &Profile) -> Result<Vec<PathBuf>> {
    let settings = &ctx.config.settings;
    let set = icons::write_for(&ctx.paths.icons_dir(), p)?;
    let target = gui_command();
    // Profile names can't contain quotes, so this is safe to quote as is.
    let args = format!("launch \"{}\"", p.name);
    let desc = format!("Open the {} Claude profile", p.name);
    let mut made = Vec::new();
    for (dir, wanted) in [(start_menu_dir(), settings.app_shortcuts), (desktop_dir(), settings.desktop_shortcuts)] {
        let Some(dir) = dir else { continue };
        let lnk = dir.join(lnk_name(&p.name));
        if wanted {
            std::fs::create_dir_all(&dir)?;
            write_lnk(&lnk, &target, &args, &set.ico, &desc)?;
            made.push(lnk);
        } else {
            remove_quietly(&lnk);
        }
    }
    Ok(made)
}

pub fn remove_shortcuts(_ctx: &Ctx, name: &str) -> Result<()> {
    for dir in [start_menu_dir(), desktop_dir()].into_iter().flatten() {
        remove_quietly(&dir.join(lnk_name(name)));
    }
    Ok(())
}

pub fn existing_shortcuts(name: &str) -> Vec<PathBuf> {
    [start_menu_dir(), desktop_dir()]
        .into_iter()
        .flatten()
        .map(|d| d.join(lnk_name(name)))
        .filter(|p| p.exists())
        .collect()
}

fn classes() -> Result<RegKey> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Classes", winreg::enums::KEY_ALL_ACCESS)
        .context("couldn't open the registry")
}

fn set_handler(command: &str, icon: Option<&Path>) -> Result<()> {
    let classes = classes()?;
    for scheme in crate::SCHEMES {
        let (key, _) = classes.create_subkey(scheme)?;
        key.set_value("", &format!("URL:{scheme}"))?;
        key.set_value("URL Protocol", &"")?;
        if let Some(icon) = icon {
            let (di, _) = key.create_subkey("DefaultIcon")?;
            di.set_value("", &format!("\"{}\",0", icon.display()))?;
        }
        let (cmd, _) = key.create_subkey(r"shell\open\command")?;
        cmd.set_value("", &command)?;
    }
    Ok(())
}

pub fn claim_links(_ctx: &Ctx) -> Result<()> {
    set_handler(&format!("\"{}\" route \"%1\"", gui_command().display()), None)
}

/// Hands links back to Claude (which would also take them back on its next start).
pub fn release_links(ctx: &Ctx) -> Result<()> {
    match ctx.claude() {
        Some(c) => set_handler(&format!("\"{}\" \"%1\"", c.exe.display()), Some(&c.exe)),
        None => {
            let classes = classes()?;
            for scheme in crate::SCHEMES {
                let _ = classes.delete_subkey_all(scheme);
            }
            Ok(())
        }
    }
}

pub fn links_are_ours() -> Option<bool> {
    let cmd: String = classes().ok()?.open_subkey(r"claude\shell\open\command").ok()?.get_value("").ok()?;
    Some(cmd.to_lowercase().contains(&crate::BIN.to_lowercase()))
}

pub fn open_path(path: &Path) -> Result<()> {
    spawn_detached(Command::new("explorer.exe").arg(path))
}
