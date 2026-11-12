//! Drawing the TUI. Pure: reads [`App`], never changes it.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::{App, Field, Modal, Setting, Tab, Tone};
use crate::ctx::Target;
use crate::doctor::Level;
use crate::ops::{ago, human_size};
use crate::process::dir_key;
use crate::profile::{Badge, COLORS, SYMBOLS, parse_color};

/// Whether the terminal shows 24-bit color. Terminal.app and some others don't, and
/// garble true-color codes, so those get the nearest 256-color instead.
fn truecolor() -> bool {
    std::env::var("COLORTERM").is_ok_and(|v| v.contains("truecolor") || v.contains("24bit"))
        || std::env::var("TERM_PROGRAM")
            .is_ok_and(|v| matches!(v.as_str(), "iTerm.app" | "WezTerm" | "ghostty" | "vscode"))
        || std::env::var_os("WT_SESSION").is_some()
}

pub fn rgb(c: [u8; 3]) -> Color {
    if truecolor() {
        return Color::Rgb(c[0], c[1], c[2]);
    }
    // Nearest color in xterm's 6x6x6 cube.
    let q = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            (v - 35) / 40
        }
    };
    Color::Indexed(16 + 36 * q(c[0]) + 6 * q(c[1]) + q(c[2]))
}

struct Theme {
    accent: Color,
    dim: Style,
    good: Color,
    bad: Color,
    warn: Color,
}

fn theme(app: &App) -> Theme {
    Theme {
        accent: rgb(parse_color(&app.ctx.config.settings.accent).unwrap_or([245, 140, 71])),
        dim: Style::new().fg(Color::DarkGray),
        good: Color::Green,
        bad: Color::Red,
        warn: Color::Yellow,
    }
}

/// A one-cell stand-in for a badge symbol. All are text (not emoji) characters, so they
/// take one column in every terminal.
pub fn symbol_glyph(name: &str) -> char {
    match name {
        "work" => '▤',
        "personal" => '☺',
        "home" => '⌂',
        "code" => '‹',
        "star" => '★',
        "heart" => '♥',
        "book" => '¶',
        "globe" => '◎',
        "bolt" => 'ϟ',
        "leaf" => '♣',
        "moon" => '☾',
        "chat" => '✉',
        _ => '•',
    }
}

fn badge_text(b: &Badge) -> String {
    match b {
        Badge::Letter(c) => c.to_string(),
        Badge::Symbol(s) => symbol_glyph(s).to_string(),
        Badge::None => " ".into(),
    }
}

/// A path for display: home shortened to ~, and the middle cut out if it's too long.
fn short_path(path: &std::path::Path, max: usize) -> String {
    let mut s = path.display().to_string();
    if let Some(home) = crate::claude::home() {
        let h = home.display().to_string();
        if let Some(rest) = s.strip_prefix(&h) {
            s = format!("~{rest}");
        }
    }
    let n = s.chars().count();
    if n <= max || max < 8 {
        return s;
    }
    let keep = max - 1;
    let head: String = s.chars().take(keep / 3).collect();
    let tail: String = s.chars().skip(n - (keep - keep / 3)).collect();
    format!("{head}…{tail}")
}

/// A small colored tile standing in for the profile's icon.
fn chip(target: &Target<'_>) -> Span<'static> {
    match target {
        Target::Default => {
            Span::styled(" C ", Style::new().fg(Color::White).bg(rgb([217, 119, 87])).add_modifier(Modifier::BOLD))
        }
        Target::Profile(p) => Span::styled(
            format!(" {} ", badge_text(&p.badge())),
            Style::new().fg(Color::White).bg(rgb(p.rgb())).add_modifier(Modifier::BOLD),
        ),
    }
}

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let t = theme(app);
    let [header, body, status, keys] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(5), Constraint::Length(1), Constraint::Length(1)])
            .areas(f.area());
    draw_header(f, app, &t, header);
    match app.tab {
        Tab::Profiles => draw_profiles(f, app, &t, body),
        Tab::Settings => draw_settings(f, app, &t, body),
        Tab::Doctor => draw_doctor(f, app, &t, body),
    }
    draw_status(f, app, &t, status);
    draw_keys(f, app, &t, keys);
    if let Some(m) = &app.modal {
        draw_modal(f, app, &t, m);
    }
}

