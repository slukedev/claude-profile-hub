#!/bin/sh
# Prints the Homebrew formula for a release: render-formula.sh VERSION SHA256SUMS
set -eu
version="$1"
sums="$2"
sha() { awk -v f="claude-profile-hub-$1.tar.gz" '$2 == f || $2 == "*" f {print $1}' "$sums"; }
sed -e "s/@VERSION@/$version/g" \
  -e "s/@SHA_MAC_ARM@/$(sha aarch64-apple-darwin)/" \
  -e "s/@SHA_MAC_INTEL@/$(sha x86_64-apple-darwin)/" \
  -e "s/@SHA_LINUX_ARM@/$(sha aarch64-unknown-linux-musl)/" \
  -e "s/@SHA_LINUX_INTEL@/$(sha x86_64-unknown-linux-musl)/" \
  "$(dirname "$0")/../packaging/homebrew/claude-profile-hub.rb.in"
