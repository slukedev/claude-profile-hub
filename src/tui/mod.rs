//! The interactive manager: a profile list with details, settings and health checks.
//!
//! Slow things (scanning processes, measuring folders, checking for updates) run on
//! background threads and report back over a channel, so keys are always answered at once.

mod view;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::claude::Install;
use crate::config::{LinkRouting, SortOrder, now};
use crate::ctx::{Ctx, DEFAULT_NAME, Target};
use crate::doctor::{self, Check};
use crate::launch::{self, Outcome};
use crate::process::{Running, dir_key};
use crate::profile::{Badge, COLORS, SYMBOLS, parse_color};
use crate::{ops, platform, update};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Profiles,
    Settings,
    Doctor,
}

impl Tab {
    pub const ALL: [Self; 3] = [Self::Profiles, Self::Settings, Self::Doctor];

    pub fn title(self) -> &'static str {
        match self {
            Self::Profiles => "Profiles",
            Self::Settings => "Settings",
            Self::Doctor => "Doctor",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Good,
    Bad,
}

/// What a text field is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputFor {
    NewProfile,
    Rename(String),
    Args(String),
    Env(String),
    Notes(String),
    Letter(String),
    HexColor(String),
    ClaudePath,
    GlobalArgs,
    Filter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmFor {
    Remove { name: String, purge: bool },
    Fresh(String),
    Stop(String),
    StopAll,
    Update(String),
}

/// Fields offered by the edit menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Color,
    Badge,
    Args,
    Env,
    Notes,
    OwnApp,
    Default,
}

impl Field {
    pub fn all() -> Vec<Self> {
        let mut v = vec![Self::Name, Self::Color, Self::Badge, Self::Args, Self::Env, Self::Notes, Self::Default];
        if cfg!(target_os = "macos") {
            v.push(Self::OwnApp);
        }
        v
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Color => "Color",
            Self::Badge => "Badge",
            Self::Args => "Extra flags for Claude",
            Self::Env => "Environment variables",
            Self::Notes => "Notes",
            Self::OwnApp => "Own Dock icon while running",
            Self::Default => "Make this the default profile",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    Input { title: String, hint: String, value: String, cursor: usize, purpose: InputFor },
    Confirm { title: String, body: String, yes: String, purpose: ConfirmFor },
    Edit { name: String, sel: usize },
    Colors { name: String, sel: usize },
    Badges { name: String, sel: usize },
    Help,
}

/// Rows of the Settings tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    ClaudePath,
    DefaultProfile,
    RouteLinks,
    LinkRouting,
    AppShortcuts,
    DesktopShortcuts,
    DockPins,
    Sort,
    Accent,
    ConfirmQuit,
    GlobalArgs,
    CheckUpdates,
    ConfigFile,
}

impl Setting {
    pub fn all() -> Vec<Self> {
        let mut v = vec![
            Self::ClaudePath,
            Self::DefaultProfile,
            Self::RouteLinks,
            Self::LinkRouting,
            Self::AppShortcuts,
            Self::DesktopShortcuts,
        ];
        if cfg!(target_os = "macos") {
            v.push(Self::DockPins);
        }
        v.extend([Self::Sort, Self::Accent, Self::ConfirmQuit, Self::GlobalArgs, Self::CheckUpdates, Self::ConfigFile]);
        v
    }
}

enum Msg {
    Running(Running),
    Size(String, u64),
    Latest(Option<String>),
}

pub struct App {
    pub ctx: Ctx,
    pub claude: Option<Install>,
    pub tab: Tab,
    pub sel: usize,
    pub settings_sel: usize,
    pub doctor_sel: usize,
    pub running: Running,
    pub sizes: HashMap<String, u64>,
    pub status: Option<(String, Tone, Instant)>,
    pub modal: Option<Modal>,
    pub filter: String,
    pub update: Option<String>,
    pub doctor: Vec<Check>,
    pub quit: bool,
    size_tx: Option<Sender<PathBuf>>,
}

impl App {
    pub fn new(ctx: Ctx) -> Self {
        let claude = ctx.claude();
        let update = ctx.state.latest_version.clone().filter(|v| update::is_newer(v, crate::VERSION));
        Self {
            ctx,
            claude,
            tab: Tab::Profiles,
            sel: 0,
            settings_sel: 0,
            doctor_sel: 0,
            running: Running::default(),
            sizes: HashMap::new(),
            status: None,
            modal: None,
            filter: String::new(),
            update,
            doctor: Vec::new(),
            quit: false,
            size_tx: None,
        }
    }

