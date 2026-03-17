#!/usr/bin/env bash
# release.sh — two-phase release wrapper around cargo-release
#
# Usage:
#   scripts/release.sh prepare <version>   # steps 0-2: branch, bump, push PR
#   scripts/release.sh finish              # step 4: tag, publish, trigger dist

set -euo pipefail

COMMAND="${1:-}"
VERSION="${2:-}"

usage() {
    echo "Usage:"
    echo "  scripts/release.sh prepare <version>   # create release branch and open PR"
    echo "  scripts/release.sh finish               # tag and publish from main"
    exit 1
}

require_clean_tree() {
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "error: working tree is not clean. Commit or stash your changes first."
        exit 1
    fi
}

case "$COMMAND" in
    prepare)
        if [ -z "$VERSION" ]; then
            echo "error: version required (e.g. scripts/release.sh prepare 0.2.0)"
            usage
        fi

        BRANCH="release/$VERSION"

        # step 0: create release branch
        echo "→ Creating branch $BRANCH"
        git checkout -b "$BRANCH"

        # step 2: bump versions, commit, push branch
        echo ""
        echo "→ Running cargo release (no publish, no tag)..."
        cargo release --no-publish --no-tag --allow-branch="$BRANCH" "$VERSION"

        echo ""
        echo "→ Opening pull request..."
        PR_URL=$(gh pr create \
            --title "chore: Release hotdata-cli version $VERSION" \
            --base main \
            --head "$BRANCH")

        echo ""
        echo "✓ PR created: $PR_URL"
        open "$PR_URL"
        echo ""
        echo "Next steps:"
        echo "  1. Review and merge the PR (use 'Squash and merge')"
        echo "  2. Run: scripts/release.sh finish"
        ;;

    finish)
        require_clean_tree

        CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
        if [ "$CURRENT_BRANCH" != "main" ]; then
            echo "→ Switching to main..."
            git checkout main
        fi

        echo "→ Pulling latest main..."
        git pull

        echo ""
        echo "→ Running cargo release (tagging release)..."
        cargo release

        echo ""
        echo "✓ Release complete. Tag pushed and dist workflow triggered."
        ;;

    *)
        usage
        ;;
esac
