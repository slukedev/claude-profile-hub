//! macOS: AppleScript applets as shortcuts, Dock pins, Apple events to reach a specific
//! Claude process, and a small applet that receives `claude://` links.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

use super::{Pick, desktop_dir, remove_quietly, shortcut_title};
use crate::claude::Install;
use crate::config::Paths;
use crate::ctx::{Ctx, gui_command};
use crate::icons;
use crate::profile::Profile;

const LSREGISTER: &str =
    "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
const BUNDLE_PREFIX: &str = "dev.claudeprofilehub";
const CLAUDE_BUNDLE_ID: &str = "com.anthropic.claudefordesktop";
const ROUTER_NAME: &str = "Claude Profile Hub Links";

/// Sends a reopen event (or a URL) to one Claude process. Every profile shares Claude's
/// bundle ID, so addressing the process by PID is the only way to pick one.
const FOCUS_JS: &str = r#"
ObjC.import('AppKit');
function code(s) {
  return ((s.charCodeAt(0) << 24) | (s.charCodeAt(1) << 16) | (s.charCodeAt(2) << 8) | s.charCodeAt(3)) >>> 0;
}
function run(argv) {
  var pid = parseInt(argv[0], 10);
  var url = argv.length > 1 ? argv[1] : '';
  var target = $.NSAppleEventDescriptor.descriptorWithProcessIdentifier(pid);
  var event = url
    ? $.NSAppleEventDescriptor.appleEventWithEventClassEventIDTargetDescriptorReturnIDTransactionID(code('GURL'), code('GURL'), target, -1, 0)
    : $.NSAppleEventDescriptor.appleEventWithEventClassEventIDTargetDescriptorReturnIDTransactionID(code('aevt'), code('rapp'), target, -1, 0);
  if (url) {
    event.setParamDescriptorForKeyword($.NSAppleEventDescriptor.descriptorWithString(url), code('----'));
  }
  var error = Ref();
  var reply = event.sendEventWithOptionsTimeoutError(3, 10, error);
  var app = $.NSRunningApplication.runningApplicationWithProcessIdentifier(pid);
  if (!app.isNil()) { app.activateWithOptions(3); }
  if (!reply || reply.isNil() || (error[0] && !error[0].isNil())) {
    throw new Error('Apple event was not delivered');
  }
}
"#;

/// Sets a Finder custom icon, as Get Info does. It lives outside the signed files.
const SETICON_JS: &str = r#"
ObjC.import('AppKit');
function run(argv) {
  var image = $.NSImage.alloc.initWithContentsOfFile(argv[0]);
  if (image.isNil()) { throw new Error('could not read ' + argv[0]); }
  if (!$.NSWorkspace.sharedWorkspace.setIconForFileOptions(image, argv[1], 0)) {
    throw new Error('could not set the icon of ' + argv[1]);
  }
}
"#;

const CLAIM_JS: &str = r#"
ObjC.import('CoreServices');
function run(argv) {
  for (var i = 1; i < argv.length; i++) {
    $.LSSetDefaultHandlerForURLScheme($(argv[i]), $(argv[0]));
  }
}
"#;

const HANDLER_JS: &str = r#"
ObjC.import('AppKit');
function run(argv) {
  var url = $.NSWorkspace.sharedWorkspace.URLForApplicationToOpenURL($.NSURL.URLWithString(argv[0] + '://x'));
  return url.isNil() ? '' : url.path.js;
}
"#;

const PICK_APPLESCRIPT: &str = r#"
on run argv
  set thePrompt to item 1 of argv
  set preselect to item 2 of argv
  set opts to items 3 thru -1 of argv
  activate
  set choice to choose from list opts with title "Claude Profile Hub" with prompt thePrompt default items {preselect} OK button name "Open"
  if choice is false then return ""
  return item 1 of choice
end run
"#;

