//! Windows only: the same program built without a console window, for shortcuts and the
//! link handler, so clicking a profile doesn't flash a terminal.
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    std::process::exit(profilehub::cli::main_entry());
}
