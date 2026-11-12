# Claude Profile Hub

Run separate Claude Desktop profiles side by side on macOS, Windows and Linux. Each profile has
its own sign-in, chats, settings and connectors, so your work Gmail, Slack and calendar never mix
with your personal ones.

`claude-profile-hub` is a small, single-binary tool with a terminal UI for managing profiles and a
command line for scripting them.

Third-party and open source. Not affiliated with or endorsed by Anthropic.

## Install

macOS and Linux:

```bash
brew install slukedev/tap/claude-profile-hub
```

Windows:

```powershell
winget install slukedev.ClaudeProfileHub
```

Or without a package manager:

```bash
curl -fsSL https://raw.githubusercontent.com/slukedev/claude-profile-hub/main/install.sh | sh
```

```powershell
irm https://raw.githubusercontent.com/slukedev/claude-profile-hub/main/install.ps1 | iex
```

Both scripts download the build for your platform from the latest GitHub release, check it
against the release's `SHA256SUMS`, and install it for your user only (no admin rights). From
source: `cargo install --git https://github.com/slukedev/claude-profile-hub`.

## Use it

Run `claude-profile-hub` with no arguments to open the manager:

- **Profiles**: every profile with a live running indicator, its data folder and size, when you
  last used it, its flags and notes. Open, quit, rename, recolor, re-badge, sign out, or remove
  profiles with one key each. Press `?` for every key.
- **Settings**: shortcuts, link routing, the default profile, sorting, the accent color, and more.
- **Doctor**: checks your setup and explains anything that's off.

Everything is also on the command line:

```bash
claude-profile-hub add Personal --launch          # new profile, open it to sign in
claude-profile-hub add "Acme Corp" --color teal --badge A
claude-profile-hub launch work                     # any unique prefix works
claude-profile-hub list --json
claude-profile-hub edit Work --arg=--disable-gpu --env LANG=en_US.UTF-8
claude-profile-hub rename Personal Home
claude-profile-hub stop --all
claude-profile-hub fresh Personal                  # sign out (data goes to the trash)
claude-profile-hub remove Acme                     # keeps its data unless you pass --purge
claude-profile-hub doctor
claude-profile-hub update
```

`claude-profile-hub help <command>` explains each one.

## What you get

- **Isolated profiles.** Each profile runs Claude Desktop with its own `--user-data-dir`, so
  sign-ins, chats, settings and connectors are separate. Profiles run at the same time.
- **Shortcuts with their own icons.** Every profile gets a shortcut in your Applications folder,
  Start menu or app launcher (and optionally the Desktop and the macOS Dock). Its icon is
  Claude's own icon recolored to the profile's color, with a small badge: one of 12 symbols, a
  letter, or nothing. 19 named colors or any hex color. Claude's icon is read from the Claude
  install on your computer; none of Anthropic's artwork ships with this project.
- **macOS 26 icon styles.** With Xcode 26 installed, shortcut icons are built as layered icons,
  so macOS draws real Dark, Clear and Tinted versions. Without Xcode, macOS applies its
  automatic treatment.
- **Every profile follows Claude updates.** Claude updates itself from whichever profile asks
  first, leaving the others on the old version, and on macOS it reopens Claude's own profile
  instead of the one you updated from. Claude Profile Hub notices the new version, quits the
  profiles still on the old one, reopens them, and puts you back in the profile you updated
  from. It checks whenever you open a profile; turn on the background watcher
  (`claude-profile-hub config watcher on`, or in Settings) to have it happen within seconds.
- **Bring back, don't duplicate.** Opening a profile that's already running brings its window
  forward, even after you closed the window, instead of starting a second copy.
- **Sign-in links that land in the right place.** Claude grabs `claude://` links every time it
  starts, so with several profiles a browser sign-in or connector callback lands in whichever one
  started last. Turn on link routing and those links ask which profile they're for (or go to the
  last-used or default profile, if you prefer).