fn draw_header(f: &mut Frame<'_>, app: &App, t: &Theme, area: Rect) {
    let mut spans = vec![
        Span::styled(format!(" {} ", crate::NAME), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::raw("  "),
    ];
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let style = if *tab == app.tab {
            Style::new().fg(Color::Black).bg(t.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new()
        };
        spans.push(Span::styled(format!(" {} {} ", i + 1, tab.title()), style));
        spans.push(Span::raw(" "));
    }
    let [left, right] = Layout::horizontal([Constraint::Min(10), Constraint::Length(34)]).areas(area);
    f.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::new().borders(Borders::BOTTOM).border_style(t.dim)),
        left,
    );
    let right_line = match &app.update {
        Some(v) => Line::from(vec![
            Span::styled(format!("{v} available"), Style::new().fg(t.good)),
            Span::styled(" (U) ", t.dim),
        ]),
        None => Line::from(Span::styled(format!("v{} ", crate::VERSION), t.dim)),
    };
    f.render_widget(
        Paragraph::new(right_line)
            .alignment(Alignment::Right)
            .block(Block::new().borders(Borders::BOTTOM).border_style(t.dim)),
        right,
    );
}

fn panel<'a>(title: impl Into<Line<'a>>, t: &Theme, focused: bool) -> Block<'a> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if focused { Style::new().fg(t.accent) } else { t.dim })
        .title(title)
}

