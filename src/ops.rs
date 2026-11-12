//! Changes that touch more than the config (shortcuts, the link handler, data folders),
//! shared by the command line and the TUI so both behave the same.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::ctx::Ctx;
use crate::platform;
use crate::process::Running;
use crate::profile::{Badge, parse_color};

pub fn add(
    ctx: &mut Ctx,
    name: &str,
    dir: Option<PathBuf>,
    color: Option<&str>,
    badge: Option<&str>,
) -> Result<String> {
    if let Some(c) = color {
        parse_color(c).with_context(|| format!("\"{c}\" isn't a color name or #RRGGBB"))?;
    }
    if let Some(b) = badge {
        Badge::parse(b).with_context(|| format!("\"{b}\" isn't a symbol, a single letter, or \"none\""))?;
    }
    let p = ctx.config.add_profile(&ctx.paths, name, dir)?;
    if let Some(c) = color {
        p.color = c.to_lowercase();
    }
    if let Some(b) = badge {
        p.badge = b.to_owned();
    }
    let name = p.name.clone();
    ctx.save()?;
    sync_shortcuts(ctx, &name)?;
    Ok(name)
}

pub fn is_running(ctx: &Ctx, name: &str) -> bool {
    let Some(p) = ctx.config.find(name) else { return false };
    Running::scan(ctx.claude().as_ref().map(|c| c.exe.as_path())).pid_for_dir(Some(&p.data_dir)).is_some()
}

/// Removes a profile and its shortcuts. With `purge`, its data folder goes to the trash.
pub fn remove(ctx: &mut Ctx, name: &str, purge: bool) -> Result<PathBuf> {
    if purge && is_running(ctx, name) {
        bail!("{name} is running. Quit it first.");
    }
    let real = ctx.config.find(name).with_context(|| format!("there's no profile named \"{name}\""))?.name.clone();
    platform::remove_shortcuts(ctx, &real)?;
    let removed = ctx.config.remove(&real)?;
    ctx.save()?;
    if purge && removed.data_dir.exists() {
        trash::delete(&removed.data_dir)
            .with_context(|| format!("couldn't move {} to the trash", removed.data_dir.display()))?;
    }
    Ok(removed.data_dir)
}

pub fn rename(ctx: &mut Ctx, old: &str, new: &str) -> Result<String> {
    let old_real = ctx.config.find(old).with_context(|| format!("there's no profile named \"{old}\""))?.name.clone();
    let new_name = crate::profile::validate_name(new)?;
    ctx.config.rename(&old_real, &new_name)?;
    ctx.save()?;
    if ctx.state.last_profile.as_deref() == Some(old_real.as_str()) {
        ctx.state.last_profile = Some(new_name.clone());
        ctx.save_state()?;
    }
    platform::remove_shortcuts(ctx, &old_real)?;
    sync_shortcuts(ctx, &new_name)?;
    Ok(new_name)
}

/// Makes or removes a profile's shortcuts to match the settings.
pub fn sync_shortcuts(ctx: &Ctx, name: &str) -> Result<Vec<PathBuf>> {
    let p = ctx.config.find(name).with_context(|| format!("there's no profile named \"{name}\""))?;
    let s = &ctx.config.settings;
    if s.app_shortcuts || s.desktop_shortcuts || s.dock_pins {
        platform::install_shortcuts(ctx, p)
    } else {
        platform::remove_shortcuts(ctx, &p.name)?;
        Ok(Vec::new())
    }
}

/// Rebuilds every profile's shortcuts. Returns each failure with its profile name.
pub fn sync_all_shortcuts(ctx: &Ctx) -> Vec<(String, anyhow::Error)> {
    ctx.config.profiles.iter().filter_map(|p| sync_shortcuts(ctx, &p.name).err().map(|e| (p.name.clone(), e))).collect()
}

/// Turns link routing on or off, and claims or releases `claude://` to match.
pub fn set_route_links(ctx: &mut Ctx, on: bool) -> Result<()> {
    if on {
        platform::claim_links(ctx)?;
    } else if ctx.config.settings.route_links || ctx.state.links_claimed {
        platform::release_links(ctx)?;
    }
    ctx.config.settings.route_links = on;
    ctx.state.links_claimed = on;
    ctx.save()?;
    ctx.save_state()
}

/// Human-friendly byte count.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

/// "3 hours ago" style time.
pub fn ago(unix: i64) -> String {
    if unix <= 0 {
        return "never".to_owned();
    }
    let secs = (crate::config::now() - unix).max(0);
    let (n, unit) = match secs {
        0..60 => return "just now".to_owned(),
        60..3600 => (secs / 60, "minute"),
        3600..86_400 => (secs / 3600, "hour"),
        86_400..2_592_000 => (secs / 86_400, "day"),
        _ => (secs / 2_592_000, "month"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_uses_decimal_units() {
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1_500_000), "1.5 MB");
    }

    #[test]
    fn ago_pluralizes() {
        let now = crate::config::now();
        assert_eq!(ago(0), "never");
        assert_eq!(ago(now - 3600), "1 hour ago");
        assert_eq!(ago(now - 7200), "2 hours ago");
    }
}
