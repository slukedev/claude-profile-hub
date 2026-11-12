//! Health checks, shown by `claude-profile-hub doctor` and the TUI's Doctor tab.

use crate::ctx::Ctx;
use crate::platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
    Info,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub level: Level,
    pub title: String,
    pub detail: String,
}

impl Check {
    fn new(level: Level, title: impl Into<String>, detail: impl Into<String>) -> Self {
        Self { level, title: title.into(), detail: detail.into() }
    }
}

pub fn run(ctx: &Ctx) -> Vec<Check> {
    let mut out = Vec::new();
    out.push(Check::new(
        Level::Info,
        format!("{} {}", crate::NAME, crate::VERSION),
        format!("{} on {}", crate::update::current_exe().display(), std::env::consts::OS),
    ));

    match ctx.claude() {
        Some(c) => out.push(Check::new(
            Level::Ok,
            "Claude Desktop found",
            format!("{}{}", c.app.display(), c.version.map(|v| format!(" (version {v})")).unwrap_or_default()),
        )),
        None => out.push(Check::new(
            Level::Fail,
            "Claude Desktop not found",
            format!("Install it from claude.ai/download, or run `{} config claude-path <path>`.", crate::BIN),
        )),
    }

    out.push(Check::new(Level::Info, "Config file", ctx.paths.config_file.display().to_string()));

    let mut dirs = std::collections::HashMap::new();
    for p in &ctx.config.profiles {
        if let Some(other) = dirs.insert(crate::process::dir_key(&p.data_dir), &p.name) {
            out.push(Check::new(
                Level::Fail,
                format!("{} and {} share a data folder", other, p.name),
                "They'd share one sign-in. Give one of them its own folder.",
            ));
        }
        if !p.data_dir.exists() {
            out.push(Check::new(
                Level::Info,
                format!("{} hasn't been opened yet", p.name),
                "Its folder is created the first time it opens.",
            ));
        }
        let settings = &ctx.config.settings;
        if (settings.app_shortcuts || settings.desktop_shortcuts) && platform::existing_shortcuts(&p.name).is_empty() {
            out.push(Check::new(
                Level::Warn,
                format!("{} has no shortcut", p.name),
                format!("Run `{} shortcuts` to make them.", crate::BIN),
            ));
        }
    }

    if ctx.config.settings.route_links {
        match platform::links_are_ours() {
            Some(true) => {
                out.push(Check::new(Level::Ok, "Sign-in links come here first", "claude:// asks which profile to use"))
            }
            Some(false) => out.push(Check::new(
                Level::Warn,
                "Claude took claude:// back",
                format!(
                    "It does that when it starts. Run `{} links on` or open a profile through {}.",
                    crate::BIN,
                    crate::NAME
                ),
            )),
            None => out.push(Check::new(Level::Warn, "Couldn't check the claude:// handler", "")),
        }
    } else {
        out.push(Check::new(
            Level::Info,
            "Link routing is off",
            "Sign-ins finish in whichever Claude started last. Turn it on in Settings.",
        ));
    }

    let s = &ctx.config.settings;
    match (s.follow_claude_updates, s.background_watcher, crate::follow::watcher_running(ctx)) {
        (false, _, _) => out.push(Check::new(
            Level::Info,
            "Following Claude updates is off",
            "Profiles keep running the old version after Claude updates until you reopen them.",
        )),
        (true, true, true) => out.push(Check::new(
            Level::Ok,
            "Watching for Claude updates",
            "Profiles reopen on the new version within seconds",
        )),
        (true, true, false) => out.push(Check::new(
            Level::Warn,
            "The background watcher isn't running",
            format!("Run `{} config watcher on` to start it again.", crate::BIN),
        )),
        (true, false, _) => out.push(Check::new(
            Level::Info,
            "Claude updates are followed when you open a profile",
            "Turn on the background watcher in Settings to follow them right away.",
        )),
    }

    let method = crate::update::InstallMethod::detect(&crate::update::current_exe());
    out.push(Check::new(
        Level::Info,
        "How to update",
        method.update_command().map_or_else(|| format!("`{} update`", crate::BIN), |c| format!("`{c}`")),
    ));
    out
}
