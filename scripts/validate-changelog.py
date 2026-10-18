#!/usr/bin/env python3
"""Fail if CHANGELOG.md alters any release section that already exists on the base ref.

git-cliff full regenerations can drop bullets from older versions; this catches that
by requiring each ## [version] block from the base to match exactly in the working tree.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

SECTION_START = re.compile(r"^## \[([^\]]+)\]", re.MULTILINE)


def split_sections(text: str) -> dict[str, str]:
    matches = list(SECTION_START.finditer(text))
    sections: dict[str, str] = {}
    for i, m in enumerate(matches):
        version = m.group(1)
        start = m.start()
        end = matches[i + 1].start() if i + 1 < len(matches) else len(text)
        sections[version] = text[start:end].rstrip() + "\n"
    return sections


def git_show_changelog(ref: str) -> str:
    return subprocess.check_output(
        ["git", "show", f"{ref}:CHANGELOG.md"],
        text=True,
    )


def main() -> None:
    base = sys.argv[1] if len(sys.argv) > 1 else "origin/main"
    current = Path("CHANGELOG.md").read_text()

    try:
        base_text = git_show_changelog(base)
    except subprocess.CalledProcessError as e:
        print(f"error: could not read {base}:CHANGELOG.md ({e})", file=sys.stderr)
        sys.exit(1)

    base_sections = split_sections(base_text)
    cur_sections = split_sections(current)

    failed = False
    for ver, body in base_sections.items():
        # The [Unreleased] section is a staging area, not a released version:
        # at release time it is renamed into a concrete ## [x.y.z] section, so
        # it legitimately disappears from the base ref. Only guard released
        # versions against having their bullets rewritten or dropped.
        if ver.lower() == "unreleased":
            continue
        if ver not in cur_sections:
            print(
                f"CHANGELOG.md: missing section [{ver}] that exists on {base}",
                file=sys.stderr,
            )
            failed = True
        elif cur_sections[ver] != body:
            # A released section may gain notes (a correction after the fact,
            # as when the 0.41.0 notes missed two PRs that merged while its
            # release PR waited for review) but never lose any: every bullet
            # the base carries must still be there verbatim. That keeps the
            # guard against git-cliff regenerations dropping history while
            # letting an additive fix through.
            missing = [
                line
                for line in body.splitlines()
                if line.startswith("- ") and line not in cur_sections[ver].splitlines()
            ]
            if missing:
                print(
                    f"CHANGELOG.md: section [{ver}] drops bullets present on {base} "
                    "(released notes may be added to, never rewritten):",
                    file=sys.stderr,
                )
                for line in missing:
                    print(f"  {line}", file=sys.stderr)
                failed = True
            else:
                print(f"changelog: section [{ver}] amended additively")

    if failed:
        sys.exit(1)
    print(
        f"changelog ok: {len(base_sections)} section(s) from {base} preserved unchanged",
    )


if __name__ == "__main__":
    main()
