#!/usr/bin/env python3
"""Fail unless CHANGELOG.md carries real release notes for a version.

    scripts/check-release-notes.py <version> [--since <ref>]

Two checks:

  * The `## [<version>]` section exists and has at least one bullet. git-cliff
    writes an empty section when nothing landed since the last tag, and an
    empty section ships as an empty GitHub Release (0.42.0 did).
  * With --since <ref>: every pull request merged between <ref> and HEAD is
    mentioned in the section by its `(#N)` number, release commits aside.
    The changelog is generated when the release PR opens; anything merged
    while that PR waits for review ends up inside the tag but outside its
    notes, and the next release's git-cliff run (which starts at the tag)
    never sees it either. The finish step runs this against the previous
    tag so such a release is refused before it is tagged.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

SECTION_START = re.compile(r"^## \[([^\]]+)\]", re.MULTILINE)
RELEASE_COMMIT = re.compile(r"^chore: Release hotdata-cli version")
PR_NUMBER = re.compile(r"\(#(\d+)\)")


def section_for(text: str, version: str) -> str | None:
    matches = list(SECTION_START.finditer(text))
    for i, m in enumerate(matches):
        if m.group(1) == version:
            end = matches[i + 1].start() if i + 1 < len(matches) else len(text)
            line_end = text.find("\n", m.end())   # skip the rest of the heading (" - <date>")
            return "" if line_end < 0 or line_end >= end else text[line_end + 1:end]
    return None


def main() -> None:
    args = sys.argv[1:]
    if not args:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    version = args[0]
    since = args[args.index("--since") + 1] if "--since" in args else None

    body = section_for(Path("CHANGELOG.md").read_text(), version)
    if body is None:
        print(f"CHANGELOG.md: no section [{version}]", file=sys.stderr)
        sys.exit(1)
    bullets = [l for l in body.splitlines() if l.lstrip().startswith("- ")]
    if not bullets:
        print(
            f"CHANGELOG.md: section [{version}] has no release notes. Nothing landed "
            "since the previous tag, so there is nothing to release.",
            file=sys.stderr,
        )
        sys.exit(1)

    if since:
        subjects = subprocess.check_output(
            ["git", "log", "--format=%s", f"{since}..HEAD"], text=True
        ).splitlines()
        noted = set(PR_NUMBER.findall(body))
        missing = [
            s for s in subjects
            if not RELEASE_COMMIT.match(s)
            and (m := PR_NUMBER.search(s)) and m.group(1) not in noted
        ]
        if missing:
            print(
                f"CHANGELOG.md: section [{version}] misses pull requests merged since {since}:",
                file=sys.stderr,
            )
            for s in missing:
                print(f"  {s}", file=sys.stderr)
            print(
                "These merged after the release PR was opened. Cut the release again "
                "from the current main so git-cliff includes them.",
                file=sys.stderr,
            )
            sys.exit(1)

    print(f"release notes ok: [{version}] has {len(bullets)} bullet(s)")


if __name__ == "__main__":
    main()