fn jxa(script: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("osascript")
        .args(["-l", "JavaScript", "-e", script])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("couldn't run osascript")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub fn start(install: &Install, args: &[String], env: &BTreeMap<String, String>, url: Option<&str>) -> Result<()> {
    // `open -n` starts a separate instance through Launch Services, like a Dock click.
    let mut cmd = Command::new("open");
    cmd.arg("-n").arg("-a").arg(&install.app);
    for (k, v) in env {
        cmd.arg("--env").arg(format!("{k}={v}"));
    }
    if let Some(url) = url {
        cmd.arg(url);
    }
    if !args.is_empty() {
        cmd.arg("--args").args(args);
    }
    let status = cmd.status().context("couldn't run `open`")?;
    if !status.success() {
        bail!("macOS couldn't open {}", install.app.display());
    }
    Ok(())
}

/// A reopen event brings back a window even after it was closed; activating alone doesn't.
pub fn focus(pid: u32, url: Option<&str>) -> Result<bool> {
    let pid = pid.to_string();
    let mut args = vec![pid.as_str()];
    if let Some(url) = url {
        args.push(url);
    }
    match jxa(FOCUS_JS, &args) {
        Ok(_) => Ok(true),
        Err(_) if url.is_some() => bail!(
            "macOS blocked handing the link to Claude. Allow Claude Profile Hub (or your terminal) to control Claude in System Settings > Privacy & Security > Automation, then try again."
        ),
        // Without a link, starting it again is a fine fallback.
        Err(_) => Ok(false),
    }
}

/// Runs the profile from its own APFS clone of Claude.app with the profile's icon, so the
/// Dock and the app switcher show which profile is which. The clone is unmodified, so
/// Anthropic's signature stays valid; the icon is a Finder custom icon outside the seal.
pub fn own_app_copy(paths: &Paths, p: &Profile, install: &Install) -> Result<Option<Install>> {
    let dir = paths.support_dir.join("apps").join(p.slug());
    let dest = dir.join(format!("{}.app", shortcut_title(&p.name)));
    std::fs::create_dir_all(&dir)?;
    // Any older copy (from before a rename) is replaced.
    for e in std::fs::read_dir(&dir)?.flatten() {
        let path = e.path();
        if path.extension().is_some_and(|x| x == "app") && path != dest {
            remove_quietly(&path);
        }
    }
    let stamp_file = dir.join("source.stamp");
    let stamp = std::fs::metadata(install.app.join("Contents/Info.plist"))
        .and_then(|m| m.modified())
        .map(|t| format!("{t:?}:{}", install.version.as_deref().unwrap_or("")))
        .unwrap_or_default();
    let current = std::fs::read_to_string(&stamp_file).unwrap_or_default();
    if !dest.exists() || current != stamp {
        let tmp = dir.join(".new.app");
        remove_quietly(&tmp);
        // -c clones on APFS, which costs almost no space. Anywhere else it would be a
        // full copy, so fall back to the shared app instead.
        let ok = Command::new("cp").arg("-cR").arg(&install.app).arg(&tmp).status().is_ok_and(|s| s.success());
        if !ok {
            remove_quietly(&tmp);
            return Ok(None);
        }
        let _ = Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(&tmp).status();
        remove_quietly(&dest);
        std::fs::rename(&tmp, &dest)?;
        std::fs::write(&stamp_file, &stamp)?;
    }
    let set = icons::write_for(&paths.icons_dir(), p)?;
    let _ = jxa(SETICON_JS, &[&set.icns.to_string_lossy(), &dest.to_string_lossy()]);
    Ok(Install::from_path(&dest))
}

pub fn pick(prompt: &str, options: &[String], preselect: &str) -> Pick {
    let out = Command::new("osascript")
        .arg("-e")
        .arg(PICK_APPLESCRIPT)
        .arg(prompt)
        .arg(preselect)
        .args(options)
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let choice = String::from_utf8_lossy(&o.stdout).trim().to_owned();
            if choice.is_empty() { Pick::Cancelled } else { Pick::Chosen(choice) }
        }
        _ => Pick::Unavailable,
    }
}

