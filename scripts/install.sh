#!/bin/sh
# install.sh — one installer for Linux and macOS.
#
# If Homebrew is on the machine, install (or upgrade) through the tap so the
# binary is managed by brew and `hotdata manage upgrade` follows suit.
# Otherwise fall back to the cargo-dist shell installer, which drops a
# prebuilt binary into ~/.hotdata/cli.
#
# Usage:
#   curl --proto '=https' --tlsv1.2 -LsSf https://github.com/hotdata-dev/hotdata-cli/releases/latest/download/install.sh | sh
#
# Environment:
#   HOTDATA_NO_BREW=1   skip Homebrew even when it is installed

set -eu

FORMULA="hotdata-dev/tap/cli"
REPO="https://github.com/hotdata-dev/hotdata-cli"
DIST_INSTALLER="$REPO/releases/latest/download/hotdata-cli-installer.sh"

find_brew() {
    if command -v brew >/dev/null 2>&1; then
        command -v brew
        return 0
    fi
    for candidate in /opt/homebrew/bin/brew /usr/local/bin/brew /home/linuxbrew/.linuxbrew/bin/brew; do
        if [ -x "$candidate" ]; then
            echo "$candidate"
            return 0
        fi
    done
    return 1
}

BREW=""
if [ "${HOTDATA_NO_BREW:-}" != "1" ]; then
    BREW="$(find_brew || true)"
fi

if [ -n "$BREW" ]; then
    echo "Homebrew found at $BREW; installing $FORMULA" >&2
    if "$BREW" list --formula cli >/dev/null 2>&1; then
        "$BREW" upgrade "$FORMULA"
    else
        "$BREW" install "$FORMULA"
    fi
    echo "Installed $("$BREW" --prefix)/bin/hotdata" >&2
    exit 0
fi

echo "Homebrew not found; installing a prebuilt binary to ~/.hotdata/cli" >&2
if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -LsSf "$DIST_INSTALLER" | sh
elif command -v wget >/dev/null 2>&1; then
    wget -qO- "$DIST_INSTALLER" | sh
else
    echo "error: need curl or wget to download the installer" >&2
    exit 1
fi
