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

/// Layered icons are a macOS 26 feature.
pub fn compile_layered_icon(_layers: &crate::icons::Layered, _out: &Path) -> bool {
    false
}

/// Claude's icon file is read directly here; there's nothing to export.
pub fn export_app_icon(_app: &Path, _png: &Path) -> bool {
    false
}

/// Copies of Claude are a macOS thing; here the icon follows the taskbar identity.
pub fn own_app_copy(_paths: &Paths, _p: &Profile, _install: &Install, _running: bool) -> Result<Option<Install>> {
    Ok(None)
}

/// Nothing to adopt: there are no per-profile copies of Claude here.
pub fn adopt_updated_copy(_paths: &Paths, _install: &Install) -> Result<Option<i64>> {
    Ok(None)
}

pub fn own_icon_env(_p: &Profile) -> Vec<(String, String)> {
    Vec::new()
}

/// The taskbar identity (AppUserModelID) of a profile's windows and its shortcuts.
fn app_id(p_name: &str) -> String {
    format!("ClaudeProfileHub.Profile.{}", crate::profile::slug(p_name))
}

/// Interop for the two things PowerShell can't do on its own: setting a shortcut's
/// AppUserModelID, and giving another program's windows an AppUserModelID, relaunch
/// details and an icon. Compiled by Add-Type when a script needs it.
const TASKBAR_CS: &str = r#"
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;

public static class CphTaskbar {
  [StructLayout(LayoutKind.Sequential, Pack = 4)]
  struct PropertyKey { public Guid Fmtid; public uint Pid; public PropertyKey(uint pid) { Fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"); Pid = pid; } }

  [StructLayout(LayoutKind.Explicit, Size = 24)]
  struct PropVariant { [FieldOffset(0)] public ushort Vt; [FieldOffset(8)] public IntPtr Ptr; }

  [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IPropertyStore {
    [PreserveSig] int GetCount(out uint count);
    [PreserveSig] int GetAt(uint index, out PropertyKey key);
    [PreserveSig] int GetValue(ref PropertyKey key, out PropVariant value);
    [PreserveSig] int SetValue(ref PropertyKey key, ref PropVariant value);
    [PreserveSig] int Commit();
  }

  [DllImport("shell32.dll")] static extern int SHGetPropertyStoreForWindow(IntPtr hwnd, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out IPropertyStore store);
  [DllImport("shell32.dll", CharSet = CharSet.Unicode)] static extern int SHGetPropertyStoreFromParsingName(string path, IntPtr bc, int flags, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out IPropertyStore store);
  [DllImport("ole32.dll")] static extern int PropVariantClear(ref PropVariant pv);
  delegate bool EnumProc(IntPtr hwnd, IntPtr lParam);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc proc, IntPtr lParam);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr hwnd, uint cmd);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr LoadImage(IntPtr inst, string name, uint type, int cx, int cy, uint flags);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
  [DllImport("user32.dll")] static extern IntPtr SendMessageTimeout(IntPtr hwnd, uint msg, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);

  const uint ID = 5, RELAUNCH_COMMAND = 2, RELAUNCH_ICON = 3, RELAUNCH_NAME = 4;
  static Guid StoreIid = new Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99");

  static string Get(IPropertyStore store, uint pid) {
    var key = new PropertyKey(pid);
    PropVariant v;
    if (store.GetValue(ref key, out v) != 0) return null;
    string s = v.Vt == 31 ? Marshal.PtrToStringUni(v.Ptr) : null;
    PropVariantClear(ref v);
    return s;
  }

  static void Set(IPropertyStore store, uint pid, string value) {
    var key = new PropertyKey(pid);
    var v = new PropVariant { Vt = 31, Ptr = Marshal.StringToCoTaskMemUni(value) };
    store.SetValue(ref key, ref v);
    PropVariantClear(ref v);
  }

  public static void SetShortcutId(string lnk, string id) {
    IPropertyStore store;
    if (SHGetPropertyStoreFromParsingName(lnk, IntPtr.Zero, 2, ref StoreIid, out store) != 0) return;
    Set(store, ID, id);
    store.Commit();
    Marshal.ReleaseComObject(store);
  }

  static List<IntPtr> TopWindows(uint pid) {
    var found = new List<IntPtr>();
    EnumWindows((h, l) => {
      uint owner;
      GetWindowThreadProcessId(h, out owner);
      if (owner == pid && GetWindow(h, 4) == IntPtr.Zero) found.Add(h);
      return true;
    }, IntPtr.Zero);
    return found;
  }