fn apps_dir() -> Option<PathBuf> {
    crate::claude::home().map(|h| h.join("Applications").join(crate::NAME))
}

fn shortcut_path(name: &str) -> Option<PathBuf> {
    apps_dir().map(|d| d.join(format!("{}.app", shortcut_title(name))))
}

/// AppleScript string literal.
fn as_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn compile_applet(app: &Path, source: &str) -> Result<()> {
    remove_quietly(app);
    let mut child = Command::new("osacompile")
        .arg("-o")
        .arg(app)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't run osacompile")?;
    child.stdin.take().context("no stdin")?.write_all(source.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("osacompile failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Gives a freshly compiled applet its identity and icon, signs it and registers it.
/// osacompile signs the applet and every edit breaks that seal, so signing comes last.
fn finalize_applet(
    app: &Path,
    bundle_id: &str,
    icns: Option<&Path>,
    extra: impl FnOnce(&mut plist::Dictionary),
) -> Result<()> {
    let info = app.join("Contents/Info.plist");
    let mut value = plist::Value::from_file(&info).context("couldn't read the applet's Info.plist")?;
    let dict = value.as_dictionary_mut().context("Info.plist isn't a dictionary")?;
    dict.insert("CFBundleIdentifier".into(), bundle_id.into());
    dict.insert("CFBundleIconFile".into(), "applet".into());
    // CFBundleIconName points at Assets.car, which would override our icon.
    dict.remove("CFBundleIconName");
    extra(dict);
    value.to_file_xml(&info)?;
    let resources = app.join("Contents/Resources");
    remove_quietly(&resources.join("Assets.car"));
    if let Some(icns) = icns {
        std::fs::copy(icns, resources.join("applet.icns"))?;
    }
    let _ = Command::new("xattr").arg("-cr").arg(app).status();
    let signed = Command::new("codesign")
        .args(["--force", "--sign", "-"])
        .arg(app)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !signed {
        eprintln!("note: couldn't sign {}; macOS may refuse to open it", app.display());
    }
    let _ = Command::new(LSREGISTER).arg("-f").arg(app).status();
    Ok(())
}

pub fn install_shortcuts(ctx: &Ctx, p: &Profile) -> Result<Vec<PathBuf>> {
    let settings = &ctx.config.settings;
    let mut made = Vec::new();
    let Some(app) = shortcut_path(&p.name) else { return Ok(made) };
    if settings.app_shortcuts || settings.desktop_shortcuts || settings.dock_pins {
        std::fs::create_dir_all(app.parent().context("no parent")?)?;
        let cmd = gui_command();
        let source = format!(
            "on run\n\ttry\n\t\tdo shell script quoted form of {} & \" launch \" & quoted form of {}\n\ton error errText\n\t\tdisplay alert {} message errText as warning\n\tend try\nend run\n",
            as_string(&cmd.to_string_lossy()),
            as_string(&p.name),
            as_string(&format!("Couldn't open {}", shortcut_title(&p.name))),
        );
        compile_applet(&app, &source)?;
        let set = icons::write_for(&ctx.paths.icons_dir(), p)?;
        finalize_applet(&app, &format!("{BUNDLE_PREFIX}.profile.{}", p.slug()), Some(&set.icns), |_| {})?;
        made.push(app.clone());
    }
    if let Some(desktop) = desktop_dir() {
        let link = desktop.join(format!("{}.app", shortcut_title(&p.name)));
        remove_quietly(&link);
        if settings.desktop_shortcuts {
            // A symlink, not a copy: a copy would be a second app with the same bundle ID.
            std::os::unix::fs::symlink(&app, &link)?;
            made.push(link);
        }
    }
    if settings.dock_pins {
        dock_pin(&app)?;
    }
    Ok(made)
}

pub fn remove_shortcuts(_ctx: &Ctx, name: &str) -> Result<()> {
    if let Some(app) = shortcut_path(name) {
        let _ = dock_unpin(&app);
        remove_quietly(&app);
    }
    if let Some(desktop) = desktop_dir() {
        remove_quietly(&desktop.join(format!("{}.app", shortcut_title(name))));
    }
    Ok(())
}

pub fn existing_shortcuts(name: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = shortcut_path(name).into_iter().collect();
    if let Some(d) = desktop_dir() {
        out.push(d.join(format!("{}.app", shortcut_title(name))));
    }
    out.retain(|p| p.exists());
    out
}

fn router_path() -> Option<PathBuf> {
    apps_dir().map(|d| d.join(format!("{ROUTER_NAME}.app")))
}

fn build_router(ctx: &Ctx) -> Result<PathBuf> {
    let app = router_path().context("no home folder")?;
    std::fs::create_dir_all(app.parent().context("no parent")?)?;
    let cmd = gui_command();
    let source = format!(
        "on run\n\tdo shell script quoted form of {cmd} & \" pick\"\nend run\n\non open location theURL\n\ttry\n\t\tdo shell script quoted form of {cmd} & \" route \" & quoted form of theURL\n\ton error errText\n\t\tdisplay alert \"Couldn't open the link\" message errText as warning\n\tend try\nend open location\n",
        cmd = as_string(&cmd.to_string_lossy()),
    );
    compile_applet(&app, &source)?;
    let icon = Profile {
        name: "Links".into(),
        data_dir: PathBuf::new(),
        color: ctx.config.settings.accent.clone(),
        badge: "chat".into(),
        extra_args: Vec::new(),
        env: Default::default(),
        notes: String::new(),
        created: 0,
        last_used: 0,
        launches: 0,
        own_app: false,
    };
    let set = icons::write_for(&ctx.paths.icons_dir(), &icon)?;
    finalize_applet(&app, &format!("{BUNDLE_PREFIX}.links"), Some(&set.icns), |dict| {
        let mut url_type = plist::Dictionary::new();
        url_type.insert("CFBundleURLName".into(), "Claude links".into());
        url_type.insert(
            "CFBundleURLSchemes".into(),
            plist::Value::Array(crate::SCHEMES.iter().map(|s| (*s).into()).collect()),
        );
        dict.insert("CFBundleURLTypes".into(), plist::Value::Array(vec![url_type.into()]));
        // No Dock icon while it hands a link over.
        dict.insert("LSUIElement".into(), true.into());
    })?;
    Ok(app)
}

pub fn claim_links(ctx: &Ctx) -> Result<()> {
    let app = router_path().context("no home folder")?;
    if !app.exists() {
        build_router(ctx)?;
    }
    let id = format!("{BUNDLE_PREFIX}.links");
    let mut args = vec![id.as_str()];
    args.extend(crate::SCHEMES);
    jxa(CLAIM_JS, &args).map(drop)
}

pub fn release_links(_ctx: &Ctx) -> Result<()> {
    let mut args = vec![CLAUDE_BUNDLE_ID];
    args.extend(crate::SCHEMES);
    let result = jxa(CLAIM_JS, &args).map(drop);
    if let Some(app) = router_path() {
        remove_quietly(&app);
    }
    result
}

/// Whether `claude://` currently opens our router.
pub fn links_are_ours() -> Option<bool> {
    let handler = jxa(HANDLER_JS, &["claude"]).ok()?;
    Some(handler.contains(ROUTER_NAME))
}

pub fn open_path(path: &Path) -> Result<()> {
    Command::new("open").arg(path).status().context("couldn't run `open`")?;
    Ok(())
}

// ---- Dock ----------------------------------------------------------------------------

fn dock_read() -> Result<plist::Value> {
    let out = Command::new("defaults").args(["export", "com.apple.dock", "-"]).output()?;
    if !out.status.success() {
        bail!("couldn't read the Dock's settings");
    }
    plist::Value::from_reader_xml(out.stdout.as_slice()).context("the Dock's settings aren't valid")
}

/// Writes through cfprefsd (not straight to the file) and restarts the Dock.
fn dock_write(value: &plist::Value) -> Result<()> {
    let mut xml = Vec::new();
    value.to_writer_xml(&mut xml)?;
    let mut child = Command::new("defaults").args(["import", "com.apple.dock", "-"]).stdin(Stdio::piped()).spawn()?;
    child.stdin.take().context("no stdin")?.write_all(&xml)?;
    if !child.wait()?.success() {
        bail!("couldn't save the Dock's settings");
    }
    let _ = Command::new("killall").arg("Dock").status();
    Ok(())
}

fn file_url(path: &Path) -> String {
    let mut s = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => s.push(b as char),
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    if !s.ends_with('/') {
        s.push('/');
    }
    s
}

fn tile_url(tile: &plist::Value) -> Option<&str> {
    tile.as_dictionary()?
        .get("tile-data")?
        .as_dictionary()?
        .get("file-data")?
        .as_dictionary()?
        .get("_CFURLString")?
        .as_string()
}

fn hfs_time(path: &Path) -> i64 {
    const APPLE_EPOCH_OFFSET: i64 = 2_082_844_800;
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64 + APPLE_EPOCH_OFFSET)
}

fn dock_pin(app: &Path) -> Result<()> {
    let url = file_url(app);
    let mut dock = dock_read()?;
    let dict = dock.as_dictionary_mut().context("bad Dock settings")?;
    if !dict.contains_key("persistent-apps") {
        dict.insert("persistent-apps".into(), plist::Value::Array(Vec::new()));
    }
    let apps = dict.get_mut("persistent-apps").and_then(plist::Value::as_array_mut).context("bad Dock settings")?;
    if apps.iter().any(|t| tile_url(t).is_some_and(|u| u.eq_ignore_ascii_case(&url))) {
        return Ok(());
    }
    let label = app.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut file_data = plist::Dictionary::new();
    file_data.insert("_CFURLString".into(), url.into());
    file_data.insert("_CFURLStringType".into(), 15.into());
    let mut tile_data = plist::Dictionary::new();
    tile_data.insert("file-data".into(), file_data.into());
    tile_data.insert("file-label".into(), label.into());
    tile_data.insert("file-type".into(), 41.into());
    // The Dock ignores tiles without real modification dates.
    tile_data.insert("file-mod-date".into(), hfs_time(app).into());
    tile_data.insert("parent-mod-date".into(), hfs_time(app.parent().unwrap_or(app)).into());
    tile_data.insert("dock-extra".into(), false.into());
    tile_data.insert("is-beta".into(), false.into());
    let mut tile = plist::Dictionary::new();
    tile.insert("tile-type".into(), "file-tile".into());
    tile.insert("tile-data".into(), tile_data.into());
    // Keep our tiles together.
    let pos = apps
        .iter()
        .rposition(|t| tile_url(t).is_some_and(|u| u.contains(&file_url(&apps_dir().unwrap_or_default()))))
        .map_or(apps.len(), |i| i + 1);
    apps.insert(pos, tile.into());
    dock_write(&dock)
}

fn dock_unpin(app: &Path) -> Result<()> {
    let url = file_url(app);
    let mut dock = dock_read()?;
    let Some(apps) =
        dock.as_dictionary_mut().and_then(|d| d.get_mut("persistent-apps")).and_then(plist::Value::as_array_mut)
    else {
        return Ok(());
    };
    let before = apps.len();
    apps.retain(|t| !tile_url(t).is_some_and(|u| u.eq_ignore_ascii_case(&url)));
    if apps.len() != before {
        dock_write(&dock)?;
    }
    Ok(())
}
