#!/usr/bin/env bash
# SessionStart hook for the hotdata-cli Claude Code plugin.
#
# The plugin ships the skill, not the binary. Stay silent when the CLI is
# reachable. When it isn't, stdout becomes session context asking Claude to
# offer an install rather than run one: installing software on every session
# start is not something users agreed to by installing a skill.
#
# The install command itself lives in the skill, not here, so this hook runs
# nothing and fetches nothing that the directory's validator has to follow.

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

cat <<'EOF'
The hotdata CLI is not installed. Before running any hotdata command, tell the
user and ask whether to install it. If they agree, follow the install steps in
the hotdata skill (the "Upgrade" section of skills/hotdata/SKILL.md), and set
HOTDATA_NO_SKILLS=1 because this plugin already provides the skill.
EOF
exit 0