  // Keeps the profile's windows on its own taskbar identity and icon until it quits.
  // Claude can make new windows (or reset these) at any time, so this keeps checking.
  public static void Watch(int pid, string id, string command, string name, string ico) {
    Process proc;
    try { proc = Process.GetProcessById(pid); } catch (ArgumentException) { return; }
    IntPtr big = LoadImage(IntPtr.Zero, ico, 1, GetSystemMetrics(11), GetSystemMetrics(12), 0x10);
    IntPtr small = LoadImage(IntPtr.Zero, ico, 1, GetSystemMetrics(49), GetSystemMetrics(50), 0x10);
    var started = DateTime.UtcNow;
    do {
      foreach (var h in TopWindows((uint)pid)) {
        IPropertyStore store;
        if (SHGetPropertyStoreForWindow(h, ref StoreIid, out store) == 0) {
          if (Get(store, ID) != id) {
            // The relaunch details go first: the taskbar reads them when the ID changes.
            Set(store, RELAUNCH_COMMAND, command);
            Set(store, RELAUNCH_NAME, name);
            Set(store, RELAUNCH_ICON, ico + ",0");
            Set(store, ID, id);
          }
          Marshal.ReleaseComObject(store);
        }
        IntPtr current;
        SendMessageTimeout(h, 0x7F, (IntPtr)1, IntPtr.Zero, 2, 1000, out current);
        if (big != IntPtr.Zero && current != big) {
          SendMessageTimeout(h, 0x80, (IntPtr)1, big, 2, 1000, out current);
          if (small != IntPtr.Zero) SendMessageTimeout(h, 0x80, IntPtr.Zero, small, 2, 1000, out current);
        }
      }
      // Quick at first, while Claude opens its windows, then at a slower pace.
    } while (!proc.WaitForExit((DateTime.UtcNow - started).TotalSeconds < 60 ? 500 : 3000));
  }
}
"#;

/// Finds the profile's Claude process and keeps its windows on the profile's taskbar
/// identity and icon until it quits. One per profile, thanks to the mutex.
const WATCH_PS: &str = r#"
$m = New-Object System.Threading.Mutex($false, 'Local\ClaudeProfileHubIcon-' + $env:CPH_ID)
if (-not $m.WaitOne(0)) { exit }
Add-Type -TypeDefinition $env:CPH_CS
$want = '--user-data-dir=' + $env:CPH_DIR
$deadline = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $deadline) {
  $p = Get-CimInstance Win32_Process -Filter ("Name='" + $env:CPH_EXE + "'") |
    Where-Object { $_.CommandLine -and $_.CommandLine.Contains($want) -and -not $_.CommandLine.Contains('--type=') } |
    Sort-Object CreationDate | Select-Object -First 1
  if ($p) { [CphTaskbar]::Watch([int]$p.ProcessId, $env:CPH_ID, $env:CPH_CMD, $env:CPH_NAME, $env:CPH_ICO); break }
  Start-Sleep -Milliseconds 500
}
"#;

/// Starts a hidden helper that gives the profile's windows its own taskbar identity
/// (grouped with its Start menu shortcut, which carries the same ID) and its icon. Without
/// it Windows groups every profile under Claude and shows Claude's icon.
pub fn show_own_icon(ctx: &Ctx, p: &Profile, install: &Install) -> Result<()> {
    let set = icons::write_for_ctx(ctx, p)?;
    let exe_name = install.exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let command = format!("\"{}\" launch \"{}\"", gui_command().display(), p.name);
    let mut cmd = Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", WATCH_PS])
        .env("CPH_CS", TASKBAR_CS)
        .env("CPH_ID", app_id(&p.name))
        .env("CPH_DIR", p.data_dir.as_os_str())
        .env("CPH_EXE", exe_name.replace('\'', ""))
        .env("CPH_CMD", command)
        .env("CPH_NAME", shortcut_title(&p.name))
        .env("CPH_ICO", set.ico.as_os_str());
    spawn_detached(&mut cmd)
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
# The same taskbar identity as the profile's windows, so the running Claude groups with
# this shortcut (and a pinned copy of it) and shows its icon.
Add-Type -TypeDefinition $env:CPH_CS
[CphTaskbar]::SetShortcutId($env:CPH_LNK, $env:CPH_ID)
"#;

fn write_lnk(lnk: &Path, target: &Path, args: &str, icon: &Path, desc: &str, id: &str) -> Result<()> {
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
            ("CPH_CS", TASKBAR_CS),
            ("CPH_ID", id),
        ],
    )
    .map(drop)
}

fn lnk_name(name: &str) -> String {
    format!("{}.lnk", shortcut_title(name))
}

pub fn install_shortcuts(ctx: &Ctx, p: &Profile) -> Result<Vec<PathBuf>> {
    let settings = &ctx.config.settings;
    let set = icons::write_for_ctx(ctx, p)?;
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
            write_lnk(&lnk, &target, &args, &set.ico, &desc, &app_id(&p.name))?;
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

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "ClaudeProfileHubWatcher";

/// Starts the watcher at sign-in (and now) through the Run key, or removes it.
pub fn set_autostart(_ctx: &Ctx, on: bool) -> Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?;
    if on {
        let cmd = gui_command();
        key.set_value(RUN_VALUE, &format!("\"{}\" watch", cmd.display()))?;
        spawn_detached(Command::new(cmd).arg("watch"))?;
    } else {
        let _ = key.delete_value(RUN_VALUE);
    }
    Ok(())
}