    /// Names in display order, Claude's own profile first, narrowed by the filter.
    pub fn rows(&self) -> Vec<Target<'_>> {
        let f = self.filter.to_lowercase();
        self.ctx.targets().into_iter().filter(|t| f.is_empty() || t.name().to_lowercase().contains(&f)).collect()
    }

    pub fn selected_name(&self) -> Option<String> {
        self.rows().get(self.sel).map(|t| t.name().to_owned())
    }

    fn selected_profile_name(&self) -> Option<String> {
        match self.rows().get(self.sel) {
            Some(Target::Profile(p)) => Some(p.name.clone()),
            _ => None,
        }
    }

    pub fn say(&mut self, text: impl Into<String>, tone: Tone) {
        self.status = Some((text.into(), tone, Instant::now()));
    }

    fn report<T>(&mut self, r: Result<T>, ok: impl FnOnce(T) -> String) {
        match r {
            Ok(v) => {
                let msg = ok(v);
                self.say(msg, Tone::Good);
            }
            Err(e) => self.say(format!("{e:#}"), Tone::Bad),
        }
    }

    fn select_name(&mut self, name: &str) {
        if let Some(i) = self.rows().iter().position(|t| t.name().eq_ignore_ascii_case(name)) {
            self.sel = i;
        }
    }

    fn request_sizes(&self) {
        if let Some(tx) = &self.size_tx {
            for p in &self.ctx.config.profiles {
                let _ = tx.send(p.data_dir.clone());
            }
        }
    }

    fn rescan(&mut self) {
        self.running = Running::scan(self.claude.as_ref().map(|c| c.exe.as_path()));
    }

    fn clamp(&mut self) {
        let n = self.rows().len();
        if self.sel >= n {
            self.sel = n.saturating_sub(1);
        }
    }

    fn input(&mut self, title: &str, hint: &str, value: &str, purpose: InputFor) {
        self.modal = Some(Modal::Input {
            title: title.to_owned(),
            hint: hint.to_owned(),
            value: value.to_owned(),
            cursor: value.chars().count(),
            purpose,
        });
    }

    fn confirm(&mut self, title: &str, body: String, yes: &str, purpose: ConfirmFor) {
        self.modal = Some(Modal::Confirm { title: title.to_owned(), body, yes: yes.to_owned(), purpose });
    }

    // ---- keys -------------------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if let Some(modal) = self.modal.take() {
            self.modal_key(modal, key);
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc if self.filter.is_empty() || key.code == KeyCode::Char('q') => {
                self.quit = true;
            }
            KeyCode::Esc => {
                self.filter.clear();
                self.clamp();
            }
            KeyCode::Tab => self.switch_tab(1),
            KeyCode::BackTab => self.switch_tab(Tab::ALL.len() - 1),
            KeyCode::Char('1') => self.set_tab(Tab::Profiles),
            KeyCode::Char('2') => self.set_tab(Tab::Settings),
            KeyCode::Char('3') => self.set_tab(Tab::Doctor),
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            KeyCode::Char('U') if self.update.is_some() => self.ask_update(),
            _ => match self.tab {
                Tab::Profiles => self.profiles_key(key),
                Tab::Settings => self.settings_key(key),
                Tab::Doctor => self.doctor_key(key),
            },
        }
    }

    fn switch_tab(&mut self, by: usize) {
        let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
        self.set_tab(Tab::ALL[(i + by) % Tab::ALL.len()]);
    }

    fn set_tab(&mut self, tab: Tab) {
        self.tab = tab;
        if tab == Tab::Doctor {
            self.doctor = doctor::run(&self.ctx);
        }
    }

    fn ask_update(&mut self) {
        let Some(v) = self.update.clone() else { return };
        let method = update::InstallMethod::detect(&update::current_exe());
        if let Some(cmd) = method.update_command() {
            self.say(format!("{v} is out. Update with: {cmd}"), Tone::Info);
        } else {
            self.confirm(
                "Update",
                format!("Download and install {} {v}? You have {}.", crate::NAME, crate::VERSION),
                "Update",
                ConfirmFor::Update(v),
            );
        }
    }

    fn move_sel(sel: &mut usize, len: usize, down: bool) {
        if len == 0 {
            return;
        }
        *sel = if down { (*sel + 1) % len } else { (*sel + len - 1) % len };
    }

    fn profiles_key(&mut self, key: KeyEvent) {
        let len = self.rows().len();
        let name = self.selected_name();
        let profile = self.selected_profile_name();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => Self::move_sel(&mut self.sel, len, true),
            KeyCode::Up | KeyCode::Char('k') => Self::move_sel(&mut self.sel, len, false),
            KeyCode::Home | KeyCode::Char('g') => self.sel = 0,
            KeyCode::End | KeyCode::Char('G') => self.sel = len.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char('o') => {
                if let Some(n) = name {
                    self.open(&n);
                }
            }
            KeyCode::Char('n') | KeyCode::Char('a') => {
                self.input("New profile", "A name like Work, Personal or a client's name", "", InputFor::NewProfile);
            }
            KeyCode::Char('/') => {
                let f = self.filter.clone();
                self.input("Filter", "Type to narrow the list", &f, InputFor::Filter);
            }
            KeyCode::Char('x') => {
                if let Some(n) = name {
                    self.ask_stop(&n);
                }
            }
            KeyCode::Char('X') => {
                if self.running.count() > 0 {
                    self.confirm(
                        "Quit all",
                        "Quit every running Claude profile?".into(),
                        "Quit all",
                        ConfirmFor::StopAll,
                    );
                }
            }
            KeyCode::Char('O') => {
                let dir = name.as_deref().and_then(|n| self.ctx.config.find(n)).map(|p| p.data_dir.clone());
                match dir {
                    Some(d) if d.exists() => {
                        let r = platform::open_path(&d);
                        self.report(r, |()| "Opened the profile's folder.".into());
                    }
                    Some(_) => self.say("It hasn't been opened yet, so there's no folder.", Tone::Info),
                    None => self.say("Claude's own profile lives where Claude keeps it.", Tone::Info),
                }
            }
            KeyCode::Char('R') => {
                self.rescan();
                self.request_sizes();
                self.say("Refreshed.", Tone::Info);
            }
            _ => {
                let Some(p) = profile else {
                    if matches!(key.code, KeyCode::Char('e' | 'r' | 'c' | 'b' | 'd' | 'D' | 'f' | 's')) {
                        self.say("That's Claude's own profile. Add one with n to customize it.", Tone::Info);
                    }
                    return;
                };
                self.profile_key(&p, key);
            }
        }
    }

    fn profile_key(&mut self, p: &str, key: KeyEvent) {
        match key.code {
            KeyCode::Char('e') => self.modal = Some(Modal::Edit { name: p.to_owned(), sel: 0 }),
            KeyCode::Char('r') => self.input("Rename", "Its sign-in and data stay", p, InputFor::Rename(p.to_owned())),
            KeyCode::Char('c') => self.open_colors(p),
            KeyCode::Char('b') => self.open_badges(p),
            KeyCode::Char('d') => self.confirm(
                "Remove profile",
                format!("Remove {p} and its shortcuts? Its data stays on disk, so adding it back with the same folder keeps the sign-in."),
                "Remove",
                ConfirmFor::Remove { name: p.to_owned(), purge: false },
            ),
            KeyCode::Char('D') => self.confirm(
                "Remove profile and data",
                format!("Remove {p} and move its data (sign-in, settings, cache) to the trash?"),
                "Remove and trash data",
                ConfirmFor::Remove { name: p.to_owned(), purge: true },
            ),
            KeyCode::Char('f') => self.confirm(
                "Sign out",
                format!("Sign {p} out by moving its data to the trash? Open it afterwards to sign in again."),
                "Sign out",
                ConfirmFor::Fresh(p.to_owned()),
            ),
            KeyCode::Char('s') => {
                let r = ops::sync_shortcuts(&self.ctx, p);
                self.report(r, |made| match made.len() {
                    0 => "Shortcuts are turned off in Settings.".into(),
                    n => format!("Rebuilt {n} shortcut(s)."),
                });
            }
            KeyCode::Char('*') => self.toggle_default(p),
            KeyCode::Char('J') | KeyCode::Char('K') => self.reorder(p, key.code == KeyCode::Char('J')),
            _ => {}
        }
    }

    fn toggle_default(&mut self, p: &str) {
        let s = &mut self.ctx.config.settings;
        let now_default = s.default_profile.as_deref() != Some(p);
        s.default_profile = now_default.then(|| p.to_owned());
        let r = self.ctx.save();
        self.report(
            r,
            |()| if now_default { format!("{p} is the default profile.") } else { "No default profile.".into() },
        );
    }

    fn reorder(&mut self, p: &str, down: bool) {
        if self.ctx.config.settings.sort != SortOrder::Manual {
            self.say("Switch Sort to Manual in Settings to reorder.", Tone::Info);
            return;
        }
        let Some(i) = self.ctx.config.position(p) else { return };
        let j = if down { i + 1 } else { i.wrapping_sub(1) };
        if j < self.ctx.config.profiles.len() {
            self.ctx.config.profiles.swap(i, j);
            let r = self.ctx.save();
            self.report(r, |()| String::new());
            self.status = None;
            self.select_name(p);
        }
    }

    fn open(&mut self, name: &str) {
        let r = launch::launch(&mut self.ctx, name, None);
        self.report(r, |o| match o {
            Outcome::Started => format!("Opened {name}."),
            Outcome::Focused => format!("{name} was open; brought it to the front."),
        });
        self.rescan();
    }

    fn ask_stop(&mut self, name: &str) {
        let dir = match self.ctx.target(name) {
            Ok(t) => t.data_dir().map(PathBuf::from),
            Err(_) => return,
        };
        if self.running.pid_for_dir(dir.as_deref()).is_none() {
            self.say(format!("{name} isn't running."), Tone::Info);
        } else if self.ctx.config.settings.confirm_quit {
            self.confirm("Quit", format!("Quit {name}?"), "Quit", ConfirmFor::Stop(name.to_owned()));
        } else {
            self.stop(name);
        }
    }

    fn stop(&mut self, name: &str) {
        let r = launch::stop(&self.ctx, name);
        self.report(r, |ok| if ok { format!("Asked {name} to quit.") } else { format!("{name} wasn't running.") });
    }

    fn open_colors(&mut self, p: &str) {
        let current = self.ctx.config.find(p).map(|x| x.color.clone()).unwrap_or_default();
        let sel = COLORS.iter().position(|(n, _)| *n == current).unwrap_or(0);
        self.modal = Some(Modal::Colors { name: p.to_owned(), sel });
    }

    fn open_badges(&mut self, p: &str) {
        let current = self.ctx.config.find(p).map(|x| x.badge()).unwrap_or(Badge::None);
        let sel = match current {
            Badge::Symbol(s) => SYMBOLS.iter().position(|x| *x == s).unwrap_or(0),
            Badge::Letter(_) => SYMBOLS.len(),
            Badge::None => SYMBOLS.len() + 1,
        };
        self.modal = Some(Modal::Badges { name: p.to_owned(), sel });
    }

    fn set_look(&mut self, p: &str, color: Option<String>, badge: Option<String>) {
        if let Some(prof) = self.ctx.config.find_mut(p) {
            if let Some(c) = color {
                prof.color = c;
            }
            if let Some(b) = badge {
                prof.badge = b;
            }
        }
        let r = self.ctx.save().and_then(|()| ops::sync_shortcuts(&self.ctx, p));
        self.report(r, |_| format!("Updated {p}'s icon."));
    }

    fn settings_key(&mut self, key: KeyEvent) {
        let items = Setting::all();
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => Self::move_sel(&mut self.settings_sel, items.len(), true),
            KeyCode::Up | KeyCode::Char('k') => Self::move_sel(&mut self.settings_sel, items.len(), false),
            KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(s) = items.get(self.settings_sel) {
                    self.change_setting(*s, true);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(s) = items.get(self.settings_sel) {
                    self.change_setting(*s, false);
                }
            }
            _ => {}
        }
    }

    fn cycle<T: Copy + PartialEq>(all: &[T], cur: T, forward: bool) -> T {
        let i = all.iter().position(|x| *x == cur).unwrap_or(0);
        let n = all.len();
        all[if forward { (i + 1) % n } else { (i + n - 1) % n }]
    }

    fn change_setting(&mut self, s: Setting, forward: bool) {
        let mut sync_shortcuts = false;
        let settings = &mut self.ctx.config.settings;
        match s {
            Setting::ClaudePath => {
                let cur = settings.claude_path.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
                self.input(
                    "Claude Desktop location",
                    "Path to Claude.app or Claude's program. Leave empty to find it automatically.",
                    &cur,
                    InputFor::ClaudePath,
                );
                return;
            }
            Setting::DefaultProfile => {
                let mut options: Vec<Option<String>> = vec![None];
                options.extend(self.ctx.config.profiles.iter().map(|p| Some(p.name.clone())));
                let i = options.iter().position(|o| *o == settings.default_profile).unwrap_or(0);
                let n = options.len();
                settings.default_profile = options[if forward { (i + 1) % n } else { (i + n - 1) % n }].clone();
            }
            Setting::RouteLinks => {
                let on = !settings.route_links;
                let r = ops::set_route_links(&mut self.ctx, on);
                self.report(r, |()| {
                    if on {
                        "Sign-in links now ask which profile they're for.".into()
                    } else {
                        "Sign-in links go straight to Claude.".into()
                    }
                });
                return;
            }
            Setting::LinkRouting => {
                settings.link_routing = Self::cycle(&LinkRouting::ALL, settings.link_routing, forward)
            }
            Setting::AppShortcuts => {
                settings.app_shortcuts = !settings.app_shortcuts;
                sync_shortcuts = true;
            }
            Setting::DesktopShortcuts => {
                settings.desktop_shortcuts = !settings.desktop_shortcuts;
                sync_shortcuts = true;
            }
            Setting::DockPins => {
                settings.dock_pins = !settings.dock_pins;
                sync_shortcuts = true;
            }
            Setting::Sort => settings.sort = Self::cycle(&SortOrder::ALL, settings.sort, forward),
            Setting::Accent => {
                let names: Vec<&str> = COLORS.iter().map(|(n, _)| *n).collect();
                let i = names.iter().position(|n| *n == settings.accent).unwrap_or(0);
                let n = names.len();
                settings.accent = names[if forward { (i + 1) % n } else { (i + n - 1) % n }].to_owned();
            }
            Setting::ConfirmQuit => settings.confirm_quit = !settings.confirm_quit,
            Setting::GlobalArgs => {
                let cur = settings.global_args.join(" ");
                self.input(
                    "Flags for every profile",
                    "Passed to Claude for every profile, separated by spaces",
                    &cur,
                    InputFor::GlobalArgs,
                );
                return;
            }
            Setting::CheckUpdates => settings.check_updates = !settings.check_updates,
            Setting::ConfigFile => {
                let r = platform::open_path(&self.ctx.paths.config_file);
                self.report(r, |()| "Opened the config file. Reopen the manager after editing it.".into());
                return;
            }
        }
        let r = self.ctx.save();
        self.report(r, |()| "Saved.".into());
        if sync_shortcuts {
            let failures = ops::sync_all_shortcuts(&self.ctx);
            if let Some((n, e)) = failures.first() {
                self.say(format!("Saved, but {n}'s shortcuts failed: {e:#}"), Tone::Bad);
            } else {
                self.say("Saved and updated shortcuts.", Tone::Good);
            }
        }
    }

    fn doctor_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => Self::move_sel(&mut self.doctor_sel, self.doctor.len(), true),
            KeyCode::Up | KeyCode::Char('k') => Self::move_sel(&mut self.doctor_sel, self.doctor.len(), false),
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.doctor = doctor::run(&self.ctx);
                self.say("Checked again.", Tone::Info);
            }
            _ => {}
        }
    }

    // ---- modals -----------------------------------------------------------------------

    fn modal_key(&mut self, modal: Modal, key: KeyEvent) {
        match modal {
            Modal::Help => {}
            Modal::Input { title, hint, mut value, mut cursor, purpose } => match key.code {
                KeyCode::Esc => {
                    if purpose == InputFor::Filter {
                        self.filter.clear();
                        self.clamp();
                    }
                }
                KeyCode::Enter => self.submit(purpose, value.trim()),
                code => {
                    edit_text(&mut value, &mut cursor, code, key.modifiers);
                    if purpose == InputFor::Filter {
                        self.filter = value.clone();
                        self.sel = 0;
                    }
                    self.modal = Some(Modal::Input { title, hint, value, cursor, purpose });
                }
            },
            Modal::Confirm { purpose, title, body, yes } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => self.confirmed(purpose),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {}
                _ => self.modal = Some(Modal::Confirm { purpose, title, body, yes }),
            },
            Modal::Edit { name, mut sel } => {
                let fields = Field::all();
                match key.code {
                    KeyCode::Esc => {}
                    KeyCode::Down | KeyCode::Char('j') => {
                        Self::move_sel(&mut sel, fields.len(), true);
                        self.modal = Some(Modal::Edit { name, sel });
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        Self::move_sel(&mut sel, fields.len(), false);
                        self.modal = Some(Modal::Edit { name, sel });
                    }
                    KeyCode::Enter => self.edit_field(&name, fields[sel]),
                    _ => self.modal = Some(Modal::Edit { name, sel }),
                }
            }
            Modal::Colors { name, mut sel } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Right | KeyCode::Char('l') => {
                    Self::move_sel(&mut sel, COLORS.len(), true);
                    self.modal = Some(Modal::Colors { name, sel });
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::Left | KeyCode::Char('h') => {
                    Self::move_sel(&mut sel, COLORS.len(), false);
                    self.modal = Some(Modal::Colors { name, sel });
                }
                KeyCode::Char('#') => {
                    self.input("Custom color", "A hex color like #3366ff", "#", InputFor::HexColor(name))
                }
                KeyCode::Enter => self.set_look(&name, Some(COLORS[sel].0.to_owned()), None),
                _ => self.modal = Some(Modal::Colors { name, sel }),
            },
            Modal::Badges { name, mut sel } => {
                let len = SYMBOLS.len() + 2;
                match key.code {
                    KeyCode::Esc => {}
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Right | KeyCode::Char('l') => {
                        Self::move_sel(&mut sel, len, true);
                        self.modal = Some(Modal::Badges { name, sel });
                    }
                    KeyCode::Up | KeyCode::Char('k') | KeyCode::Left | KeyCode::Char('h') => {
                        Self::move_sel(&mut sel, len, false);
                        self.modal = Some(Modal::Badges { name, sel });
                    }
                    KeyCode::Enter if sel < SYMBOLS.len() => self.set_look(&name, None, Some(SYMBOLS[sel].to_owned())),
                    KeyCode::Enter if sel == SYMBOLS.len() => {
                        self.input("Letter badge", "One letter or digit", "", InputFor::Letter(name));
                    }
                    KeyCode::Enter => self.set_look(&name, None, Some("none".into())),
                    _ => self.modal = Some(Modal::Badges { name, sel }),
                }
            }
        }
    }

    fn edit_field(&mut self, name: &str, field: Field) {
        let Some(p) = self.ctx.config.find(name).cloned() else { return };
        match field {
            Field::Name => self.input("Rename", "Its sign-in and data stay", &p.name, InputFor::Rename(p.name.clone())),
            Field::Color => self.open_colors(name),
            Field::Badge => self.open_badges(name),
            Field::Args => self.input(
                "Extra flags",
                "Passed to Claude for this profile, separated by spaces",
                &p.extra_args.join(" "),
                InputFor::Args(p.name.clone()),
            ),
            Field::Env => {
                let cur: Vec<String> = p.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
                self.input(
                    "Environment",
                    "KEY=VALUE pairs separated by spaces",
                    &cur.join(" "),
                    InputFor::Env(p.name.clone()),
                );
            }
            Field::Notes => self.input(
                "Notes",
                "Anything you want to remember about this profile",
                &p.notes,
                InputFor::Notes(p.name.clone()),
            ),
            Field::OwnApp => {
                if let Some(prof) = self.ctx.config.find_mut(name) {
                    prof.own_app = !prof.own_app;
                }
                let on = !p.own_app;
                let r = self.ctx.save();
                self.report(r, |()| {
                    if on {
                        format!("{name} will run from its own copy of Claude, with its own Dock icon. Takes effect next time it opens.")
                    } else {
                        format!("{name} will run from Claude.app.")
                    }
                });
            }
            Field::Default => self.toggle_default(name),
        }
    }

    fn submit(&mut self, purpose: InputFor, value: &str) {
        match purpose {
            InputFor::Filter => {}
            InputFor::NewProfile => {
                if value.is_empty() {
                    return;
                }
                let r = ops::add(&mut self.ctx, value, None, None, None);
                if let Ok(n) = &r {
                    let n = n.clone();
                    self.select_name(&n);
                    self.request_sizes();
                }
                self.report(r, |n| format!("Added {n}. Press Enter to open it and sign in."));
            }
            InputFor::Rename(old) => {
                if value.is_empty() || value == old {
                    return;
                }
                let r = ops::rename(&mut self.ctx, &old, value);
                if let Ok(n) = &r {
                    let n = n.clone();
                    self.select_name(&n);
                }
                self.report(r, |n| format!("Renamed to {n}."));
            }
            InputFor::Args(n) => {
                if let Some(p) = self.ctx.config.find_mut(&n) {
                    p.extra_args = value.split_whitespace().map(str::to_owned).collect();
                }
                let r = self.ctx.save();
                self.report(r, |()| "Saved. Takes effect next time it opens.".into());
            }
            InputFor::Env(n) => {
                let mut env = std::collections::BTreeMap::new();
                for pair in value.split_whitespace() {
                    match pair.split_once('=') {
                        Some((k, v)) if !k.is_empty() => {
                            env.insert(k.to_owned(), v.to_owned());
                        }
                        _ => {
                            self.say(format!("\"{pair}\" should look like KEY=VALUE"), Tone::Bad);
                            return;
                        }
                    }
                }
                if let Some(p) = self.ctx.config.find_mut(&n) {
                    p.env = env;
                }
                let r = self.ctx.save();
                self.report(r, |()| "Saved. Takes effect next time it opens.".into());
            }
            InputFor::Notes(n) => {
                if let Some(p) = self.ctx.config.find_mut(&n) {
                    p.notes = value.to_owned();
                }
                let r = self.ctx.save();
                self.report(r, |()| "Saved.".into());
            }
            InputFor::Letter(n) => match Badge::parse(value) {
                Some(b @ Badge::Letter(_)) => self.set_look(&n, None, Some(b.as_config())),
                _ => self.say("Use one letter or digit.", Tone::Bad),
            },
            InputFor::HexColor(n) => {
                if parse_color(value).is_some() {
                    self.set_look(&n, Some(value.to_lowercase()), None);
                } else {
                    self.say("Use a hex color like #3366ff.", Tone::Bad);
                }
            }
            InputFor::ClaudePath => {
                if value.is_empty() {
                    self.ctx.config.settings.claude_path = None;
                } else {
                    let p = PathBuf::from(value);
                    if Install::from_path(&p).is_none() {
                        self.say(format!("There's no Claude at {value}."), Tone::Bad);
                        return;
                    }
                    self.ctx.config.settings.claude_path = Some(p);
                }
                self.claude = self.ctx.claude();
                let r = self.ctx.save();
                self.report(r, |()| "Saved.".into());
            }
            InputFor::GlobalArgs => {
                self.ctx.config.settings.global_args = value.split_whitespace().map(str::to_owned).collect();
                let r = self.ctx.save();
                self.report(r, |()| "Saved.".into());
            }
        }
    }

    fn confirmed(&mut self, purpose: ConfirmFor) {
        match purpose {
            ConfirmFor::Remove { name, purge } => {
                let r = ops::remove(&mut self.ctx, &name, purge);
                self.clamp();
                self.report(r, |_| {
                    if purge {
                        format!("Removed {name} and trashed its data.")
                    } else {
                        format!("Removed {name}. Its data is still on disk.")
                    }
                });
            }
            ConfirmFor::Fresh(name) => {
                let r = launch::fresh_start(&self.ctx, &name);
                if r.is_ok() {
                    self.sizes.remove(&self.ctx.config.find(&name).map(|p| dir_key(&p.data_dir)).unwrap_or_default());
                }
                self.report(r, |()| format!("{name} is signed out. Open it to sign in again."));
            }
            ConfirmFor::Stop(name) => self.stop(&name),
            ConfirmFor::StopAll => {
                let names: Vec<String> = self.ctx.targets().iter().map(|t| t.name().to_owned()).collect();
                let mut n = 0;
                for name in names {
                    if launch::stop(&self.ctx, &name).unwrap_or(false) {
                        n += 1;
                    }
                }
                self.say(format!("Asked {n} profile(s) to quit."), Tone::Good);
            }
            ConfirmFor::Update(v) => {
                let r = update::self_update(false);
                self.report(r, |u| match u {
                    update::Updated::To(v) => format!("Updated to {v}. Restart the manager to use it."),
                    update::Updated::AlreadyCurrent(_) => format!("Already on the newest version ({v})."),
                    update::Updated::UseCommand(c) => format!("Update with: {c}"),
                });
                self.update = None;
            }
        }
    }

    fn on_msg(&mut self, msg: Msg) {
        match msg {
            Msg::Running(r) => self.running = r,
            Msg::Size(k, s) => {
                self.sizes.insert(k, s);
            }
            Msg::Latest(v) => {
                self.ctx.state.last_update_check = now();
                if v.is_some() {
                    self.ctx.state.latest_version.clone_from(&v);
                }
                let _ = self.ctx.save_state();
                self.update = v.filter(|v| update::is_newer(v, crate::VERSION));
            }
        }
    }
}

