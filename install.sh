#!/bin/sh
# Install kimo from the latest GitHub release.
#   curl -fsSL https://raw.githubusercontent.com/OWNER/kimo/main/install.sh | sh
#   … | sh -s -- --server        # without the window (headless boxes, Raspberry Pi)
# Environment: KIMO_REPO (owner/name), KIMO_VERSION (tag, default latest), KIMO_BIN_DIR.
set -eu

REPO="${KIMO_REPO:-vallverdu/kimo}"
BIN_DIR="${KIMO_BIN_DIR:-$HOME/.local/bin}"
VARIANT="kimo"
for arg in "$@"; do
  case "$arg" in
    --server) VARIANT="kimo-server" ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

case "$(uname -s)" in
  Linux) os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin"; VARIANT="kimo" ;;
  *) echo "kimo runs on Linux and macOS" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) echo "no kimo build for $(uname -m): build from source with cargo" >&2; exit 1 ;;
esac

name="$VARIANT-$arch-$os"
if [ -n "${KIMO_VERSION:-}" ]; then
  base="https://github.com/$REPO/releases/download/$KIMO_VERSION"
else
  base="https://github.com/$REPO/releases/latest/download"
fi
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "downloading $name"
curl -fsSL "$base/$name.tar.gz" -o "$tmp/kimo.tar.gz"
# Every release publishes a checksum: without it, nothing is installed.
curl -fsSL "$base/$name.tar.gz.sha256" -o "$tmp/kimo.sha256" \
  || { echo "cannot download the checksum of $name: not installed" >&2; exit 1; }
expected="$(cut -d' ' -f1 "$tmp/kimo.sha256")"
actual="$( (sha256sum "$tmp/kimo.tar.gz" 2>/dev/null || shasum -a 256 "$tmp/kimo.tar.gz") | cut -d' ' -f1)"
[ -n "$expected" ] && [ "$expected" = "$actual" ] || { echo "checksum mismatch: not installed" >&2; exit 1; }
tar -xzf "$tmp/kimo.tar.gz" -C "$tmp"
mkdir -p "$BIN_DIR"
# Next to the old one, then renamed over it: a running kimo keeps its own file.
install -m 755 "$tmp/$name/kimo" "$BIN_DIR/.kimo-new"
mv -f "$BIN_DIR/.kimo-new" "$BIN_DIR/kimo"
echo "installed $BIN_DIR/kimo"
# The fleet coordinator (for the always-on machine of a fleet; unused otherwise).
if [ -f "$tmp/$name/kimo-fleet" ]; then
  install -m 755 "$tmp/$name/kimo-fleet" "$BIN_DIR/.kimo-fleet-new"
  mv -f "$BIN_DIR/.kimo-fleet-new" "$BIN_DIR/kimo-fleet"
fi
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) echo "add $BIN_DIR to your PATH" ;; esac
# macOS: kimo.app in Applications (Dock, Launchpad, Spotlight).
if [ "$(uname -s)" = "Darwin" ]; then
  "$BIN_DIR/kimo" install-app || echo "kimo.app not set up: run kimo install-app later"
  echo "next: open kimo from Launchpad or Spotlight (or run kimo)"
elif [ "$VARIANT" = "kimo-server" ]; then
  echo "next: kimo serve   (then pair your phone: kimo pair)"
else
  echo "next: kimo         (opens the window; kimo serve runs without it)"
fi