fn draw_profiles(f: &mut Frame<'_>, app: &App, t: &Theme, area: Rect) {
    let list_width = (area.width * 36 / 100).clamp(26, 40);
    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Length(list_width), Constraint::Min(20)]).areas(area);
    let rows = app.rows();
    let default = app.ctx.config.settings.default_profile.as_deref();
    let items: Vec<ListItem<'_>> = rows
        .iter()
        .enumerate()
        .map(|(i, target)| {
            let running = app.running.pid_for_dir(target.data_dir()).is_some();
            let dot = if running { Span::styled("● ", Style::new().fg(t.good)) } else { Span::styled("○ ", t.dim) };
            let name_style = if i == app.sel { Style::new().fg(t.accent) } else { Style::new() };
            let mut spans = vec![dot, chip(target), Span::raw(" "), Span::styled(target.name().to_owned(), name_style)];
            if default == Some(target.name()) {
                spans.push(Span::styled("  default", t.dim));
            }
            if matches!(target, Target::Default) {
                spans.push(Span::styled("  built in", t.dim));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let title = if app.filter.is_empty() {
        Line::from(format!(" Profiles ({}) ", rows.len().saturating_sub(1)))
    } else {
        Line::from(vec![
            Span::raw(" Filter: "),
            Span::styled(app.filter.clone(), Style::new().fg(t.accent)),
            Span::raw(" "),
        ])
    };
    let list = List::new(items)
        .block(panel(title, t, true))
        .highlight_style(Style::new().add_modifier(Modifier::BOLD))
        .highlight_symbol(Span::styled("▌", Style::new().fg(t.accent)));
    let mut state = ListState::default().with_selected(Some(app.sel));
    f.render_stateful_widget(list, list_area, &mut state);

    match rows.get(app.sel) {
        Some(target) => draw_details(f, app, t, target, detail_area),
        None => {
            let msg = if app.filter.is_empty() { "No profiles." } else { "Nothing matches the filter. Esc clears it." };
            f.render_widget(Paragraph::new(msg).style(t.dim).block(panel(" Details ", t, false)), detail_area);
        }
    }
}

fn kv<'a>(k: &'a str, v: impl Into<Span<'a>>, t: &Theme) -> Line<'a> {
    Line::from(vec![Span::styled(format!("{k:<14}"), t.dim), v.into()])
}

fn draw_details(f: &mut Frame<'_>, app: &App, t: &Theme, target: &Target<'_>, area: Rect) {
    let pid = app.running.pid_for_dir(target.data_dir());
    let status: Span<'_> = match pid {
        Some(pid) => Span::styled(format!("Running (pid {pid})"), Style::new().fg(t.good)),
        None => Span::styled("Not running", t.dim),
    };
    let mut lines: Vec<Line<'_>> = vec![
        Line::from(vec![
            chip(target),
            Span::raw(" "),
            Span::styled(target.name().to_owned(), Style::new().add_modifier(Modifier::BOLD)),
        ]),
        Line::raw(""),
        kv("Status", status, t),
    ];
    match target {
        Target::Default => {
            lines.push(kv("Data", Span::raw("Claude's own profile, where Claude keeps it"), t));
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "This is the Claude you get when you open Claude.app yourself. Add profiles with n to keep other accounts apart.",
                t.dim,
            ));
        }
        Target::Profile(p) => {
            let size = if p.data_dir.exists() {
                app.sizes.get(&dir_key(&p.data_dir)).map_or_else(|| "measuring…".to_owned(), |s| human_size(*s))
            } else {
                "not created yet (opens on first launch)".to_owned()
            };
            let room = usize::from(area.width.saturating_sub(18));
            lines.push(kv("Data", Span::raw(short_path(&p.data_dir, room)), t));
            lines.push(kv("Size", Span::raw(size), t));
            lines.push(kv(
                "Last opened",
                Span::raw(format!(
                    "{}{}",
                    ago(p.last_used),
                    if p.launches > 0 { format!(" · {} launches", p.launches) } else { String::new() }
                )),
                t,
            ));
            lines.push(kv(
                "Look",
                Span::raw(format!(
                    "{} {}",
                    p.color,
                    match p.badge() {
                        Badge::Symbol(s) => format!("with a {s} badge"),
                        Badge::Letter(c) => format!("with the letter {c}"),
                        Badge::None => "with no badge".into(),
                    }
                )),
                t,
            ));
            if !p.extra_args.is_empty() {
                lines.push(kv("Flags", Span::raw(p.extra_args.join(" ")), t));
            }
            if !p.env.is_empty() {
                let env: Vec<String> = p.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
                lines.push(kv("Environment", Span::raw(env.join(" ")), t));
            }
            if cfg!(target_os = "macos") && p.own_app {
                lines.push(kv("Dock icon", Span::raw("its own, from a private copy of Claude"), t));
            }
            let shortcuts = crate::platform::existing_shortcuts(&p.name);
            let sc = if shortcuts.is_empty() {
                Span::styled("none", t.dim)
            } else {
                Span::raw(
                    shortcuts
                        .iter()
                        .filter_map(|s| s.parent().map(|d| short_path(d, 40)))
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            };
            lines.push(kv("Shortcuts", sc, t));
            if !p.notes.is_empty() {
                lines.push(Line::raw(""));
                lines.push(Line::styled(p.notes.clone(), Style::new().add_modifier(Modifier::ITALIC)));
            }
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel(" Details ", t, false).padding(ratatui::widgets::Padding::horizontal(1))),
        area,
    );
}

fn on(v: bool) -> &'static str {
    if v { "On" } else { "Off" }
}

