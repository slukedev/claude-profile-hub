//! Claude Profile Hub: separate Claude Desktop profiles, each with its own sign-in, chats
//! and connectors, managed from one command-line tool and TUI.

pub mod claude;
pub mod cli;
pub mod config;
pub mod ctx;
pub mod doctor;
pub mod follow;
pub mod icons;
pub mod launch;
pub mod ops;
pub mod platform;
pub mod process;
pub mod profile;
pub mod tui;
pub mod update;

/// The command name, used in messages and generated shortcuts.
pub const BIN: &str = "claude-profile-hub";
/// The product name.
pub const NAME: &str = "Claude Profile Hub";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The GitHub repository releases come from.
pub const REPO: &str = "slukedev/claude-profile-hub";
/// URL schemes Claude Desktop registers: its own links and the Microsoft sign-in callback.
pub const SCHEMES: &[&str] = &["claude", "msauth.com.anthropic.claudefordesktop"];
