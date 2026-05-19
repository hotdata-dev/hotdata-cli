#!/usr/bin/env bash
# release.sh — two-phase release wrapper around cargo-release
#
# Usage:
#   scripts/release.sh prepare <version>   # branch, bump, changelog PR
#   scripts/release.sh finish              # tag only (main is branch-protected)

set -euo pipefail

COMMAND="${1:-}"
VERSION="${2:-}"

usage() {
    echo "Usage:"
    echo "  scripts/release.sh prepare <version>   # create release branch and open PR"
    echo "  scripts/release.sh finish               # push v<version> tag from main (no main push)"
    exit 1
}

require_clean_tree() {
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "error: working tree is not clean. Commit or stash your changes first."
        exit 1
    fi
}

read_crate_version() {
    local ver
    ver="$(grep -E '^version = ' Cargo.toml | head -1 | sed -E 's/^version = "([^"]+)".*/\1/')"
    if [ -z "$ver" ]; then
        echo "error: could not read version from Cargo.toml" >&2
        exit 1
    fi
    printf '%s' "$ver"
}

case "$COMMAND" in
    prepare)
        if [ -z "$VERSION" ]; then
            echo "error: version required (e.g. scripts/release.sh prepare 0.2.0)"
            usage
        fi

        BRANCH="release/$VERSION"

        require_clean_tree

        echo "→ Creating branch $BRANCH"
        git checkout -b "$BRANCH"

        echo ""
        echo "→ Running cargo release (no publish, no tag)..."
        export PATH="${HOME}/.cargo/bin:${PATH}"
        cargo release --no-publish --no-tag --no-confirm --allow-branch="$BRANCH" --execute "$VERSION"

        if [ -f scripts/validate-changelog.py ]; then
            echo ""
            echo "→ Validating CHANGELOG.md against origin/main..."
            git fetch origin main 2>/dev/null || true
            python3 scripts/validate-changelog.py origin/main
        fi

        echo ""
        echo "→ Opening pull request..."
        PR_URL=$(gh pr create \
            --title "chore: Release hotdata-cli version $VERSION" \
            --body "" \
            --base main \
            --head "$BRANCH")

        echo ""
        echo "✓ PR created: $PR_URL"
        if command -v xdg-open &>/dev/null; then
            xdg-open "$PR_URL" || true
        elif command -v open &>/dev/null; then
            open "$PR_URL" || true
        fi
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
        git pull origin main

        VERSION="$(read_crate_version)"
        TAG="v${VERSION}"

        echo ""
        echo "→ Release version from Cargo.toml: $VERSION (tag $TAG)"

        if git rev-parse "$TAG" >/dev/null 2>&1; then
            echo "error: tag $TAG already exists locally. Delete it or pick a new version." >&2
            exit 1
        fi

        if git ls-remote --exit-code --tags origin "refs/tags/${TAG}" >/dev/null 2>&1; then
            echo "error: tag $TAG already exists on origin." >&2
            exit 1
        fi

        echo "→ Creating annotated tag $TAG (no commit to main)..."
        git tag -a "$TAG" -m "Release hotdata-cli version $VERSION"

        echo "→ Pushing tag to origin..."
        git push origin "$TAG"

        echo ""
        echo "✓ Tag $TAG pushed. Dist/release workflow should run on GitHub."
        echo "  (main was not pushed — version bump must already be merged via release PR.)"
        ;;

    *)
        usage
        ;;
esac