fn setting_row(app: &App, s: Setting) -> (&'static str, String, &'static str) {
    let c = &app.ctx.config.settings;
    match s {
        Setting::ClaudePath => (
            "Claude Desktop",
            match (&c.claude_path, &app.claude) {
                (Some(p), _) => short_path(p, 64),
                (None, Some(i)) => format!("{} (found)", short_path(&i.app, 56)),
                (None, None) => "not found".into(),
            },
            "Where Claude Desktop is. Leave it to automatic unless it's somewhere unusual.",
        ),
        Setting::DefaultProfile => (
            "Default profile",
            c.default_profile.clone().unwrap_or_else(|| "Claude (built in)".into()),
            "What `launch` opens with no name, and where links go when set to Default profile.",
        ),
        Setting::RouteLinks => (
            "Route sign-in links",
            on(c.route_links).into(),
            "Claude sends browser sign-ins and connector callbacks to whichever Claude started last. With this on, they ask which profile they're for.",
        ),
        Setting::LinkRouting => {
            ("When a link arrives", c.link_routing.label().into(), "Ask, or send links straight to a profile.")
        }
        Setting::AppShortcuts => (
            "App shortcuts",
            on(c.app_shortcuts).into(),
            "A shortcut per profile in your Applications folder, Start menu or app launcher, with the profile's icon.",
        ),
        Setting::DesktopShortcuts => {
            ("Desktop shortcuts", on(c.desktop_shortcuts).into(), "Also put a shortcut per profile on the Desktop.")
        }
        Setting::DockPins => ("Pin to Dock", on(c.dock_pins).into(), "Keep every profile's shortcut in the Dock."),
        Setting::Sort => ("Sort profiles", c.sort.label().into(), "Manual order lets you move profiles with J and K."),
        Setting::Accent => ("Accent color", c.accent.clone(), "The color of this manager's highlights."),
        Setting::ConfirmQuit => {
            ("Ask before quitting", on(c.confirm_quit).into(), "Confirm before quitting a running profile with x.")
        }
        Setting::GlobalArgs => (
            "Flags for every profile",
            if c.global_args.is_empty() { "none".into() } else { c.global_args.join(" ") },
            "Command-line flags passed to Claude for every profile, such as --disable-gpu.",
        ),
        Setting::CheckUpdates => {
            ("Check for updates", on(c.check_updates).into(), "Look for a new release on GitHub once a day.")
        }
        Setting::ConfigFile => {
            ("Config file", short_path(&app.ctx.paths.config_file, 64), "Open it in your editor. It's plain TOML.")
        }
    }
}

fn draw_settings(f: &mut Frame<'_>, app: &App, t: &Theme, area: Rect) {
    let items = Setting::all();
    let [list_area, help_area] = Layout::vertical([Constraint::Min(3), Constraint::Length(4)]).areas(area);
    let rows: Vec<ListItem<'_>> = items
        .iter()
        .map(|s| {
            let (label, value, _) = setting_row(app, *s);
            ListItem::new(Line::from(vec![
                Span::raw(format!("{label:<26}")),
                Span::styled(value, Style::new().fg(t.accent)),
            ]))
        })
        .collect();
    let list = List::new(rows)
        .block(panel(" Settings ", t, true))
        .highlight_style(Style::new().add_modifier(Modifier::BOLD))
        .highlight_symbol(Span::styled("▌", Style::new().fg(t.accent)));
    let mut state = ListState::default().with_selected(Some(app.settings_sel));
    f.render_stateful_widget(list, list_area, &mut state);
    let help = items.get(app.settings_sel).map(|s| setting_row(app, *s).2).unwrap_or_default();
    f.render_widget(
        Paragraph::new(help)
            .style(t.dim)
            .wrap(Wrap { trim: true })
            .block(Block::new().padding(ratatui::widgets::Padding::horizontal(1))),
        help_area,
    );
}

