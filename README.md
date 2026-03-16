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

## Configuration

Config is stored at `~/.hotdata/config.yml` keyed by profile (default: `default`).

Environment variable overrides:

| Variable           | Description                              |
|--------------------|------------------------------------------|
| `HOTDATA_API_KEY`  | API key (overrides config)               |
| `HOTDATA_API_URL`  | API base URL (default: `https://api.hotdata.dev/v1`) |
| `HOTDATA_APP_URL`  | App URL for browser login (default: `https://app.hotdata.dev`) |
