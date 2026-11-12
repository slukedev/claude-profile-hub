//! Opening a profile (or bringing it back), routing `claude://` links, and stopping.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::config::{LinkRouting, now};
use crate::ctx::{Ctx, DEFAULT_NAME, Target};
use crate::platform;
use crate::process::Running;
use crate::profile::Profile;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A new Claude process was started.
    Started,
    /// The profile was already running; its window was brought forward.
    Focused,
}

/// Arguments for a profile's Claude process.
pub fn claude_args(global: &[String], profile: Option<&Profile>) -> Vec<String> {
    let mut args: Vec<String> = global.to_vec();
    if let Some(p) = profile {
        args.push(format!("--user-data-dir={}", p.data_dir.display()));
        args.extend(p.extra_args.iter().cloned());
    }
    args
}

/// Opens a profile, or brings its window back if it's running. `url` is handed to it.
pub fn launch(ctx: &mut Ctx, name: &str, url: Option<&str>) -> Result<Outcome> {
    // Claude may have updated since the last launch; bring the other profiles along first.
    // (Relaunches from here find the version already recorded, so this doesn't recurse.)
    if ctx.config.settings.follow_claude_updates {
        let _ = crate::follow::check(ctx, None);
    }
    let install = ctx.require_claude()?;
    let profile = match ctx.target(name)? {
        Target::Default => None,
        Target::Profile(p) => Some(p.clone()),
    };
    let running = Running::scan(Some(&install.exe));
    let pid = running.pid_for_dir(profile.as_ref().map(|p| p.data_dir.as_path()));

    let outcome = match pid {
        Some(pid) if platform::focus(pid, url)? => Outcome::Focused,
        _ => {
            let mut install = install;
            if let Some(p) = &profile {
                fs::create_dir_all(&p.data_dir).with_context(|| format!("couldn't create {}", p.data_dir.display()))?;
                if p.own_app {
                    if let Some(copy) = platform::own_app_copy(&ctx.paths, p, &install)? {
                        install = copy;
                    }
                }
            }
            let args = claude_args(&ctx.config.settings.global_args, profile.as_ref());
            let env = profile.as_ref().map(|p| p.env.clone()).unwrap_or_default();
            platform::start(&install, &args, &env, url)?;
            if pid.is_some() { Outcome::Focused } else { Outcome::Started }
        }
    };

    let name = profile.as_ref().map_or(DEFAULT_NAME, |p| p.name.as_str()).to_owned();
    if let Some(p) = ctx.config.find_mut(&name) {
        p.last_used = now();
        p.launches += 1;
    }
    ctx.state.last_profile = Some(name);
    ctx.save()?;
    ctx.save_state()?;

    // Claude makes itself the claude:// handler every time it starts; take it back.
    if ctx.config.settings.route_links && outcome == Outcome::Started {
        start_reclaimer(ctx)?;
    }
    Ok(outcome)
}

/// Decides which profile gets a `claude://` link and opens it there.
pub fn route(ctx: &mut Ctx, url: &str) -> Result<()> {
    let names: Vec<String> = ctx.targets().iter().map(|t| t.name().to_owned()).collect();
    let last = ctx.state.last_profile.clone().filter(|n| names.contains(n));
    let default = ctx.config.settings.default_profile.clone().filter(|n| names.contains(n));
    let fallback = last.or(default.clone()).unwrap_or_else(|| DEFAULT_NAME.to_owned());

    let chosen = match ctx.config.settings.link_routing {
        LinkRouting::LastUsed => Some(fallback),
        LinkRouting::Default => Some(default.unwrap_or(fallback)),
        LinkRouting::Ask if names.len() == 1 => Some(fallback),
        LinkRouting::Ask => match platform::pick("Continue in which Claude?", &names, &fallback) {
            platform::Pick::Chosen(n) => Some(n),
            platform::Pick::Cancelled => None,
            // No way to show a dialog here: the last profile is the best guess.
            platform::Pick::Unavailable => Some(fallback),
        },
    };
    if let Some(name) = chosen {
        launch(ctx, &name, Some(url))?;
    }
    Ok(())
}