- **Own Dock icon on macOS.** Optionally run a profile from its own copy of Claude.app (an APFS
  clone that takes almost no space and keeps Anthropic's signature), so the Dock and app switcher
  show that profile's icon while it runs.
- **Per-profile flags and environment variables**, plus flags for every profile.
- **Nothing gets deleted outright.** Signing out and removing data move folders to the system
  trash, so a mistake can be undone.
- **Updates.** Checks for a new release once a day and updates itself (or tells you the
  `brew` or `winget` command when a package manager installed it). Downloads are verified
  against the release checksums.
- **Export and import** your profile list, and shell completions for bash, zsh, fish, PowerShell
  and elvish (`claude-profile-hub completions zsh`).

## Platform notes

| | macOS | Windows | Linux |
|---|---|---|---|
| Finds Claude | /Applications, ~/Applications, Spotlight | `%LOCALAPPDATA%\AnthropicClaude` and Program Files | `claude-desktop` on PATH, common package paths, AppImages in ~/Applications, Flatpak |
| Shortcuts | `~/Applications/Claude Profile Hub/` apps, Dock pins, Desktop links | Start menu folder, Desktop | `~/.local/share/applications`, Desktop |
| Link routing | A small helper app registered for `claude://` | `HKCU\Software\Classes\claude` | `xdg-mime` |
| Profile picker dialog | Native list dialog | Native list dialog | zenity or kdialog |

Claude Desktop has no official Linux build; the tool works with the community packages. If it
can't find Claude, point it there: `claude-profile-hub config claude-path /path/to/claude`.

On Windows a second, windowless copy of the program (`claude-profile-hub-w.exe`) runs behind the
shortcuts so clicking one doesn't flash a terminal window.

## Config

Settings and profiles live in one TOML file you can edit by hand (`claude-profile-hub config path`
prints where). `claude-profile-hub config` lists every setting, and
`claude-profile-hub config <key> <value>` changes one.

```toml
[settings]
default_profile = "Work"
route_links = true
link_routing = "ask"          # ask | last-used | default
app_shortcuts = true
desktop_shortcuts = false
dock_pins = false             # macOS
follow_claude_updates = true  # reopen profiles on the new Claude after it updates
background_watcher = false    # watch for updates from login (a tiny background process)
accent = "orange"
sort = "manual"               # manual | name | recent
global_args = []

[[profiles]]
name = "Work"
data_dir = "/Users/you/Library/Application Support/dev.slukedev.claude-profile-hub/profiles/work"
color = "indigo"
badge = "work"                # a symbol, one letter, or "none"
extra_args = ["--disable-gpu"]
env = { LANG = "en_US.UTF-8" }
notes = "Company Google Workspace"
```

Set `CLAUDE_PROFILE_HUB_HOME` to keep everything (config and new profiles) in one folder.

## How it works

Claude Desktop is an Electron app, and Electron keeps everything about a session in one user data
folder. Starting Claude with `--user-data-dir=<folder>` gives that copy its own sign-in and
state. Claude Profile Hub keeps track of a folder per profile and starts Claude with it. It never
modifies or re-signs Claude itself.

Isolation depends on Claude Desktop continuing to honor `--user-data-dir`. It does today, but
Anthropic could change that.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo run --example icons icons.png   # renders every badge, for checking icon changes
```

Releases: bump the version in `Cargo.toml`, add a section to `CHANGELOG.md`, and push a tag like
`v0.2.0`. The Release workflow builds macOS (Apple silicon and Intel), Linux (x86_64 and arm64,
static) and Windows (x64 and arm64), publishes the GitHub release with `SHA256SUMS`, updates the
Homebrew tap, and opens the winget PR. It needs two secrets: `HOMEBREW_TAP_TOKEN` (push access
to `slukedev/homebrew-tap`) and `WINGET_TOKEN`. The first winget version is submitted by hand; see
[packaging/winget](packaging/winget/README.md).

## License

MIT. The letter badges use Archivo Black
(SIL Open Font License). See [NOTICE](NOTICE).
