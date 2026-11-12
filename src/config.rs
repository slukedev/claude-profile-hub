//! The config file, where Claude Profile Hub keeps its own files, and small runtime state.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::profile::{self, Profile};

/// Where Claude Profile Hub keeps things. Every path can be moved with `CLAUDE_PROFILE_HUB_HOME` (used by tests,
/// and by people who want everything in one folder).
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub state_file: PathBuf,
    /// Default parent folder for new profiles' data.
    pub profiles_dir: PathBuf,
    /// Generated icons, the link router and other files Claude Profile Hub owns.
    pub support_dir: PathBuf,
}

impl Paths {
    pub fn detect() -> Result<Self> {
        if let Some(home) = std::env::var_os("CLAUDE_PROFILE_HUB_HOME") {
            return Ok(Self::rooted(PathBuf::from(home)));
        }
        let dirs = directories::ProjectDirs::from("dev", "slukedev", "claude-profile-hub")
            .context("couldn't find your home folder")?;
        Ok(Self {
            config_file: dirs.config_dir().join("config.toml"),
            state_file: dirs.data_local_dir().join("state.toml"),
            profiles_dir: dirs.data_local_dir().join("profiles"),
            support_dir: dirs.data_local_dir().to_path_buf(),
        })
    }

    pub fn rooted(root: PathBuf) -> Self {
        Self {
            config_file: root.join("config.toml"),
            state_file: root.join("state.toml"),
            profiles_dir: root.join("profiles"),
            support_dir: root,
        }
    }

    pub fn icons_dir(&self) -> PathBuf {
        self.support_dir.join("icons")
    }
}

/// What to do with a `claude://` link (sign-in or connector callbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum LinkRouting {
    /// Ask which profile, with the last one used preselected.
    #[default]
    Ask,
    /// Send it to the profile used most recently.
    LastUsed,
    /// Send it to the default profile.
    Default,
}

impl LinkRouting {
    pub const ALL: [Self; 3] = [Self::Ask, Self::LastUsed, Self::Default];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask every time",
            Self::LastUsed => "Last used profile",
            Self::Default => "Default profile",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SortOrder {
    #[default]
    Manual,
    Name,
    Recent,
}

impl SortOrder {
    pub const ALL: [Self; 3] = [Self::Manual, Self::Name, Self::Recent];

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::Name => "Name",
            Self::Recent => "Recently used",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Path to Claude Desktop, when it isn't where Claude Profile Hub looks.
    pub claude_path: Option<PathBuf>,
    /// Profile opened by `claude-profile-hub launch` with no name, and by link routing "default".
    pub default_profile: Option<String>,
    pub link_routing: LinkRouting,
    /// Make Claude Profile Hub the `claude://` handler so sign-ins land in the right profile.
    pub route_links: bool,
    /// Check GitHub for a newer Claude Profile Hub once a day.
    pub check_updates: bool,
    /// Make a shortcut per profile in the app menu (Applications, Start menu, app grid).
    pub app_shortcuts: bool,
    pub desktop_shortcuts: bool,
    /// macOS: keep profile shortcuts pinned in the Dock.
    pub dock_pins: bool,
    /// Accent color of the TUI, any profile color name or #RRGGBB.
    pub accent: String,
    pub sort: SortOrder,
    /// Ask before quitting a running profile from the TUI.
    pub confirm_quit: bool,
    /// Flags passed to every profile.
    pub global_args: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            claude_path: None,
            default_profile: None,
            link_routing: LinkRouting::Ask,
            route_links: false,
            check_updates: true,
            app_shortcuts: true,
            desktop_shortcuts: false,
            dock_pins: false,
            accent: "orange".to_owned(),
            sort: SortOrder::Manual,
            confirm_quit: true,
            global_args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .with_context(|| format!("{} isn't valid Claude Profile Hub config", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("couldn't read {}", path.display())),
        }
    }

