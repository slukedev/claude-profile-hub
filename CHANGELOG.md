# Changelog

## Unreleased

- A running profile now shows its own icon in the Dock, taskbar or app switcher, not only
  while its shortcut opens it, on macOS, Windows and Linux. On by default for every profile;
  `edit <name> --own-icon off` turns it off (`--own-app` still works)
- macOS: when Claude updates itself from a profile's own copy, the new version is put in place
  of Claude.app so the other profiles follow it

## 0.1.0

First release, for macOS, Windows and Linux.

- Terminal UI with Profiles, Settings and Doctor tabs: live running status, data folder sizes,
  and one-key open, quit, rename, recolor, re-badge, sign out and remove
- Command line for everything: `add`, `launch`, `list --json`, `edit`, `rename`, `stop`,
  `fresh`, `remove`, `shortcuts`, `links`, `config`, `doctor`, `update`, `export`,
  `import`, `completions`
- Shortcuts for the macOS Applications folder and Dock, the Windows Start menu, Linux app
  launchers, and the Desktop, each with Claude's icon recolored per profile and a badge (12
  symbols, letters, 19 colors or any hex)
- Layered macOS 26 icons with Dark, Clear and Tinted styles when Xcode 26 is installed; Dark
  keeps each profile's color (tinted dark background, mark in the profile color)
- When Claude updates from one profile, the others are quit and reopened on the new version,
  and the profile that updated is put back in place of the default one the updater reopens;
  optional background watcher to do this within seconds
- Opening a running profile brings its window back instead of starting another copy
- Sign-in link routing for `claude://` on all three platforms
- Optional own Dock icon per profile on macOS (APFS clone of Claude.app)
- Per-profile and global Claude flags, per-profile environment variables and notes
- Sign-out and data removal go to the system trash
- Self-update with checksum verification; Homebrew and winget packages
