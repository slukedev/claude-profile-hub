//! Which profiles are running, and stopping them.

use std::collections::HashMap;
use std::path::Path;

#[cfg(not(windows))]
use sysinfo::Pid;
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// Which profile a Claude process belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Owner {
    /// Claude's normal profile (no `--user-data-dir`).
    Default,
    /// A profile, keyed by its normalized data folder.
    Dir(String),
}

/// Normalizes a folder for comparison: no trailing separator, and case-folded on
/// Windows and macOS, whose file systems usually ignore case.
pub fn dir_key(path: &Path) -> String {
    let s = path.to_string_lossy();
    let s = s.trim_end_matches(['/', '\\']);
    if cfg!(any(windows, target_os = "macos")) { s.to_lowercase() } else { s.to_owned() }
}

/// Decides whether a process is a Claude Desktop main process, and whose.
///
/// Chromium helpers (renderer, GPU, utility) carry `--type=`; only the main process
/// doesn't, so it's the one to focus or stop.
pub fn classify(exe: Option<&Path>, cmd: &[String], claude_exe: Option<&Path>) -> Option<Owner> {
    if cmd.iter().any(|a| a.starts_with("--type=")) {
        return None;
    }
    let mut iter = cmd.iter().skip(1);
    while let Some(arg) = iter.next() {
        if let Some(dir) = arg.strip_prefix("--user-data-dir=") {
            return Some(Owner::Dir(dir_key(Path::new(dir.trim_matches('"')))));
        }
        if arg == "--user-data-dir" {
            return iter.next().map(|d| Owner::Dir(dir_key(Path::new(d))));
        }
    }
    // No data folder: it's the default profile, but only if it really is Claude Desktop.
    // Claude Code's binary is also called `claude`, so match the install, not the name.
    let exe = exe?;
    let is_desktop = match claude_exe {
        Some(c) => same_file(exe, c),
        None => false,
    } || is_mac_claude(exe);
    is_desktop.then_some(Owner::Default)
}

/// Claude.app's main binary, or the same binary in a profile's own copy of the app
/// (`Claude Work.app`), which Claude's updater reopens without a data folder.
fn is_mac_claude(exe: &Path) -> bool {
    exe.file_name().is_some_and(|n| n == "Claude")
        && exe.parent().is_some_and(|d| d.ends_with("Contents/MacOS"))
        && exe.ancestors().nth(3).is_some_and(|app| app.extension().is_some_and(|x| x == "app"))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// One running profile's main process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    /// Unix seconds.
    pub started: i64,
}

/// A snapshot of running Claude profiles.
#[derive(Debug, Default, Clone)]
pub struct Running {
    procs: HashMap<Owner, Proc>,
}

impl Running {
    pub fn scan(claude_exe: Option<&Path>) -> Self {
        let Ok(mut sys) = System::new() else { return Self::default() };
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always),
        );
        let mut procs: HashMap<Owner, Proc> = HashMap::new();
        for (pid, proc_) in sys.processes() {
            let cmd: Vec<String> = proc_.cmd().iter().map(|s| s.to_string_lossy().into_owned()).collect();
            if let Some(owner) = classify(proc_.exe(), &cmd, claude_exe) {
                let p = Proc { pid: pid.as_u32(), started: i64::try_from(proc_.start_time()).unwrap_or(0) };
                // Keep the oldest if a profile somehow runs twice.
                procs
                    .entry(owner)
                    .and_modify(|q| {
                        if p.started < q.started {
                            *q = p
                        }
                    })
                    .or_insert(p);
            }
        }
        Self { procs }
    }

    #[cfg(test)]
    pub fn from_procs(procs: impl IntoIterator<Item = (Owner, Proc)>) -> Self {
        Self { procs: procs.into_iter().collect() }
    }

    pub fn get(&self, owner: &Owner) -> Option<Proc> {
        self.procs.get(owner).copied()
    }

    pub fn pid(&self, owner: &Owner) -> Option<u32> {
        self.get(owner).map(|p| p.pid)
    }

    pub fn owner_for_dir(dir: Option<&Path>) -> Owner {
        dir.map_or(Owner::Default, |d| Owner::Dir(dir_key(d)))
    }

    pub fn pid_for_dir(&self, dir: Option<&Path>) -> Option<u32> {
        self.pid(&Self::owner_for_dir(dir))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Owner, &Proc)> {
        self.procs.iter()
    }

    pub fn count(&self) -> usize {
        self.procs.len()
    }
}

/// Asks a process to quit the way closing its window would, so Claude saves its state.
pub fn stop(pid: u32) -> bool {
    #[cfg(windows)]
    {
        // Without /F, taskkill sends WM_CLOSE, which Claude handles like a normal quit.
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(not(windows))]
    {
        let Ok(mut sys) = System::new() else { return false };
        let pid = Pid::from_u32(pid);
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        sys.process(pid).and_then(|p| p.kill_with(sysinfo::Signal::Term)).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn classify_reads_equals_form() {
        let c = cmd(&["/x/Claude", "--user-data-dir=/home/me/work/"]);
        assert_eq!(classify(None, &c, None), Some(Owner::Dir(dir_key(Path::new("/home/me/work")))));
    }

    #[test]
    fn classify_reads_two_argument_form() {
        let c = cmd(&["claude.exe", "--user-data-dir", "/p/a b"]);
        assert_eq!(classify(None, &c, None), Some(Owner::Dir(dir_key(Path::new("/p/a b")))));
    }

    #[test]
    fn classify_ignores_chromium_helpers() {
        let c = cmd(&["/x/Claude Helper", "--type=renderer", "--user-data-dir=/p"]);
        assert_eq!(classify(None, &c, None), None);
    }

    #[test]
    fn classify_does_not_mistake_claude_code_for_desktop() {
        let c = cmd(&["claude", "--resume"]);
        assert_eq!(classify(Some(Path::new("/usr/local/bin/claude")), &c, None), None);
    }

    #[test]
    fn classify_recognizes_default_profile_on_mac() {
        let exe = Path::new("/Applications/Claude.app/Contents/MacOS/Claude");
        assert_eq!(classify(Some(exe), &cmd(&["Claude"]), None), Some(Owner::Default));
    }

    #[test]
    fn classify_recognizes_a_profile_copy_reopened_by_the_updater() {
        let exe =
            Path::new("/Users/me/Library/Application Support/hub/apps/work/Claude Work.app/Contents/MacOS/Claude");
        assert_eq!(classify(Some(exe), &cmd(&["Claude"]), None), Some(Owner::Default));
        let helper =
            Path::new("/Applications/Claude.app/Contents/Frameworks/Claude Helper.app/Contents/MacOS/Claude Helper");
        assert_eq!(classify(Some(helper), &cmd(&["Claude Helper"]), None), None);
    }
}
