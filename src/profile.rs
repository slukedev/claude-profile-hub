//! Profile names, colors and badge symbols.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Longest profile name we accept. Long names make awkward shortcut and folder names.
pub const MAX_NAME_LEN: usize = 40;

/// Words that are subcommands. A profile with one of these names would be ambiguous on
/// the command line (`claude-profile-hub list` vs. a profile called "list").
const RESERVED: &[&str] = &[
    "add",
    "new",
    "create",
    "rm",
    "remove",
    "delete",
    "list",
    "ls",
    "launch",
    "open",
    "run",
    "quit",
    "stop",
    "edit",
    "rename",
    "icon",
    "fresh",
    "reset",
    "status",
    "doctor",
    "update",
    "upgrade",
    "import",
    "export",
    "route",
    "links",
    "shortcut",
    "shortcuts",
    "config",
    "settings",
    "help",
    "version",
    "completions",
    "default",
    "tui",
];

/// Named colors. The first eight are handed out in order to new profiles.
pub const COLORS: &[(&str, [u8; 3])] = &[
    ("indigo", [97, 89, 199]),
    ("sage", [115, 173, 133]),
    ("orange", [245, 140, 71]),
    ("purple", [148, 84, 219]),
    ("teal", [31, 173, 189]),
    ("pink", [219, 66, 140]),
    ("coral", [237, 107, 79]),
    ("slate", [115, 128, 143]),
    ("amber", [224, 158, 51]),
    ("graphite", [51, 56, 71]),
    ("gold", [219, 153, 33]),
    ("red", [219, 74, 92]),
    ("brown", [143, 97, 69]),
    ("ocean", [43, 133, 173]),
    ("violet", [110, 82, 230]),
    ("green", [74, 148, 92]),
    ("navy", [48, 56, 107]),
    ("sky", [69, 153, 224]),
    ("clay", [217, 120, 87]),
];

/// Badge symbols drawn on profile icons. Anything else is shown as a letter.
pub const SYMBOLS: &[&str] =
    &["work", "personal", "home", "code", "star", "heart", "book", "globe", "bolt", "leaf", "moon", "chat"];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum NameError {
    #[error("a profile name can't be empty")]
    Empty,
    #[error("a profile name can be at most {MAX_NAME_LEN} characters")]
    TooLong,
    #[error("a profile name can't start with a dot")]
    LeadingDot,
    #[error("a profile name can't contain {0:?}")]
    BadChar(char),
    #[error("\"{0}\" is a claude-profile-hub command, so pick another name")]
    Reserved(String),
}

/// Checks a display name and returns it trimmed.
///
/// ```
/// assert_eq!(profilehub::profile::validate_name("  Work ").unwrap(), "Work");
/// assert!(profilehub::profile::validate_name("a/b").is_err());
/// ```
pub fn validate_name(raw: &str) -> Result<String, NameError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.chars().count() > MAX_NAME_LEN {
        return Err(NameError::TooLong);
    }
    if name.starts_with('.') {
        return Err(NameError::LeadingDot);
    }
    // Characters that are unsafe in a file name on at least one platform.
    if let Some(c) =
        name.chars().find(|c| c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err(NameError::BadChar(c));
    }
    if RESERVED.contains(&name.to_lowercase().as_str()) {
        return Err(NameError::Reserved(name.to_owned()));
    }
    Ok(name.to_owned())
}

/// A folder-safe, lowercase identifier for a name: "Big Client" -> "big-client".
///
/// ```
/// assert_eq!(profilehub::profile::slug("Big  Client!"), "big-client");
/// assert_eq!(profilehub::profile::slug("Ünïcode"), "ünïcode");
/// ```
pub fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for c in name.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "profile".to_owned() } else { out }
}

/// Parses a color name or `#RRGGBB`.
pub fn parse_color(value: &str) -> Option<[u8; 3]> {
    let v = value.trim().to_lowercase();
    if let Some((_, rgb)) = COLORS.iter().find(|(n, _)| *n == v) {
        return Some(*rgb);
    }
    let hex = v.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

/// A badge: one of [`SYMBOLS`], a single letter or digit, or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Badge {
    Symbol(&'static str),
    Letter(char),
    None,
}

impl Badge {
    /// Parses user input; `None` means it isn't a valid badge.
    pub fn parse(value: &str) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("none") {
            return Some(Self::None);
        }
        if let Some(s) = SYMBOLS.iter().find(|s| s.eq_ignore_ascii_case(v)) {
            return Some(Self::Symbol(s));
        }
        let mut chars = v.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_alphanumeric() => Some(Self::Letter(c.to_ascii_uppercase())),
            _ => None,
        }
    }

    pub fn as_config(&self) -> String {
        match self {
            Self::Symbol(s) => (*s).to_owned(),
            Self::Letter(c) => c.to_string(),
            Self::None => "none".to_owned(),
        }
    }
}

