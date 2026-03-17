# hotdata-cli

Command line interface for [Hotdata](https://www.hotdata.dev).

## Installation

Build from source (requires Rust):

```sh
cargo build --release
cp target/release/hotdata /usr/local/bin/hotdata
```

## Getting Started

```sh
# Initialize config
hotdata init

# Log in via browser
hotdata auth login

# Check auth status
hotdata auth status
```

## Commands

### Workspaces

```sh
hotdata workspace list [--format table|json|yaml]
```

### Connections

```sh
hotdata connections list <workspace_id> [--format table|json|yaml]
```

### Tables

```sh
# List all tables in a workspace
hotdata tables list <workspace_id> [--format table|json|yaml]

# List columns for a specific connection
hotdata tables list <workspace_id> --connection-id <connection_id> [--format table|json|yaml]
```

### Query

```sh
hotdata query "<sql>" --workspace-id <workspace_id> [--connection <connection_id>] [--format table|json|csv]
```

## Releasing

Releases use a two-phase workflow wrapping [`cargo-release`](https://github.com/crate-ci/cargo-release).

**Phase 1 — prepare**

```sh
scripts/release.sh prepare <version>
# e.g. scripts/release.sh prepare 0.2.0
```

This will:
1. Create a `release/<version>` branch
2. Bump the version in `Cargo.toml`, update `CHANGELOG.md`, and push the branch
3. Open a GitHub pull request and launch it in the browser

Squash and merge the PR into `main` when ready.

**Phase 2 — finish**

```sh
scripts/release.sh finish
```

Run this from any branch after the PR is merged. It will switch to `main`, pull the latest, tag the release, and trigger the dist workflow.

## Configuration

Config is stored at `~/.hotdata/config.yml` keyed by profile (default: `default`).

Environment variable overrides:

| Variable           | Description                              |
|--------------------|------------------------------------------|
| `HOTDATA_API_KEY`  | API key (overrides config)               |
| `HOTDATA_API_URL`  | API base URL (default: `https://api.hotdata.dev/v1`) |
| `HOTDATA_APP_URL`  | App URL for browser login (default: `https://app.hotdata.dev`) |
