#!/bin/sh
# Installs claude-profile-hub from the latest GitHub release (macOS and Linux).
#
#   curl -fsSL https://raw.githubusercontent.com/slukedev/claude-profile-hub/main/install.sh | sh
#
# Settings (environment variables):
#   CPH_INSTALL_DIR   where to put the binary (default: ~/.local/bin)
#   CPH_VERSION       a release tag such as v0.1.0 (default: the latest)
#   CPH_BASE_URL      download from a mirror instead of GitHub releases
set -eu

REPO="slukedev/claude-profile-hub"
BIN="claude-profile-hub"
DIR="${CPH_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-musl" ;;
  *) die "this script is for macOS and Linux. On Windows run: irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) die "unsupported processor: $(uname -m)" ;;
esac
# Under Rosetta, uname reports x86_64 on an Apple silicon Mac; the native build is better.
if [ "$os" = "apple-darwin" ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
  arch="aarch64"
fi
target="$arch-$os"

if [ -n "${CPH_BASE_URL:-}" ]; then
  base="$CPH_BASE_URL"
elif [ -n "${CPH_VERSION:-}" ]; then
  base="https://github.com/$REPO/releases/download/$CPH_VERSION"
else
  base="https://github.com/$REPO/releases/latest/download"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
else
  die "needs curl or wget"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading $BIN for $target..."
fetch "$base/$BIN-$target" "$tmp/$BIN" || die "no release build for $target"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" || die "couldn't download SHA256SUMS"

want=$(awk -v f="$BIN-$target" '$2 == f || $2 == "*" f {print $1}' "$tmp/SHA256SUMS")
if command -v sha256sum >/dev/null 2>&1; then
  got=$(sha256sum "$tmp/$BIN" | awk '{print $1}')
else
  got=$(shasum -a 256 "$tmp/$BIN" | awk '{print $1}')
fi
[ -n "$want" ] && [ "$want" = "$got" ] || die "checksum mismatch; nothing was installed"

mkdir -p "$DIR"
chmod 755 "$tmp/$BIN"
mv "$tmp/$BIN" "$DIR/$BIN"
say "Installed $DIR/$BIN"

case ":$PATH:" in
  *":$DIR:"*) ;;
  *) say "Add $DIR to your PATH, for example: echo 'export PATH=\"$DIR:\$PATH\"' >> ~/.zshrc" ;;
esac
say "Run '$BIN' to set up your profiles."
