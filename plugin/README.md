# Hotdata CLI plugin

This plugin teaches Claude to use the [Hotdata](https://www.hotdata.dev) CLI.
With it, Claude can manage workspaces and databases, run SQL, pull data in from
other sources with ingests, build full-text and vector search indexes, run
analytics, and work with geospatial data.

The plugin ships the `hotdata` skill. It does not ship the CLI binary. If the
CLI is missing, Claude asks before it installs anything.

## What the plugin runs, sends, and fetches

- **SessionStart hook** (`hooks/check-cli.sh`, Claude Code and Cowork only):
  checks whether `hotdata` is on `PATH` or in `~/.hotdata/cli`. If it's only in
  `~/.hotdata/cli`, it adds that folder to `PATH` for the session. If it's
  missing, it asks Claude to offer to install the CLI. The hook makes no
  network requests, installs nothing, and sends no data.
- **Installer**, only after you agree: `brew install hotdata-dev/tap/cli` when
  `brew` is present. Otherwise Claude downloads `https://hotdata.dev/install` to
  a file, shows it to you, and runs it with `HOTDATA_NO_SKILLS=1`. It installs
  the CLI from the
  [hotdata-cli GitHub releases](https://github.com/hotdata-dev/hotdata-cli/releases).
- **The skill** teaches Claude to run `hotdata` commands. Those commands talk to
  the Hotdata API (`https://api.hotdata.dev/v1`, or `HOTDATA_API_URL`) with your
  Hotdata credentials, and send the queries, files, and data source settings
  you ask Claude to work with. `hotdata auth login` opens
  `https://app.hotdata.dev` in your browser, and the CLI checks the hotdata-cli
  GitHub releases for new versions (`HOTDATA_NO_UPDATE_CHECK=1` turns that off).
  Neither the plugin nor the CLI collects telemetry.

## Links

- [Documentation](https://www.hotdata.dev/docs) and the
  [CLI reference](https://www.hotdata.dev/docs/cli-reference)
- [Source](https://github.com/hotdata-dev/hotdata-cli)
- [Privacy policy](https://www.hotdata.dev/privacy-policy) and
  [terms of service](https://www.hotdata.dev/terms-of-service)
- License: MIT