/// Basic line editing: typing, Backspace/Delete, arrows, Home/End, Ctrl+U and Ctrl+W.
pub fn edit_text(value: &mut String, cursor: &mut usize, code: KeyCode, mods: KeyModifiers) {
    let byte = |s: &str, i: usize| s.char_indices().nth(i).map_or(s.len(), |(b, _)| b);
    let len = value.chars().count();
    match code {
        KeyCode::Char('u') if mods.contains(KeyModifiers::CONTROL) => {
            value.replace_range(..byte(value, *cursor), "");
            *cursor = 0;
        }
        KeyCode::Char('w') if mods.contains(KeyModifiers::CONTROL) => {
            let before: Vec<char> = value.chars().take(*cursor).collect();
            let mut start = before.len();
            while start > 0 && before[start - 1] == ' ' {
                start -= 1;
            }
            while start > 0 && before[start - 1] != ' ' {
                start -= 1;
            }
            value.replace_range(byte(value, start)..byte(value, *cursor), "");
            *cursor = start;
        }
        KeyCode::Char(c) if !mods.contains(KeyModifiers::CONTROL) => {
            let at = byte(value, *cursor);
            value.insert(at, c);
            *cursor += 1;
        }
        KeyCode::Backspace if *cursor > 0 => {
            let at = byte(value, *cursor - 1);
            value.remove(at);
            *cursor -= 1;
        }
        KeyCode::Delete if *cursor < len => {
            let at = byte(value, *cursor);
            value.remove(at);
        }
        KeyCode::Left => *cursor = cursor.saturating_sub(1),
        KeyCode::Right => *cursor = (*cursor + 1).min(len),
        KeyCode::Home => *cursor = 0,
        KeyCode::End => *cursor = len,
        _ => {}
    }
}

