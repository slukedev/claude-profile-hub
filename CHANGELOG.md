# Changelog

## 0.1.0

First release, for macOS, Windows and Linux.

- Terminal UI with Profiles, Settings and Doctor tabs: live running status, data folder sizes,
  and one-key open, quit, rename, recolor, re-badge, sign out and remove
- Command line for everything: `add`, `launch`, `list --json`, `edit`, `rename`, `stop`,
  `fresh`, `remove`, `shortcuts`, `links`, `config`, `doctor`, `update`, `export`,
  `import`, `completions`
- Shortcuts with generated icons (12 symbols, letters, 19 colors or any hex) for the macOS
  Applications folder and Dock, the Windows Start menu, Linux app launchers, and the Desktop
- Opening a running profile brings its window back instead of starting another copy
- Sign-in link routing for `claude://` on all three platforms
- Optional own Dock icon per profile on macOS (APFS clone of Claude.app)
- Per-profile and global Claude flags, per-profile environment variables and notes
- Sign-out and data removal go to the system trash
- Self-update with checksum verification; Homebrew and winget packages