/// The badge a profile gets when the user hasn't picked one.
pub fn default_badge(name: &str) -> Badge {
    match name.to_lowercase().as_str() {
        "work" | "company" | "job" => Badge::Symbol("work"),
        "personal" | "me" | "private" => Badge::Symbol("personal"),
        "home" | "family" => Badge::Symbol("home"),
        "code" | "dev" | "development" => Badge::Symbol("code"),
        _ => name
            .chars()
            .find(|c| c.is_alphanumeric())
            .map_or(Badge::None, |c| Badge::Letter(c.to_uppercase().next().unwrap_or(c))),
    }
}

/// The color a new profile gets: Work and Personal keep their familiar colors,
/// others take the next palette color not already in use.
pub fn default_color<'a>(name: &str, taken: impl IntoIterator<Item = &'a str>) -> &'static str {
    match name.to_lowercase().as_str() {
        "work" => return "indigo",
        "personal" => return "sage",
        _ => {}
    }
    let taken: Vec<&str> = taken.into_iter().collect();
    COLORS.iter().take(8).map(|(n, _)| *n).find(|n| !taken.contains(n)).unwrap_or(COLORS[(name.len()) % 8].0)
}

/// One Claude profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Profile {
    pub name: String,
    /// Chromium user data folder; holds the sign-in, chats, settings and connectors.
    pub data_dir: PathBuf,
    #[serde(default = "default_color_name")]
    pub color: String,
    /// Symbol name, a single letter, or "none". Empty means the default for the name.
    #[serde(default)]
    pub badge: String,
    /// Extra command-line flags passed to Claude for this profile.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
    /// Extra environment variables for this profile's Claude process.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Unix seconds.
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub last_used: i64,
    #[serde(default)]
    pub launches: u64,
    /// macOS: run from a private copy of Claude.app so the Dock shows this profile's icon.
    #[serde(default)]
    pub own_app: bool,
}

fn default_color_name() -> String {
    "slate".to_owned()
}

impl Profile {
    pub fn badge(&self) -> Badge {
        Badge::parse(&self.badge).unwrap_or_else(|| default_badge(&self.name))
    }

    pub fn rgb(&self) -> [u8; 3] {
        parse_color(&self.color).unwrap_or([115, 128, 143])
    }

    pub fn slug(&self) -> String {
        slug(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_trims_whitespace() {
        assert_eq!(validate_name("  Big Client  ").as_deref(), Ok("Big Client"));
    }

    #[test]
    fn validate_name_rejects_path_separators() {
        assert_eq!(validate_name("a/b"), Err(NameError::BadChar('/')));
        assert_eq!(validate_name("a\\b"), Err(NameError::BadChar('\\')));
    }

    #[test]
    fn validate_name_rejects_commands_case_insensitively() {
        assert!(matches!(validate_name("List"), Err(NameError::Reserved(_))));
    }

    #[test]
    fn validate_name_rejects_overlong_names() {
        assert_eq!(validate_name(&"x".repeat(MAX_NAME_LEN + 1)), Err(NameError::TooLong));
    }

    #[test]
    fn validate_name_accepts_apostrophes_and_unicode() {
        assert!(validate_name("Luke's Café").is_ok());
    }

    #[test]
    fn slug_collapses_punctuation() {
        assert_eq!(slug("Client -- A"), "client-a");
        assert_eq!(slug("!!!"), "profile");
    }

    #[test]
    fn parse_color_accepts_names_and_hex() {
        assert_eq!(parse_color("Teal"), Some([31, 173, 189]));
        assert_eq!(parse_color("#3366ff"), Some([0x33, 0x66, 0xff]));
        assert_eq!(parse_color("#36f"), None);
        assert_eq!(parse_color("blurple"), None);
    }

    #[test]
    fn badge_parse_handles_symbols_letters_and_none() {
        assert_eq!(Badge::parse("Heart"), Some(Badge::Symbol("heart")));
        assert_eq!(Badge::parse("b"), Some(Badge::Letter('B')));
        assert_eq!(Badge::parse("none"), Some(Badge::None));
        assert_eq!(Badge::parse("xy"), None);
    }

    #[test]
    fn default_badge_maps_common_names() {
        assert_eq!(default_badge("Work"), Badge::Symbol("work"));
        assert_eq!(default_badge("acme"), Badge::Letter('A'));
    }

    #[test]
    fn default_color_skips_colors_in_use() {
        assert_eq!(default_color("Acme", ["indigo", "sage"]), "orange");
        assert_eq!(default_color("Work", ["indigo"]), "indigo");
    }
}