fn draw_doctor(f: &mut Frame<'_>, app: &App, t: &Theme, area: Rect) {
    let items: Vec<ListItem<'_>> = app
        .doctor
        .iter()
        .map(|c| {
            let (mark, color) = match c.level {
                Level::Ok => ("✓", t.good),
                Level::Warn => ("!", t.warn),
                Level::Fail => ("✗", t.bad),
                Level::Info => ("·", Color::DarkGray),
            };
            let mut text = Text::from(Line::from(vec![
                Span::styled(format!(" {mark} "), Style::new().fg(color).add_modifier(Modifier::BOLD)),
                Span::raw(c.title.clone()),
            ]));
            if !c.detail.is_empty() {
                text.push_line(Line::styled(format!("   {}", c.detail), t.dim));
            }
            ListItem::new(text)
        })
        .collect();
    let list = List::new(items).block(panel(" Doctor ", t, true)).highlight_style(Style::new().bg(Color::Indexed(236)));
    let mut state = ListState::default().with_selected(Some(app.doctor_sel));
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_status(f: &mut Frame<'_>, app: &App, t: &Theme, area: Rect) {
    if let Some((msg, tone, _)) = &app.status {
        let color = match tone {
            Tone::Good => t.good,
            Tone::Bad => t.bad,
            Tone::Info => Color::Reset,
        };
        f.render_widget(Paragraph::new(format!(" {msg}")).style(Style::new().fg(color)), area);
    }
}

fn draw_keys(f: &mut Frame<'_>, app: &App, t: &Theme, area: Rect) {
    let pairs: &[(&str, &str)] = match (&app.modal, app.tab) {
        (Some(Modal::Input { .. }), _) => &[("enter", "save"), ("esc", "cancel")],
        (Some(Modal::Confirm { .. }), _) => &[("y", "yes"), ("n", "no")],
        (Some(Modal::Colors { .. }), _) => {
            &[("↑↓", "choose"), ("enter", "use"), ("#", "custom hex"), ("esc", "cancel")]
        }
        (Some(_), _) => &[("↑↓", "choose"), ("enter", "select"), ("esc", "close")],
        (None, Tab::Profiles) => &[
            ("enter", "open"),
            ("n", "new"),
            ("e", "edit"),
            ("c", "color"),
            ("b", "badge"),
            ("x", "quit it"),
            ("d", "remove"),
            ("/", "filter"),
            ("?", "all keys"),
            ("q", "exit"),
        ],
        (None, Tab::Settings) => &[("↑↓", "choose"), ("enter/←→", "change"), ("tab", "next tab"), ("q", "exit")],
        (None, Tab::Doctor) => &[("r", "check again"), ("tab", "next tab"), ("q", "exit")],
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, v) in pairs {
        spans.push(Span::styled((*k).to_owned(), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {v}   "), t.dim));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn draw_modal(f: &mut Frame<'_>, app: &App, t: &Theme, modal: &Modal) {
    match modal {
        Modal::Input { title, hint, value, cursor, .. } => {
            let area = centered(f.area(), 64, 7);
            f.render_widget(Clear, area);
            let block = panel(format!(" {title} "), t, true);
            let inner = block.inner(area).inner(Margin::new(1, 0));
            f.render_widget(block, area);
            let [hint_area, _, field] =
                Layout::vertical([Constraint::Length(2), Constraint::Length(1), Constraint::Length(1)]).areas(inner);
            f.render_widget(Paragraph::new(hint.as_str()).style(t.dim).wrap(Wrap { trim: true }), hint_area);
            // Scroll long values so the cursor stays visible.
            let width = usize::from(field.width.saturating_sub(1));
            let chars: Vec<char> = value.chars().collect();
            let start = cursor.saturating_sub(width);
            let shown: String = chars.iter().skip(start).take(width).collect();
            f.render_widget(Paragraph::new(shown).style(Style::new().add_modifier(Modifier::BOLD)), field);
            f.set_cursor_position((field.x + u16::try_from(cursor - start).unwrap_or(0), field.y));
        }
        Modal::Confirm { title, body, yes, .. } => {
            let area = centered(f.area(), 60, 8);
            f.render_widget(Clear, area);
            let text = vec![
                Line::raw(body.clone()),
                Line::raw(""),
                Line::from(vec![
                    Span::styled(
                        format!(" y {yes} "),
                        Style::new().fg(Color::Black).bg(t.accent).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("   "),
                    Span::styled(" n Cancel ", t.dim),
                ]),
            ];
            f.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: true })
                    .block(panel(format!(" {title} "), t, true).padding(ratatui::widgets::Padding::horizontal(1))),
                area,
            );
        }
        Modal::Edit { name, sel } => {
            let p = app.ctx.config.find(name);
            let fields = Field::all();
            let items: Vec<ListItem<'_>> = fields
                .iter()
                .map(|fl| {
                    let value = p.map_or(String::new(), |p| match fl {
                        Field::Name => p.name.clone(),
                        Field::Color => p.color.clone(),
                        Field::Badge => p.badge().as_config(),
                        Field::Args => p.extra_args.join(" "),
                        Field::Env => p.env.keys().cloned().collect::<Vec<_>>().join(", "),
                        Field::Notes => p.notes.chars().take(24).collect(),
                        Field::OwnApp => on(p.own_app).into(),
                        Field::Default => {
                            on(app.ctx.config.settings.default_profile.as_deref() == Some(p.name.as_str())).into()
                        }
                    });
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("{:<30}", fl.label())),
                        Span::styled(value, Style::new().fg(t.accent)),
                    ]))
                })
                .collect();
            let area = centered(f.area(), 66, u16::try_from(fields.len()).unwrap_or(8) + 2);
            f.render_widget(Clear, area);
            let list = List::new(items)
                .block(panel(format!(" Edit {name} "), t, true))
                .highlight_symbol(Span::styled("▌", Style::new().fg(t.accent)))
                .highlight_style(Style::new().add_modifier(Modifier::BOLD));
            f.render_stateful_widget(list, area, &mut ListState::default().with_selected(Some(*sel)));
        }
        Modal::Colors { name, sel } => {
            let items: Vec<ListItem<'_>> = COLORS
                .iter()
                .map(|(n, c)| {
                    ListItem::new(Line::from(vec![
                        Span::styled("    ", Style::new().bg(rgb(*c))),
                        Span::raw(format!("  {n}")),
                    ]))
                })
                .collect();
            let area = centered(f.area(), 30, 21);
            f.render_widget(Clear, area);
            let list = List::new(items)
                .block(panel(format!(" Color for {name} "), t, true))
                .highlight_symbol(Span::styled("▌", Style::new().fg(t.accent)))
                .highlight_style(Style::new().add_modifier(Modifier::BOLD));
            f.render_stateful_widget(list, area, &mut ListState::default().with_selected(Some(*sel)));
        }
        Modal::Badges { name, sel } => {
            let rgb_c = app.ctx.config.find(name).map_or([115, 128, 143], |p| p.rgb());
            let mut items: Vec<ListItem<'_>> = SYMBOLS
                .iter()
                .map(|s| {
                    ListItem::new(Line::from(vec![
                        Span::styled(format!(" {} ", symbol_glyph(s)), Style::new().fg(Color::White).bg(rgb(rgb_c))),
                        Span::raw(format!("  {s}")),
                    ]))
                })
                .collect();
            items.push(ListItem::new(Line::raw("  …   a letter or digit")));
            items.push(ListItem::new(Line::raw("      no badge")));
            let area = centered(f.area(), 32, 16);
            f.render_widget(Clear, area);
            let list = List::new(items)
                .block(panel(format!(" Badge for {name} "), t, true))
                .highlight_symbol(Span::styled("▌", Style::new().fg(t.accent)))
                .highlight_style(Style::new().add_modifier(Modifier::BOLD));
            f.render_stateful_widget(list, area, &mut ListState::default().with_selected(Some(*sel)));
        }
        Modal::Help => {
            let keys: &[(&str, &str)] = &[
                ("enter, o", "Open the profile, or bring it to the front"),
                ("n", "New profile"),
                ("e", "Edit (name, look, flags, environment, notes)"),
                ("r", "Rename"),
                ("c / b", "Color / badge"),
                ("*", "Make it the default profile"),
                ("x / X", "Quit it / quit every profile"),
                ("f", "Sign out (moves its data to the trash)"),
                ("d / D", "Remove / remove and trash its data"),
                ("s", "Rebuild its shortcuts"),
                ("O", "Open its data folder"),
                ("J / K", "Move down / up (manual sort)"),
                ("/", "Filter"),
                ("R", "Refresh"),
                ("U", "Update (when one is available)"),
                ("1 2 3, tab", "Switch tabs"),
                ("q, ctrl-c", "Exit"),
            ];
            let lines: Vec<Line<'_>> = keys
                .iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(format!("{k:<12}"), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
                        Span::raw(*v),
                    ])
                })
                .collect();
            let area = centered(f.area(), 64, u16::try_from(keys.len()).unwrap_or(18) + 2);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(lines).block(panel(" Keys ", t, true).padding(ratatui::widgets::Padding::horizontal(1))),
                area,
            );
        }
    }
}
