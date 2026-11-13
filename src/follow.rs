//! Keeping every profile on the same Claude version.
//!
//! Claude Desktop updates itself from whichever profile asks first. The other profiles keep
//! running the old version, and on macOS the updater reopens Claude's own profile instead of
//! the one that asked. This notices the new version, quits every profile that started
//! before it, and reopens them, and puts the profile that triggered the update back.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::config::now;
use crate::ctx::{Ctx, DEFAULT_NAME};
use crate::launch;
use crate::platform;
use crate::process::{Owner, Running, dir_key};

/// How long after an update a freshly started default profile is taken to be the
/// updater's relaunch rather than the user opening it.
const RELAUNCH_WINDOW: i64 = 90;

/// The profile name for a running Claude, if it's one we know.
fn name_of(ctx: &Ctx, owner: &Owner) -> Option<String> {
    match owner {
        Owner::Default => Some(DEFAULT_NAME.to_owned()),
        Owner::Dir(k) => ctx.config.profiles.iter().find(|p| dir_key(&p.data_dir) == *k).map(|p| p.name.clone()),
    }
}

/// Profiles started before `installed_at`, so still on the old version.
pub fn stale(ctx: &Ctx, running: &Running, installed_at: i64) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = running
        .iter()
        // A couple of seconds of slack for clocks and file systems that round.
        .filter(|(_, p)| p.started > 0 && p.started + 2 < installed_at)
        .filter_map(|(o, p)| name_of(ctx, o).map(|n| (n, p.pid)))
        .collect();
    out.sort();
    out
}

