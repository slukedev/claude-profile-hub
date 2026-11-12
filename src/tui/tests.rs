use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use crate::config::{Config, Paths, State};

fn app() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Paths::rooted(dir.path().to_path_buf());
    let mut config = Config::default();
    // Never touch the real Applications folder or Desktop from tests.
    config.settings.app_shortcuts = false;
    config.settings.desktop_shortcuts = false;
    config.settings.check_updates = false;
    let ctx = Ctx { paths, config, state: State::default() };
    let app = App::new(ctx);
    (dir, app)
}

fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn screen(app: &App) -> String {
    let mut term = Terminal::new(TestBackend::new(110, 30)).expect("terminal");
    term.draw(|f| view::draw(f, app)).expect("draw");
    let buf = term.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_owned()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn new_profile_flow_adds_and_selects_it() {
    let (_d, mut app) = app();
    press(&mut app, KeyCode::Char('n'));
    type_str(&mut app, "Work");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.ctx.config.profiles.len(), 1);
    assert_eq!(app.selected_name().as_deref(), Some("Work"));
}

#[test]
fn rename_flow_replaces_the_name() {
    let (_d, mut app) = app();
    ops::add(&mut app.ctx, "Work", None, None, None).expect("add");
    app.select_name("Work");
    press(&mut app, KeyCode::Char('r'));
    for _ in 0..4 {
        press(&mut app, KeyCode::Backspace);
    }
    type_str(&mut app, "Acme");
    press(&mut app, KeyCode::Enter);
    assert!(app.ctx.config.find("Acme").is_some());
}

#[test]
fn remove_needs_confirmation() {
    let (_d, mut app) = app();
    ops::add(&mut app.ctx, "Work", None, None, None).expect("add");
    app.select_name("Work");
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(app.ctx.config.profiles.len(), 1, "n cancels");
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('y'));
    assert!(app.ctx.config.profiles.is_empty());
}

#[test]
fn color_picker_sets_the_chosen_color() {
    let (_d, mut app) = app();
    ops::add(&mut app.ctx, "Work", None, None, None).expect("add");
    app.select_name("Work");
    press(&mut app, KeyCode::Char('c'));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.ctx.config.profiles[0].color, "sage");
}

#[test]
fn filter_narrows_the_list_and_esc_clears_it() {
    let (_d, mut app) = app();
    ops::add(&mut app.ctx, "Work", None, None, None).expect("add");
    ops::add(&mut app.ctx, "Personal", None, None, None).expect("add");
    press(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "pers");
    assert_eq!(app.rows().len(), 1);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.rows().len(), 3);
}

#[test]
fn default_profile_cannot_be_edited() {
    let (_d, mut app) = app();
    app.sel = 0;
    press(&mut app, KeyCode::Char('d'));
    assert!(app.modal.is_none());
}

#[test]
fn settings_toggle_saves_to_disk() {
    let (_d, mut app) = app();
    press(&mut app, KeyCode::Char('2'));
    let idx = Setting::all().iter().position(|s| *s == Setting::ConfirmQuit).expect("row");
    app.settings_sel = idx;
    press(&mut app, KeyCode::Enter);
    let saved = Config::load(&app.ctx.paths.config_file).expect("load");
    assert!(!saved.settings.confirm_quit);
}

#[test]
fn edit_text_handles_unicode_and_word_delete() {
    let (mut v, mut c) = (String::from("héllo wörld"), 11);
    edit_text(&mut v, &mut c, KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!((v.as_str(), c), ("héllo ", 6));
    edit_text(&mut v, &mut c, KeyCode::Backspace, KeyModifiers::NONE);
    edit_text(&mut v, &mut c, KeyCode::Left, KeyModifiers::NONE);
    edit_text(&mut v, &mut c, KeyCode::Char('X'), KeyModifiers::NONE);
    assert_eq!(v, "héllXo");
}

#[test]
fn screen_shows_profiles_and_details() {
    let (_d, mut app) = app();
    ops::add(&mut app.ctx, "Work", None, None, None).expect("add");
    app.select_name("Work");
    let s = screen(&app);
    assert!(s.contains("Claude Profile Hub"));
    assert!(s.contains("Work"));
    assert!(s.contains("Not running"));
    assert!(s.contains("indigo with a work badge"));
}

#[test]
fn every_tab_and_modal_renders_at_small_sizes() {
    let (_d, mut app) = app();
    ops::add(&mut app.ctx, "Work", None, None, None).expect("add");
    app.select_name("Work");
    let modals = [
        None,
        Some(Modal::Help),
        Some(Modal::Edit { name: "Work".into(), sel: 0 }),
        Some(Modal::Colors { name: "Work".into(), sel: 3 }),
        Some(Modal::Badges { name: "Work".into(), sel: 2 }),
    ];
    for tab in Tab::ALL {
        app.set_tab(tab);
        for m in &modals {
            app.modal = m.clone();
            for (w, h) in [(40, 12), (80, 24), (160, 50)] {
                let mut term = Terminal::new(TestBackend::new(w, h)).expect("terminal");
                term.draw(|f| view::draw(f, &app)).expect("draw");
            }
        }
    }
}
