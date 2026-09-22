#!/usr/bin/env bash
# check-skills.sh — refuse to release with agent skills that lag the code.
#
# Runs before a release branch is cut and again before the tag is pushed.
# Three checks, all mechanical:
#
#   1. Drift: if src/ changed since the last release tag, skills/ must have
#      changed too. The script cannot judge prose, so this is the reminder
#      that a code change needs its SKILL.md follow-up. Override with
#      SKIP_SKILL_DRIFT=1 when a release is verified to be skill-neutral.
#   2. Coverage: every subcommand the built binary exposes, and every long
#      flag in its --help, must be named in skills/**/*.md, so a new command
#      or flag cannot ship undocumented. Hidden flags are not in --help and
#      are not checked.
#   3. Version: every SKILL.md frontmatter `version:` must equal Cargo.toml
#      (finish phase only — prepare runs before cargo-release bumps them).
#
# Usage:
#   scripts/check-skills.sh [--require-version]

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

REQUIRE_VERSION=0
[ "${1:-}" = "--require-version" ] && REQUIRE_VERSION=1

BIN="${HOTDATA_BIN:-target/debug/hotdata}"
SKILL_FILES=(skills/hotdata/SKILL.md skills/hotdata/subskills/*/SKILL.md)
fail=0

# --- 1. drift ---------------------------------------------------------------
LAST_TAG="$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)"
if [ -z "$LAST_TAG" ]; then
    echo "→ skills: no previous v* tag; skipping drift check"
else
    src_changes="$(git log --oneline "$LAST_TAG"..HEAD -- src/ README.md | grep -v -E 'chore\(deps\)|chore: Release' || true)"
    skill_changes="$(git log --oneline "$LAST_TAG"..HEAD -- skills/ || true)"
    if [ -n "$src_changes" ] && [ -z "$skill_changes" ]; then
        if [ "${SKIP_SKILL_DRIFT:-}" = "1" ]; then
            echo "→ skills: code changed since $LAST_TAG without a skills change (SKIP_SKILL_DRIFT=1, continuing)"
        else
            echo "error: code changed since $LAST_TAG but skills/ did not." >&2
            echo "" >&2
            echo "$src_changes" | sed 's/^/    /' >&2
            echo "" >&2
            echo "Review each commit against skills/hotdata/SKILL.md and the subskills, update them," >&2
            echo "and commit. If none of these change agent-visible behavior, rerun with SKIP_SKILL_DRIFT=1." >&2
            fail=1
        fi
    else
        echo "→ skills: drift check ok (since $LAST_TAG)"
    fi
fi

# --- 2. coverage ------------------------------------------------------------
if [ ! -x "$BIN" ]; then
    echo "→ skills: building $BIN for the command inventory..."
    cargo build -q
fi

# Walk the clap tree: "<group> <sub> [<sub>]", leaf commands only.
list_subcommands() {
    "$BIN" "$@" --help 2>/dev/null \
        | awk '/^Commands:/{f=1;next} /^$/{f=0} f && $1!="help" {print $1}'
}
leaves=()
walk() {
    local path=("$@")
    local subs
    subs="$(list_subcommands "${path[@]}")"
    if [ -z "$subs" ]; then
        [ ${#path[@]} -gt 0 ] && leaves+=("${path[*]}")
        return
    fi
    local s
    for s in $subs; do walk "${path[@]}" "$s"; done
}
walk

skill_text="$(cat "${SKILL_FILES[@]}" skills/hotdata/references/*.md skills/hotdata/subskills/*/references/*.md)"
missing=()
for leaf in "${leaves[@]}"; do
    if ! grep -qF "hotdata $leaf" <<<"$skill_text"; then
        missing+=("$leaf")
    fi
done
if [ ${#missing[@]} -gt 0 ]; then
    echo "error: commands the CLI exposes but no skill mentions:" >&2
    printf '    hotdata %s\n' "${missing[@]}" >&2
    fail=1
else
    echo "→ skills: command coverage ok (${#leaves[@]} commands documented)"
fi

# Long flags, per leaf, minus the globals every command carries.
GLOBAL_FLAGS='^--(api-key|no-input|help|output|workspace-id)$'
missing_flags=()
flag_count=0
for leaf in "${leaves[@]}"; do
    # shellcheck disable=SC2086
    flags="$("$BIN" $leaf --help 2>/dev/null \
        | grep -oE '^\s+(-[a-zA-Z], )?--[a-z][a-z0-9-]+' \
        | grep -oE -- '--[a-z][a-z0-9-]+' | sort -u | grep -vE "$GLOBAL_FLAGS" || true)"
    for f in $flags; do
        flag_count=$((flag_count + 1))
        grep -qF -- "$f" <<<"$skill_text" || missing_flags+=("hotdata $leaf $f")
    done
done
if [ ${#missing_flags[@]} -gt 0 ]; then
    echo "error: flags in --help that no skill mentions:" >&2
    printf '    %s\n' "${missing_flags[@]}" >&2
    fail=1
else
    echo "→ skills: flag coverage ok ($flag_count flags documented)"
fi

# --- 3. version -------------------------------------------------------------
if [ "$REQUIRE_VERSION" = 1 ]; then
    crate="$(grep -E '^version = ' Cargo.toml | head -1 | sed -E 's/^version = "([^"]+)".*/\1/')"
    for f in "${SKILL_FILES[@]}"; do
        v="$(sed -n 's/^version: //p' "$f" | head -1)"
        if [ "$v" != "$crate" ]; then
            echo "error: $f declares version $v, Cargo.toml is $crate" >&2
            fail=1
        fi
    done
    [ "$fail" = 0 ] && echo "→ skills: version ok ($crate)"
fi

exit $fail