/// Background work: a process scanner, a folder-size worker and a one-off update check.
/// Each thread stops when `stop` is set or the UI drops its receiver.
fn spawn_workers(app: &mut App, tx: &Sender<Msg>, stop: &Arc<AtomicBool>) {
    let exe = app.claude.as_ref().map(|c| c.exe.clone());
    {
        let (tx, stop) = (tx.clone(), Arc::clone(stop));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if tx.send(Msg::Running(Running::scan(exe.as_deref()))).is_err() {
                    break;
                }
                for _ in 0..15 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
    }
    let (size_tx, size_rx) = mpsc::channel::<PathBuf>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for dir in size_rx {
                if tx.send(Msg::Size(dir_key(&dir), launch::dir_size(&dir))).is_err() {
                    break;
                }
            }
        });
    }
    app.size_tx = Some(size_tx);
    app.request_sizes();

    let settings = &app.ctx.config.settings;
    let due = now() - app.ctx.state.last_update_check > 24 * 60 * 60;
    if settings.check_updates && due && std::env::var_os("CLAUDE_PROFILE_HUB_NO_UPDATE_CHECK").is_none() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let v = update::fetch_latest(Duration::from_secs(6)).ok().map(|r| r.version);
            let _ = tx.send(Msg::Latest(v));
        });
    }
}

pub fn run(ctx: Ctx) -> Result<()> {
    let mut app = App::new(ctx);
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    spawn_workers(&mut app, &tx, &stop);
    if app.claude.is_none() {
        app.say("Couldn't find Claude Desktop. Set its location in Settings (2).", Tone::Bad);
    }

    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        while !app.quit {
            terminal.draw(|f| view::draw(f, &app))?;
            if event::poll(Duration::from_millis(200))? {
                if let Event::Key(key) = event::read()? {
                    app.on_key(key);
                }
            }
            while let Ok(msg) = rx.try_recv() {
                app.on_msg(msg);
            }
            if app.status.as_ref().is_some_and(|(_, _, at)| at.elapsed() > Duration::from_secs(8)) {
                app.status = None;
            }
        }
        Ok(())
    })();
    stop.store(true, Ordering::Relaxed);
    ratatui::restore();
    if let Some(last) = &app.ctx.state.last_profile {
        if last != DEFAULT_NAME && app.ctx.config.find(last).is_none() {
            app.ctx.state.last_profile = None;
            let _ = app.ctx.save_state();
        }
    }
    result
}

#[cfg(test)]
mod tests;
