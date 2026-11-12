//! Command-line interface. With no command, the TUI opens.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

use crate::config::{LinkRouting, SortOrder};
use crate::ctx::{Ctx, Target};
use crate::launch::{self, Outcome};
use crate::process::Running;
use crate::profile::{Badge, COLORS, SYMBOLS, parse_color};
use crate::{BIN, NAME, doctor, ops, platform, update};

#[derive(Parser)]
#[command(
    name = BIN,
    version,
    about = "Run separate Claude Desktop profiles side by side, each with its own sign-in, chats and connectors.",
    after_help = "Run with no command to open the interactive manager."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Open a profile, or bring its window back if it's already running
    #[command(visible_aliases = ["open", "run"])]
    Launch {
        /// Profile name (a unique prefix works). Defaults to the default profile.
        name: Option<String>,
        /// A claude:// link to hand to the profile
        #[arg(long)]
        url: Option<String>,
    },
    /// List profiles and whether they're running
    #[command(visible_alias = "ls")]
    List {
        #[arg(long)]
        json: bool,
    },
    /// Add a profile
    #[command(visible_aliases = ["new", "create"])]
    Add {
        name: String,
        /// Color name or #RRGGBB
        #[arg(long)]
        color: Option<String>,
        /// Symbol (work, personal, home, code, star, heart, book, globe, bolt, leaf, moon, chat), a letter, or none
        #[arg(long)]
        badge: Option<String>,
        /// Use this folder for the profile's data (an existing one keeps its sign-in)
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Open it right away so you can sign in
        #[arg(long)]
        launch: bool,
    },
    /// Remove a profile and its shortcuts. Its data stays unless you pass --purge.
    #[command(visible_aliases = ["rm", "delete"])]
    Remove {
        name: String,
        /// Also move the profile's data (sign-in, chats cache, settings) to the trash
        #[arg(long)]
        purge: bool,
        #[arg(short, long)]
        yes: bool,
    },
    /// Rename a profile; its sign-in and data stay
    Rename { old: String, new: String },
    /// Change a profile's look and launch options
    Edit {
        name: String,
        #[arg(long)]
        color: Option<String>,
        #[arg(long)]
        badge: Option<String>,
        /// Add a flag passed to Claude (repeatable), e.g. --arg=--disable-gpu
        #[arg(long = "arg", allow_hyphen_values = true)]
        args: Vec<String>,
        /// Remove all extra flags first
        #[arg(long)]
        clear_args: bool,
        /// Set an environment variable, KEY=VALUE (repeatable)
        #[arg(long = "env")]
        env: Vec<String>,
        /// Remove an environment variable (repeatable)
        #[arg(long = "unset-env")]
        unset_env: Vec<String>,
        #[arg(long)]
        notes: Option<String>,
        /// macOS: run from its own copy of Claude so the Dock shows this profile's icon
        #[arg(long)]
        own_app: Option<Switch>,
    },
    /// Sign a profile out by moving its data folder to the trash
    #[command(visible_alias = "reset")]
    Fresh {
        name: String,
        #[arg(short, long)]
        yes: bool,
    },
    /// Quit a running profile (or all of them)
    #[command(visible_alias = "quit")]
    Stop {
        name: Option<String>,
        #[arg(long, conflicts_with = "name")]
        all: bool,
    },
    /// Choose a profile in a dialog and open it
    Pick,
    /// Make (or remove) shortcuts for every profile, or one
    Shortcuts {
        name: Option<String>,
        #[arg(long)]
        remove: bool,
    },
    /// Make sign-in links (claude://) ask which profile they're for
    Links {
        #[arg(value_enum)]
        action: LinksAction,
    },
    /// Show or change settings: `config`, `config <key>`, `config <key> <value>`
    Config { key: Option<String>, value: Option<String> },
    /// Check the setup and explain anything that's off
    Doctor,
    /// Update to the newest release
    #[command(visible_alias = "upgrade")]
    Update {
        /// Only report whether there's a newer version
        #[arg(long)]
        check: bool,
        /// Reinstall even if this is the newest, or if a package manager installed it
        #[arg(long)]
        force: bool,
    },
    /// Write the profile list (not their data) as TOML, to a file or stdout
    Export { file: Option<PathBuf> },
    /// Add profiles from a file made by `export`
    Import { file: PathBuf },
    /// Print shell completions
    Completions { shell: clap_complete::Shell },
    /// Watch for Claude updates and keep every profile on the new version (the background
    /// watcher runs this; turn it on with `config watcher on`)
    Watch,
    /// Hand a claude:// link to the right profile (used by the link handler)
    #[command(hide = true)]
    Route { url: String },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Switch {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum LinksAction {
    On,
    Off,
    Status,
    #[value(hide = true)]
    Reclaim,
}

/// Entry point shared by both binaries. Returns the process exit code.
pub fn main_entry() -> i32 {
    match run(Cli::parse()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
}

pub fn run(cli: Cli) -> Result<()> {
    let mut ctx = Ctx::load()?;
    let Some(cmd) = cli.command else {
        if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
            return crate::tui::run(ctx);
        }
        return list(&ctx, false);
    };
    match cmd {
        Cmd::Launch { name, url } => {
            let name = name
                .or_else(|| ctx.config.settings.default_profile.clone())
                .unwrap_or_else(|| crate::ctx::DEFAULT_NAME.to_owned());
            match launch::launch(&mut ctx, &name, url.as_deref())? {
                Outcome::Started => println!("Opened {name}."),
                Outcome::Focused => println!("{name} was already open; brought it to the front."),
            }
        }
        Cmd::List { json } => list(&ctx, json)?,
        Cmd::Add { name, color, badge, dir, launch: open } => {
            let name = ops::add(&mut ctx, &name, dir, color.as_deref(), badge.as_deref())?;
            let p = ctx.config.find(&name).context("profile vanished")?;
            println!("Added {name}. Its data lives in {}.", p.data_dir.display());
            if open {
                launch::launch(&mut ctx, &name, None)?;
                println!("Opened it. Sign in with the account you want for {name}.");
            } else {
                println!("Open it with `{BIN} launch {}` and sign in.", shell_word(&name));
            }
        }
        Cmd::Remove { name, purge, yes } => {
            let what = if purge { "remove it and move its data to the trash" } else { "remove it (its data stays)" };
            if !yes && !confirm(&format!("{name}: {what}?"))? {
                return Ok(());
            }
            let dir = ops::remove(&mut ctx, &name, purge)?;
            if purge {
                println!("Removed {name} and moved its data to the trash.");
            } else {
                println!("Removed {name}. Its data is still in {}.", dir.display());
            }
        }
        Cmd::Rename { old, new } => {
            let new = ops::rename(&mut ctx, &old, &new)?;
            println!("Renamed to {new}. Its sign-in and data are unchanged.");
        }
        Cmd::Edit { name, color, badge, args, clear_args, env, unset_env, notes, own_app } => {
            edit(&mut ctx, &name, color, badge, args, clear_args, env, unset_env, notes, own_app)?;
        }
        Cmd::Fresh { name, yes } => {
            if !yes && !confirm(&format!("Sign {name} out by moving its data to the trash?"))? {
                return Ok(());
            }
            launch::fresh_start(&ctx, &name)?;
            println!("{name} is signed out. Open it to sign in again.");
        }
        Cmd::Stop { name, all } => stop(&ctx, name, all)?,
        Cmd::Pick => {
            let names: Vec<String> = ctx.targets().iter().map(|t| t.name().to_owned()).collect();
            let pre = ctx.state.last_profile.clone().unwrap_or_else(|| names[0].clone());
            match platform::pick("Open which Claude?", &names, &pre) {
                platform::Pick::Chosen(n) => {
                    launch::launch(&mut ctx, &n, None)?;
                }
                platform::Pick::Cancelled => {}
                platform::Pick::Unavailable => bail!("couldn't show a dialog here; use `{BIN} launch <name>`"),
            }
        }
        Cmd::Shortcuts { name, remove } => shortcuts(&ctx, name, remove)?,
        Cmd::Links { action } => links(&mut ctx, action)?,
        Cmd::Config { key, value } => config(&mut ctx, key, value)?,
        Cmd::Doctor => {
            for c in doctor::run(&ctx) {
                let mark = match c.level {
                    doctor::Level::Ok => "ok  ",
                    doctor::Level::Warn => "warn",
                    doctor::Level::Fail => "FAIL",
                    doctor::Level::Info => "    ",
                };
                println!("{mark}  {}", c.title);
                if !c.detail.is_empty() {
                    println!("      {}", c.detail);
                }
            }
        }
        Cmd::Update { check, force } => {
            if check {
                let r = update::fetch_latest(std::time::Duration::from_secs(10))?;
                if update::is_newer(&r.version, crate::VERSION) {
                    println!("{} is out (you have {}). Run `{BIN} update`.", r.version, crate::VERSION);
                } else {
                    println!("You have the newest version ({}).", crate::VERSION);
                }
            } else {
                match update::self_update(force)? {
                    update::Updated::AlreadyCurrent(v) => println!("You have the newest version ({v})."),
                    update::Updated::To(v) => println!("Updated to {v}."),
                    update::Updated::UseCommand(c) => {
                        println!("This copy was installed by a package manager. Update it with:\n  {c}")
                    }
                }
            }
        }
        Cmd::Export { file } => {
            let out = crate::config::Config { profiles: ctx.config.profiles.clone(), ..Default::default() };
            let text = toml::to_string_pretty(&out)?;
            match file {
                Some(f) => {
                    std::fs::write(&f, text)?;
                    println!("Wrote {} profiles to {}.", out.profiles.len(), f.display());
                }
                None => print!("{text}"),
            }
        }
        Cmd::Import { file } => {
            let text = std::fs::read_to_string(&file).with_context(|| format!("couldn't read {}", file.display()))?;
            let theirs: crate::config::Config = toml::from_str(&text).context("that file isn't an export")?;
            let mut added = 0;
            for p in theirs.profiles {
                if ctx.config.find(&p.name).is_some() || ctx.config.profiles.iter().any(|q| q.data_dir == p.data_dir) {
                    println!("Skipped {} (already here).", p.name);
                    continue;
                }
                crate::profile::validate_name(&p.name)?;
                ctx.config.profiles.push(p);
                added += 1;
            }
            ctx.save()?;
            for (n, e) in ops::sync_all_shortcuts(&ctx) {
                eprintln!("note: shortcuts for {n}: {e:#}");
            }
            println!("Imported {added} profile(s).");
        }
        Cmd::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), BIN, &mut std::io::stdout());
        }
        Cmd::Route { url } => launch::route(&mut ctx, &url)?,
        // Started by hand with the setting off, it runs until stopped; the login item
        // stops when the setting is turned off.
        Cmd::Watch => crate::follow::run_watcher(!ctx.config.settings.background_watcher)?,
    }
    Ok(())
}