    /// Writes through a temporary file, so a crash never leaves a half-written config.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).context("couldn't encode the config")?;
        let body = format!(
            "# Claude Profile Hub config. Edit by hand or with `claude-profile-hub` (the TUI).\n# Reference: https://github.com/slukedev/claude-profile-hub#config\n\n{text}"
        );
        write_atomic(path, body.as_bytes())
    }

    pub fn find(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
    }

    pub fn position(&self, name: &str) -> Option<usize> {
        self.profiles.iter().position(|p| p.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Adds a profile with sensible defaults. `data_dir` defaults to a new folder under
    /// `paths.profiles_dir`.
    pub fn add_profile(&mut self, paths: &Paths, raw_name: &str, data_dir: Option<PathBuf>) -> Result<&mut Profile> {
        let name = profile::validate_name(raw_name)?;
        if self.find(&name).is_some() {
            bail!("there's already a profile named \"{name}\"");
        }
        let data_dir = match data_dir {
            Some(dir) => dir,
            None => self.unused_dir(paths, &profile::slug(&name)),
        };
        if self.profiles.iter().any(|p| p.data_dir == data_dir) {
            bail!("another profile already uses {}", data_dir.display());
        }
        let color = profile::default_color(&name, self.profiles.iter().map(|p| p.color.as_str()));
        self.profiles.push(Profile {
            name,
            data_dir,
            color: color.to_owned(),
            badge: String::new(),
            extra_args: Vec::new(),
            env: Default::default(),
            notes: String::new(),
            created: now(),
            last_used: 0,
            launches: 0,
            own_app: false,
        });
        let last = self.profiles.len() - 1;
        Ok(&mut self.profiles[last])
    }

    fn unused_dir(&self, paths: &Paths, slug: &str) -> PathBuf {
        let mut candidate = paths.profiles_dir.join(slug);
        let mut n = 2;
        while candidate.exists() || self.profiles.iter().any(|p| p.data_dir == candidate) {
            candidate = paths.profiles_dir.join(format!("{slug}-{n}"));
            n += 1;
        }
        candidate
    }

    /// Renames a profile. Its data folder stays where it is.
    pub fn rename(&mut self, old: &str, new_raw: &str) -> Result<String> {
        let new = profile::validate_name(new_raw)?;
        let idx = self.position(old).with_context(|| format!("there's no profile named \"{old}\""))?;
        if let Some(other) = self.position(&new) {
            if other != idx {
                bail!("there's already a profile named \"{new}\"");
            }
        }
        let old_name = std::mem::replace(&mut self.profiles[idx].name, new.clone());
        if self.settings.default_profile.as_deref() == Some(old_name.as_str()) {
            self.settings.default_profile = Some(new);
        }
        Ok(old_name)
    }

    pub fn remove(&mut self, name: &str) -> Result<Profile> {
        let idx = self.position(name).with_context(|| format!("there's no profile named \"{name}\""))?;
        let removed = self.profiles.remove(idx);
        if self.settings.default_profile.as_deref() == Some(removed.name.as_str()) {
            self.settings.default_profile = None;
        }
        Ok(removed)
    }

    /// Indices of profiles in the user's chosen order.
    pub fn sorted_indices(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.profiles.len()).collect();
        match self.settings.sort {
            SortOrder::Manual => {}
            SortOrder::Name => idx.sort_by_key(|&i| self.profiles[i].name.to_lowercase()),
            SortOrder::Recent => idx.sort_by_key(|&i| std::cmp::Reverse(self.profiles[i].last_used)),
        }
        idx
    }
}

/// Small facts Claude Profile Hub remembers between runs. Not worth putting in the user's config.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct State {
    pub last_profile: Option<String>,
    pub last_update_check: i64,
    pub latest_version: Option<String>,
    /// claude-profile-hub was set up as the claude:// handler, so `links off` knows to undo it.
    pub links_claimed: bool,
}

impl State {
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string(self).context("couldn't encode state")?;
        write_atomic(path, text.as_bytes())
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("config path has no folder")?;
    fs::create_dir_all(dir).with_context(|| format!("couldn't create {}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("couldn't write {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| format!("couldn't replace {}", path.display()))
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::rooted(dir.path().to_path_buf());
        (dir, paths)
    }

    #[test]
    fn missing_config_loads_as_default() {
        let (_d, p) = paths();
        assert_eq!(Config::load(&p.config_file).expect("load"), Config::default());
    }

    #[test]
    fn config_round_trips_through_toml() {
        let (_d, p) = paths();
        let mut c = Config::default();
        c.add_profile(&p, "Work", None).expect("add");
        c.profiles[0].env.insert("A".into(), "b".into());
        c.save(&p.config_file).expect("save");
        assert_eq!(Config::load(&p.config_file).expect("load"), c);
    }

    #[test]
    fn add_profile_rejects_duplicates_case_insensitively() {
        let (_d, p) = paths();
        let mut c = Config::default();
        c.add_profile(&p, "Work", None).expect("add");
        assert!(c.add_profile(&p, "work", None).is_err());
    }

    #[test]
    fn add_profile_picks_distinct_folders_for_similar_names() {
        let (_d, p) = paths();
        let mut c = Config::default();
        c.add_profile(&p, "Big Client", None).expect("add");
        c.add_profile(&p, "big-client", None).expect("add");
        assert_ne!(c.profiles[0].data_dir, c.profiles[1].data_dir);
    }

    #[test]
    fn rename_keeps_data_dir_and_updates_default() {
        let (_d, p) = paths();
        let mut c = Config::default();
        c.add_profile(&p, "Work", None).expect("add");
        c.settings.default_profile = Some("Work".into());
        let dir = c.profiles[0].data_dir.clone();
        c.rename("work", "Acme").expect("rename");
        assert_eq!(c.profiles[0].data_dir, dir);
        assert_eq!(c.settings.default_profile.as_deref(), Some("Acme"));
    }

    #[test]
    fn remove_clears_default_profile() {
        let (_d, p) = paths();
        let mut c = Config::default();
        c.add_profile(&p, "Work", None).expect("add");
        c.settings.default_profile = Some("Work".into());
        c.remove("Work").expect("remove");
        assert!(c.settings.default_profile.is_none());
    }

    #[test]
    fn sorted_indices_by_recent_puts_latest_first() {
        let (_d, p) = paths();
        let mut c = Config::default();
        c.add_profile(&p, "A", None).expect("add");
        c.add_profile(&p, "B", None).expect("add");
        c.profiles[1].last_used = 10;
        c.settings.sort = SortOrder::Recent;
        assert_eq!(c.sorted_indices(), vec![1, 0]);
    }
}
