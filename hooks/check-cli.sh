#!/usr/bin/env bash
# SessionStart hook for the hotdata-cli Claude Code plugin.
#
# The plugin ships the skill, not the binary. Stay silent when the CLI is
# reachable. When it isn't, stdout becomes session context asking Claude to
# offer the installer rather than run it: piping curl into sh on every session
# start is not something users agreed to by installing a skill.

set -u

DIST_BIN_DIR="$HOME/.hotdata/cli"

if command -v hotdata >/dev/null 2>&1; then
    exit 0
fi

# Without Homebrew the installer puts the binary in ~/.hotdata/cli, which is
# not on PATH until the user's shell profile is reloaded.
if [ -x "$DIST_BIN_DIR/hotdata" ]; then
    if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
        echo "export PATH=\"$DIST_BIN_DIR:\$PATH\"" >> "$CLAUDE_ENV_FILE"
    fi
    exit 0
fi

# HOTDATA_NO_SKILLS=1: the plugin already provides the skill, so the installer
# must not add a second copy to ~/.claude/skills.
cat <<'EOF'
The hotdata CLI is not installed. Before running any hotdata command, tell the
user and ask whether to install it. If they agree, run:

  curl -fsSL https://hotdata.dev/install | HOTDATA_NO_SKILLS=1 sh

If the binary lands in ~/.hotdata/cli, call it as ~/.hotdata/cli/hotdata for the
rest of this session.
EOF
exit 0