fn shell_word(s: &str) -> String {
    if s.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') { s.to_owned() } else { format!("\"{s}\"") }
}

fn confirm(question: &str) -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("{question} Pass --yes to confirm when not running interactively.");
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes" | "Yes"))
}

fn list(ctx: &Ctx, json: bool) -> Result<()> {
    let claude = ctx.claude();
    let running = Running::scan(claude.as_ref().map(|c| c.exe.as_path()));
    if json {
        let items: Vec<serde_json::Value> = ctx
            .targets()
            .iter()
            .map(|t| {
                let pid = running.pid_for_dir(t.data_dir());
                serde_json::json!({
                    "name": t.name(),
                    "managed": matches!(t, Target::Profile(_)),
                    "data_dir": t.data_dir(),
                    "running": pid.is_some(),
                    "pid": pid,
                    "default": ctx.config.settings.default_profile.as_deref() == Some(t.name()),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }
    if ctx.config.profiles.is_empty() {
        println!("No profiles yet. Add one with `{BIN} add Personal`, or run `{BIN}` to manage them.");
    }
    for t in ctx.targets() {
        let state = if running.pid_for_dir(t.data_dir()).is_some() { "running" } else { "" };
        let default = if ctx.config.settings.default_profile.as_deref() == Some(t.name()) { " (default)" } else { "" };
        let place = t.data_dir().map_or_else(|| "Claude's own profile".to_owned(), |d| d.display().to_string());
        println!("{:<24} {:<8} {place}", format!("{}{default}", t.name()), state);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn edit(
    ctx: &mut Ctx,
    name: &str,
    color: Option<String>,
    badge: Option<String>,
    args: Vec<String>,
    clear_args: bool,
    env: Vec<String>,
    unset_env: Vec<String>,
    notes: Option<String>,
    own_app: Option<Switch>,
) -> Result<()> {
    if let Some(c) = &color {
        parse_color(c).with_context(|| {
            format!(
                "\"{c}\" isn't a color. Use #RRGGBB or one of: {}",
                COLORS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
            )
        })?;
    }
    if let Some(b) = &badge {
        Badge::parse(b)
            .with_context(|| format!("\"{b}\" isn't a badge. Use a letter, none, or one of: {}", SYMBOLS.join(", ")))?;
    }
    let mut parsed_env = Vec::new();
    for e in env {
        let (k, v) = e.split_once('=').with_context(|| format!("\"{e}\" should look like KEY=VALUE"))?;
        parsed_env.push((k.to_owned(), v.to_owned()));
    }
    let p = ctx.config.find_mut(name).with_context(|| format!("there's no profile named \"{name}\""))?;
    let look_changed = color.is_some() || badge.is_some();
    if let Some(c) = color {
        p.color = c.to_lowercase();
    }
    if let Some(b) = badge {
        p.badge = b;
    }
    if clear_args {
        p.extra_args.clear();
    }
    p.extra_args.extend(args);
    for (k, v) in parsed_env {
        p.env.insert(k, v);
    }
    for k in unset_env {
        p.env.remove(&k);
    }
    if let Some(n) = notes {
        p.notes = n;
    }
    if let Some(s) = own_app {
        if !cfg!(target_os = "macos") {
            bail!("own-app is a macOS feature");
        }
        p.own_app = matches!(s, Switch::On);
    }
    let real = p.name.clone();
    ctx.save()?;
    if look_changed {
        ops::sync_shortcuts(ctx, &real)?;
    }
    println!("Updated {real}.");
    Ok(())
}

fn stop(ctx: &Ctx, name: Option<String>, all: bool) -> Result<()> {
    let names: Vec<String> = if all {
        ctx.targets().iter().map(|t| t.name().to_owned()).collect()
    } else {
        vec![name.context("say which profile, or --all")?]
    };
    let mut any = false;
    for n in &names {
        if launch::stop(ctx, n)? {
            println!("Asked {n} to quit.");
            any = true;
        } else if !all {
            println!("{n} isn't running.");
        }
    }
    if all && !any {
        println!("Nothing was running.");
    }
    Ok(())
}

fn shortcuts(ctx: &Ctx, name: Option<String>, remove: bool) -> Result<()> {
    let names: Vec<String> = match name {
        Some(n) => vec![ctx.config.find(&n).with_context(|| format!("there's no profile named \"{n}\""))?.name.clone()],
        None => ctx.config.profiles.iter().map(|p| p.name.clone()).collect(),
    };
    for n in names {
        if remove {
            platform::remove_shortcuts(ctx, &n)?;
            println!("Removed shortcuts for {n}.");
        } else {
            let made = ops::sync_shortcuts(ctx, &n)?;
            for m in &made {
                println!("{n}: {}", m.display());
            }
            if made.is_empty() {
                println!("{n}: shortcuts are turned off in settings.");
            }
        }
    }
    Ok(())
}

fn links(ctx: &mut Ctx, action: LinksAction) -> Result<()> {
    match action {
        LinksAction::On => {
            ops::set_route_links(ctx, true)?;
            println!(
                "Sign-in links now come to {NAME} first ({}).",
                ctx.config.settings.link_routing.label().to_lowercase()
            );
        }
        LinksAction::Off => {
            ops::set_route_links(ctx, false)?;
            println!("Sign-in links go straight to Claude again.");
        }
        LinksAction::Status => match (ctx.config.settings.route_links, platform::links_are_ours()) {
            (true, Some(true)) => println!("On. claude:// links come here first."),
            (true, _) => println!("On, but Claude has taken claude:// back. Run `{BIN} links on`."),
            (false, _) => println!("Off. claude:// links go to whichever Claude registered last."),
        },
        LinksAction::Reclaim => launch::run_reclaimer(ctx)?,
    }
    Ok(())
}

const CONFIG_KEYS: &[(&str, &str)] = &[
    ("claude-path", "path to Claude Desktop, or \"auto\""),
    ("default", "profile `launch` opens with no name, or \"none\""),
    ("link-routing", "ask | last-used | default"),
    ("check-updates", "on | off"),
    ("app-shortcuts", "on | off"),
    ("desktop-shortcuts", "on | off"),
    ("dock", "on | off (macOS)"),
    ("accent", "TUI accent color"),
    ("sort", "manual | name | recent"),
    ("confirm-quit", "on | off"),
    ("global-args", "flags for every profile, space separated, or \"none\""),
    ("follow-updates", "on | off: reopen profiles on the new Claude after it updates"),
    ("watcher", "on | off: watch for Claude updates in the background, from login"),
];

fn on_off(v: &str) -> Result<bool> {
    match v.to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Ok(true),
        "off" | "false" | "no" | "0" => Ok(false),
        _ => bail!("use on or off"),
    }
}

fn config(ctx: &mut Ctx, key: Option<String>, value: Option<String>) -> Result<()> {
    let s = &ctx.config.settings;
    let get = |k: &str| -> Option<String> {
        Some(match k {
            "claude-path" => s.claude_path.as_ref().map_or("auto".into(), |p| p.display().to_string()),
            "default" => s.default_profile.clone().unwrap_or_else(|| "none".into()),
            "link-routing" => format!("{:?}", s.link_routing).to_lowercase().replace("lastused", "last-used"),
            "check-updates" => s.check_updates.to_string(),
            "app-shortcuts" => s.app_shortcuts.to_string(),
            "desktop-shortcuts" => s.desktop_shortcuts.to_string(),
            "dock" => s.dock_pins.to_string(),
            "accent" => s.accent.clone(),
            "sort" => format!("{:?}", s.sort).to_lowercase(),
            "confirm-quit" => s.confirm_quit.to_string(),
            "global-args" => s.global_args.join(" "),
            "follow-updates" => s.follow_claude_updates.to_string(),
            "watcher" => s.background_watcher.to_string(),
            _ => return None,
        })
    };
    let Some(key) = key else {
        println!("# {}", ctx.paths.config_file.display());
        for (k, help) in CONFIG_KEYS {
            println!("{k:<18} {:<24} # {help}", get(k).unwrap_or_default());
        }
        return Ok(());
    };
    if key == "path" {
        println!("{}", ctx.paths.config_file.display());
        return Ok(());
    }
    let Some(value) = value else {
        println!("{}", get(&key).with_context(|| format!("unknown setting \"{key}\""))?);
        return Ok(());
    };
    let mut shortcuts_changed = false;
    let s = &mut ctx.config.settings;
    match key.as_str() {
        "claude-path" => {
            if value == "auto" {
                s.claude_path = None;
            } else {
                let p = PathBuf::from(&value);
                crate::claude::Install::from_path(&p).with_context(|| format!("there's no Claude at {value}"))?;
                s.claude_path = Some(p);
            }
        }
        "default" => {
            let chosen = if value == "none" {
                None
            } else {
                Some(
                    ctx.config
                        .find(&value)
                        .with_context(|| format!("there's no profile named \"{value}\""))?
                        .name
                        .clone(),
                )
            };
            ctx.config.settings.default_profile = chosen;
        }
        "link-routing" => {
            ctx.config.settings.link_routing = match value.as_str() {
                "ask" => LinkRouting::Ask,
                "last-used" | "last" => LinkRouting::LastUsed,
                "default" => LinkRouting::Default,
                _ => bail!("use ask, last-used or default"),
            }
        }
        "check-updates" => s.check_updates = on_off(&value)?,
        "app-shortcuts" => {
            s.app_shortcuts = on_off(&value)?;
            shortcuts_changed = true;
        }
        "desktop-shortcuts" => {
            s.desktop_shortcuts = on_off(&value)?;
            shortcuts_changed = true;
        }
        "dock" => {
            s.dock_pins = on_off(&value)?;
            shortcuts_changed = true;
        }
        "accent" => {
            parse_color(&value).context("use a color name or #RRGGBB")?;
            s.accent = value.to_lowercase();
        }
        "sort" => {
            s.sort = match value.as_str() {
                "manual" => SortOrder::Manual,
                "name" => SortOrder::Name,
                "recent" => SortOrder::Recent,
                _ => bail!("use manual, name or recent"),
            }
        }
        "confirm-quit" => s.confirm_quit = on_off(&value)?,
        "follow-updates" => s.follow_claude_updates = on_off(&value)?,
        "watcher" => {
            let on = on_off(&value)?;
            ops::set_watcher(ctx, on)?;
            println!("watcher = {}", if on { "on (starts at login)" } else { "off" });
            return Ok(());
        }
        "global-args" => {
            s.global_args =
                if value == "none" { Vec::new() } else { value.split_whitespace().map(str::to_owned).collect() }
        }
        _ => bail!("unknown setting \"{key}\". Run `{BIN} config` to see them all."),
    }
    ctx.save()?;
    if shortcuts_changed {
        for (n, e) in ops::sync_all_shortcuts(ctx) {
            eprintln!("note: shortcuts for {n}: {e:#}");
        }
    }
    let s = &ctx.config.settings;
    let shown = match key.as_str() {
        "claude-path" => s.claude_path.as_ref().map_or("auto".into(), |p| p.display().to_string()),
        "default" => s.default_profile.clone().unwrap_or_else(|| "none".into()),
        "link-routing" => s.link_routing.label().to_owned(),
        "accent" => s.accent.clone(),
        "sort" => s.sort.label().to_owned(),
        "global-args" if s.global_args.is_empty() => "none".into(),
        "global-args" => s.global_args.join(" "),
        _ => value.to_lowercase(),
    };
    println!("{key} = {shown}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn edit_accepts_hyphenated_args() {
        let cli = Cli::try_parse_from([BIN, "edit", "Work", "--arg", "--disable-gpu"]).expect("parse");
        assert!(matches!(cli.command, Some(Cmd::Edit { ref args, .. }) if args == &["--disable-gpu"]));
    }
}
