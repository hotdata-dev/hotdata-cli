# Releasing

Releases run in CI. The `Cut Release` workflow (`.github/workflows/cut-release.yml`) calls the same two-phase script, `scripts/release.sh`, that wraps [`cargo-release`](https://github.com/crate-ci/cargo-release).

## Normal path

```sh
gh workflow run cut-release.yml -f version=0.40.0
```

1. **prepare** — the workflow runs `scripts/release.sh prepare 0.40.0`: skill check (below), `release/0.40.0` branch, version bump (including the `version:` line in every `SKILL.md`), `CHANGELOG.md` update, push, and a pull request. The job then enables auto-merge on the PR.
2. **merge** — the PR squash-merges on its own once CI and the review gate pass.
3. **finish** — the merge triggers the workflow again, which runs `scripts/release.sh finish` on `main`: skill check with `--require-version`, annotated `v0.40.0` tag, push. The tag triggers the dist workflow (`release.yml`), which builds the binaries, publishes the GitHub Release, and updates the Homebrew formula.

The workflow acts as the `hotdata-automation` GitHub App. A PR opened or a tag pushed with the default `GITHUB_TOKEN` would trigger no other workflows.

If the `prepare` job fails on the skill check, the skills lag the code: update `skills/` on `main` first (see below), then dispatch again. CI does not set `SKIP_SKILL_DRIFT=1`.

## Manual path

The script still works locally with the same two phases:

```sh
scripts/release.sh prepare <version>   # branch, bump, changelog, PR
scripts/release.sh finish              # on main after the PR merges: tag and push
```

If you run `prepare` locally, the merge of its `release/<version>` PR still triggers the workflow's `finish` job, so do not run `finish` by hand as well.

## Skill check

Both phases run `scripts/check-skills.sh`. The agent skills under `skills/` ship in every release (`skills.tar.gz`, `SKILL.md`), so the script refuses to release when they lag the code:

- **Drift** — if `src/` or `README.md` changed since the last `v*` tag and `skills/` did not, the run fails and lists the commits to review. Update the skills and commit, or set `SKIP_SKILL_DRIFT=1` (local runs only) when the release is verified to be skill-neutral.
- **Coverage** — every subcommand the built binary exposes must appear as `hotdata <group> <sub>` somewhere in `skills/**/*.md`, and every long flag in its `--help` must be mentioned too. CI runs this check on every pull request (with the drift check skipped), so a new command or flag cannot merge undocumented.
- **Version** (`finish` only) — every `SKILL.md` frontmatter `version:` must equal the crate version.

The drift check is a reminder, not a judge of prose. Before cutting a release, read the commits since the last tag and update `skills/hotdata/SKILL.md` and the subskills to match the current `--help` output and behavior.
