//! Everything a command needs: paths, config and state, loaded once.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::claude::{self, Install};
use crate::config::{Config, Paths, State};
use crate::profile::Profile;

pub struct Ctx {
    pub paths: Paths,
    pub config: Config,
    pub state: State,
}

/// The profile a command acts on: Claude's normal profile, or one of ours.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    Default,
    Profile(&'a Profile),
}

impl Target<'_> {
    pub fn name(&self) -> &str {
        match self {
            Self::Default => DEFAULT_NAME,
            Self::Profile(p) => &p.name,
        }
    }

    pub fn data_dir(&self) -> Option<&Path> {
        match self {
            Self::Default => None,
            Self::Profile(p) => Some(&p.data_dir),
        }
    }
}

/// How Claude's own, unmanaged profile is named in lists and on the command line.
pub const DEFAULT_NAME: &str = "Claude";

impl Ctx {
    pub fn load() -> Result<Self> {
        let paths = Paths::detect()?;
        let config = Config::load(&paths.config_file)?;
        let state = State::load(&paths.state_file);
        Ok(Self { paths, config, state })
    }

    pub fn save(&self) -> Result<()> {
        self.config.save(&self.paths.config_file)
    }

    pub fn save_state(&self) -> Result<()> {
        self.state.save(&self.paths.state_file)
    }

    pub fn claude(&self) -> Option<Install> {
        claude::find(self.config.settings.claude_path.as_deref())
    }

    pub fn require_claude(&self) -> Result<Install> {
        if let Some(p) = &self.config.settings.claude_path {
            return claude::Install::from_path(p).with_context(|| {
                format!(
                    "Claude Desktop isn't at {} (set in your config). Fix it with `{} config claude-path <path>`.",
                    p.display(),
                    crate::BIN
                )
            });
        }
        self.claude().with_context(|| {
            format!(
                "Couldn't find Claude Desktop. Install it from https://claude.ai/download, or point to it with `{} config claude-path <path>`.",
                crate::BIN
            )
        })
    }

    /// Finds a profile by name. "Claude" (or "default") is Claude's own profile, unless
    /// the user made a profile with that name.
    pub fn target(&self, name: &str) -> Result<Target<'_>> {
        if let Some(p) = self.config.find(name) {
            return Ok(Target::Profile(p));
        }
        if name.eq_ignore_ascii_case(DEFAULT_NAME) || name.eq_ignore_ascii_case("default") {
            return Ok(Target::Default);
        }
        // Accept a unique prefix, so `claude-profile-hub launch wo` opens Work.
        let lower = name.to_lowercase();
        let matches: Vec<&Profile> =
            self.config.profiles.iter().filter(|p| p.name.to_lowercase().starts_with(&lower)).collect();
        match matches.as_slice() {
            [one] => Ok(Target::Profile(one)),
            [] => bail!("there's no profile named \"{name}\". See `{} list`.", crate::BIN),
            many => bail!(
                "\"{name}\" matches {}. Be more specific.",
                many.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
            ),
        }
    }

    /// Every launchable target, Claude's own profile first.
    pub fn targets(&self) -> Vec<Target<'_>> {
        std::iter::once(Target::Default)
            .chain(self.config.sorted_indices().into_iter().map(|i| Target::Profile(&self.config.profiles[i])))
            .collect()
    }
}

/// The path shortcuts should call. Prefers the copy on PATH (for Homebrew that's a symlink
/// that survives upgrades) over the versioned file the current process runs from.
pub fn self_command() -> PathBuf {
    let current = std::env::current_exe().ok();
    let exe_name = format!("{}{}", crate::BIN, std::env::consts::EXE_SUFFIX);
    if let (Some(cur), Some(path)) = (&current, std::env::var_os("PATH")) {
        let cur_real = cur.canonicalize().ok();
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(&exe_name);
            if candidate.is_file() && candidate.canonicalize().ok() == cur_real {
                return candidate;
            }
        }
    }
    current.unwrap_or_else(|| PathBuf::from(crate::BIN))
}

/// On Windows, the windowless twin of the binary (`claude-profile-hub-w.exe`) when it's
/// installed beside us, so shortcuts don't flash a console window.
pub fn gui_command() -> PathBuf {
    let cmd = self_command();
    if cfg!(windows) {
        let w = cmd.with_file_name(format!("{}-w.exe", crate::BIN));
        if w.is_file() {
            return w;
        }
    }
    cmd
}