/// Stops a profile. Returns false when it wasn't running.
pub fn stop(ctx: &Ctx, name: &str) -> Result<bool> {
    let target = ctx.target(name)?;
    let claude = ctx.claude();
    let running = Running::scan(claude.as_ref().map(|c| c.exe.as_path()));
    Ok(match running.pid_for_dir(target.data_dir()) {
        Some(pid) => crate::process::stop(pid),
        None => false,
    })
}

const RECLAIM_SECONDS: i64 = 60;

/// Keeps lanes the link handler for a minute after a profile starts. A single helper
/// process does this; a second launch just pushes its deadline back.
fn start_reclaimer(ctx: &Ctx) -> Result<()> {
    let until_file = ctx.paths.support_dir.join("reclaim.until");
    let heartbeat_fresh = fs::metadata(&until_file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(6));
    fs::create_dir_all(&ctx.paths.support_dir)?;
    fs::write(&until_file, (now() + RECLAIM_SECONDS).to_string())?;
    if !heartbeat_fresh {
        spawn_detached(Command::new(crate::ctx::gui_command()).args(["links", "reclaim"]))?;
    }
    Ok(())
}

/// The helper behind [`start_reclaimer`]: re-claims links every few seconds until the
/// deadline in `reclaim.until` passes.
pub fn run_reclaimer(ctx: &Ctx) -> Result<()> {
    let until_file = ctx.paths.support_dir.join("reclaim.until");
    loop {
        let until: i64 = fs::read_to_string(&until_file).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        if now() >= until {
            let _ = fs::remove_file(&until_file);
            return Ok(());
        }
        let _ = platform::claim_links(ctx);
        // Touch the file so launchers can tell this helper is alive.
        let _ = fs::write(&until_file, until.to_string());
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// Starts a program that outlives us, with no console and no inherited output (a
/// shortcut waiting on our stdout would otherwise hang until it exits).
pub fn spawn_detached(cmd: &mut Command) -> Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    cmd.spawn().map(drop).with_context(|| format!("couldn't start {:?}", cmd.get_program()))
}

/// Size of a folder in bytes, skipping anything unreadable. Symlinks aren't followed.
pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(e.path());
            } else if meta.is_file() {
                total += meta.len();
            }
        }
    }
    total
}

/// Signs a profile out by moving its data folder to the trash. Fails if it's running.
pub fn fresh_start(ctx: &Ctx, name: &str) -> Result<()> {
    let Target::Profile(p) = ctx.target(name)? else {
        anyhow::bail!("Claude's own profile isn't managed by {}; sign out inside Claude instead", crate::NAME);
    };
    let running = Running::scan(ctx.claude().as_ref().map(|c| c.exe.as_path()));
    if running.pid_for_dir(Some(&p.data_dir)).is_some() {
        anyhow::bail!("{} is running. Quit it first.", p.name);
    }
    if p.data_dir.exists() {
        trash::delete(&p.data_dir).with_context(|| format!("couldn't move {} to the trash", p.data_dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn claude_args_put_data_dir_before_profile_flags() {
        let p = Profile {
            name: "Work".into(),
            data_dir: PathBuf::from("/d/work"),
            color: "indigo".into(),
            badge: String::new(),
            extra_args: vec!["--lang=en".into()],
            env: Default::default(),
            notes: String::new(),
            created: 0,
            last_used: 0,
            launches: 0,
            own_app: false,
        };
        let args = claude_args(&["--disable-gpu".into()], Some(&p));
        assert_eq!(args, ["--disable-gpu", "--user-data-dir=/d/work", "--lang=en"]);
    }

    #[test]
    fn claude_args_for_default_profile_have_no_data_dir() {
        assert!(claude_args(&[], None).is_empty());
    }

    #[test]
    fn dir_size_adds_nested_files() {
        let d = tempfile::tempdir().expect("tempdir");
        fs::create_dir(d.path().join("a")).expect("mkdir");
        fs::write(d.path().join("a/x"), [0u8; 10]).expect("write");
        fs::write(d.path().join("y"), [0u8; 5]).expect("write");
        assert_eq!(dir_size(d.path()), 15);
    }
}
