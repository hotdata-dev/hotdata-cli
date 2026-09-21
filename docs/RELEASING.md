# Releasing

Releases use a two-phase workflow wrapping [`cargo-release`](https://github.com/crate-ci/cargo-release).

**Phase 1 — prepare**

```sh
scripts/release.sh prepare <version>
```

Runs the skill check (below), then creates a `release/<version>` branch, bumps the version (including the `version:` line in every `SKILL.md`), updates `CHANGELOG.md`, pushes the branch, and opens a pull request.

**Phase 2 — finish**

```sh
scripts/release.sh finish
```

Switches to `main`, pulls latest, runs the skill check with `--require-version`, tags the release, and triggers the dist workflow.

## Skill check

Both phases run `scripts/check-skills.sh`. The agent skills under `skills/` ship in every release (`skills.tar.gz`, `SKILL.md`), so the script refuses to release when they lag the code:

- **Drift** — if `src/` or `README.md` changed since the last `v*` tag and `skills/` did not, the run fails and lists the commits to review. Update the skills and commit, or set `SKIP_SKILL_DRIFT=1` when the release is verified to be skill-neutral.
- **Coverage** — every subcommand the built binary exposes must appear as `hotdata <group> <sub>` somewhere in `skills/**/*.md`.
- **Version** (`finish` only) — every `SKILL.md` frontmatter `version:` must equal the crate version.

The drift check is a reminder, not a judge of prose. Before `prepare`, read the commits since the last tag and update `skills/hotdata/SKILL.md` and the subskills to match the current `--help` output and behavior.