fn wait_for_exit(exe: Option<&Path>, pids: &[u32], timeout: Duration) -> HashSet<u32> {
    let start = Instant::now();
    loop {
        let alive: HashSet<u32> = Running::scan(exe).iter().map(|(_, p)| p.pid).filter(|p| pids.contains(p)).collect();
        if alive.is_empty() || start.elapsed() > timeout {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// What [`check`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Followed {
    pub version: Option<String>,
    pub reopened: Vec<String>,
    /// Still running after being asked to quit (unsaved work, a dialog), so left alone.
    pub refused: Vec<String>,
}

/// If Claude's version changed since last time, restarts the profiles still on the old one.
/// `previous` is the watcher's last snapshot, used to spot the profile that triggered the
/// update (it's gone now) and bring it back.
pub fn check(ctx: &mut Ctx, previous: Option<&Running>) -> Result<Followed> {
    let mut done = Followed::default();
    let Some(mut install) = ctx.claude() else { return Ok(done) };
    // Claude's updater replaces the app it runs from, so an update started from a profile
    // running on its own copy lands in that copy. Put it in place of Claude itself, and
    // date the update from when the updater installed it.
    let adopted_at = match platform::adopt_updated_copy(&ctx.paths, &install) {
        Ok(Some(at)) => {
            install = ctx.claude().unwrap_or(install);
            Some(at)
        }
        Ok(None) => None,
        Err(e) => {
            eprintln!("note: Claude updated in a profile's copy, but it couldn't be put in place: {e:#}");
            None
        }
    };
    if !ctx.config.settings.follow_claude_updates {
        return Ok(done);
    }
    let Some(version) = install.version.clone() else { return Ok(done) };
    let known = ctx.state.claude_version.replace(version.clone());
    if known.as_deref() == Some(version.as_str()) {
        return Ok(done);
    }
    // Record it first, so the relaunches below don't see a change again.
    ctx.save_state()?;
    if known.is_none() {
        return Ok(done);
    }
    done.version = Some(version);
    let installed_at = adopted_at.or_else(|| install.installed_at()).unwrap_or_else(now);
    let exe = Some(install.exe.as_path());
    let running = Running::scan(exe);
    let old = stale(ctx, &running, installed_at);

    // The profile that installed the update has already quit.
    let mut triggers: Vec<String> = previous
        .map(|prev| {
            prev.iter()
                .filter(|(o, _)| running.get(o).is_none())
                .filter_map(|(o, _)| name_of(ctx, o))
                .filter(|n| n != DEFAULT_NAME)
                .collect()
        })
        .unwrap_or_default();

    for (_, pid) in &old {
        crate::process::stop(*pid);
    }
    let pids: Vec<u32> = old.iter().map(|(_, p)| *p).collect();
    let alive = wait_for_exit(exe, &pids, Duration::from_secs(30));
    for (name, pid) in &old {
        if alive.contains(pid) {
            done.refused.push(name.clone());
        } else if launch::launch(ctx, name, None).is_ok() {
            done.reopened.push(name.clone());
        }
    }
    triggers.retain(|t| !done.reopened.contains(t));
    if !triggers.is_empty() {
        put_back(ctx, previous, &triggers, installed_at, &mut done);
    }
    Ok(done)
}

/// Undoes the updater reopening Claude's own profile in place of the one that updated:
/// quits that default profile (if it wasn't open before) and reopens the triggers.
fn put_back(ctx: &mut Ctx, previous: Option<&Running>, triggers: &[String], installed_at: i64, done: &mut Followed) {
    let exe = ctx.claude().map(|c| c.exe);
    let deadline = Instant::now() + Duration::from_secs(RELAUNCH_WINDOW as u64);
    let default_was_open = previous.is_some_and(|p| p.get(&Owner::Default).is_some());
    loop {
        let now_running = Running::scan(exe.as_deref());
        if let Some(d) = now_running.get(&Owner::Default) {
            if !default_was_open && d.started + 2 >= installed_at {
                crate::process::stop(d.pid);
                wait_for_exit(exe.as_deref(), &[d.pid], Duration::from_secs(15));
                break;
            }
        }
        // The relaunch may land late, or not at all (Windows reopens the right profile).
        if Instant::now() > deadline || now() - installed_at > RELAUNCH_WINDOW {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    for t in triggers {
        let still_closed = ctx
            .config
            .find(t)
            .map(|p| Running::scan(exe.as_deref()).pid_for_dir(Some(&p.data_dir)).is_none())
            .unwrap_or(false);
        if still_closed && launch::launch(ctx, t, None).is_ok() {
            done.reopened.push(t.clone());
        }
    }
}

/// The background watcher: checks every few seconds until the setting is turned off
/// (or, with `forever`, until it's stopped).
pub fn run_watcher(forever: bool) -> Result<()> {
    let mut ctx = Ctx::load()?;
    let lock = ctx.paths.support_dir.join("watcher.pid");
    if let Some(pid) = std::fs::read_to_string(&lock).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
        if pid != std::process::id() && process_alive(pid) {
            return Ok(());
        }
    }
    std::fs::create_dir_all(&ctx.paths.support_dir)?;
    std::fs::write(&lock, std::process::id().to_string())?;
    let mut previous: Option<Running> = None;
    loop {
        if let Ok(fresh) = Ctx::load() {
            ctx = fresh;
        }
        if !ctx.config.settings.background_watcher && !forever {
            let _ = std::fs::remove_file(&lock);
            return Ok(());
        }
        if let Ok(f) = check(&mut ctx, previous.as_ref()) {
            if let Some(v) = &f.version {
                eprintln!("Claude {v}: reopened {:?}, left open {:?}", f.reopened, f.refused);
            }
        }
        previous = Some(Running::scan(ctx.claude().as_ref().map(|c| c.exe.as_path())));
        std::thread::sleep(Duration::from_secs(4));
    }
}

fn process_alive(pid: u32) -> bool {
    let Ok(mut sys) = sysinfo::System::new() else { return false };
    let pid = sysinfo::Pid::from_u32(pid);
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).is_some_and(|p| p.name().to_string_lossy().contains(crate::BIN))
}

/// Whether the background watcher is running now.
pub fn watcher_running(ctx: &Ctx) -> bool {
    std::fs::read_to_string(ctx.paths.support_dir.join("watcher.pid"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(process_alive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Paths, State};
    use crate::process::Proc;

    fn ctx() -> (tempfile::TempDir, Ctx) {
        let d = tempfile::tempdir().expect("tempdir");
        let paths = Paths::rooted(d.path().to_path_buf());
        let mut config = Config::default();
        config.add_profile(&paths, "Work", None).expect("add");
        config.add_profile(&paths, "Personal", None).expect("add");
        (d, Ctx { paths, config, state: State::default() })
    }

    #[test]
    fn stale_picks_profiles_started_before_the_update() {
        let (_d, ctx) = ctx();
        let work = Owner::Dir(dir_key(&ctx.config.profiles[0].data_dir));
        let personal = Owner::Dir(dir_key(&ctx.config.profiles[1].data_dir));
        let running = Running::from_procs([
            (work, Proc { pid: 1, started: 100 }),
            (personal, Proc { pid: 2, started: 500 }),
            (Owner::Default, Proc { pid: 3, started: 50 }),
            (Owner::Dir("/not/ours".into()), Proc { pid: 4, started: 10 }),
        ]);
        assert_eq!(stale(&ctx, &running, 400), vec![("Claude".to_owned(), 3), ("Work".to_owned(), 1)]);
    }

    #[test]
    fn check_records_first_version_without_restarting() {
        let (_d, mut ctx) = ctx();
        ctx.config.settings.claude_path = None;
        // With no Claude installed there's nothing to follow.
        assert_eq!(check(&mut ctx, None).expect("check"), Followed::default());
    }
}
