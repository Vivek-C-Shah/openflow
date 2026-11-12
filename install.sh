#!/bin/sh
# OpenFlow installer for macOS and Linux.
#   curl -fsSL https://raw.githubusercontent.com/Vivek-C-Shah/openflow/main/install.sh | sh
set -eu
REPO="${OPENFLOW_REPO:-Vivek-C-Shah/openflow}"
DIR="$HOME/.local/bin"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  TARGET="macos-arm64" ;;
  Darwin-x86_64) TARGET="macos-x64" ;;
  Linux-x86_64)  TARGET="linux-x64" ;;
  *) echo "Unsupported platform: $(uname -s) $(uname -m)"; exit 1 ;;
esac

printf '\n  Installing OpenFlow (%s) to %s\n' "$TARGET" "$DIR"
URL=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | grep -o "\"browser_download_url\": *\"[^\"]*$TARGET.tar.gz\"" | head -1 | sed 's/.*"\(https[^"]*\)"/\1/')
[ -n "$URL" ] || { echo "No $TARGET build in the latest release of $REPO"; exit 1; }

TMP=$(mktemp -d)
curl -fsSL "$URL" -o "$TMP/openflow.tar.gz"
[ -x "$DIR/openflow" ] && "$DIR/openflow" stop >/dev/null 2>&1 || true
mkdir -p "$DIR"
tar -xzf "$TMP/openflow.tar.gz" -C "$DIR"
rm -rf "$TMP"
chmod +x "$DIR/openflow" "$DIR/openflow-daemon"

case ":$PATH:" in
  *":$DIR:"*) ;;
  *) echo "  Add $DIR to your PATH (e.g. in ~/.zshrc or ~/.bashrc): export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac

"$DIR/openflow" setup
