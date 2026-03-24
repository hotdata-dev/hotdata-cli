# hotdata-cli

Command line interface for [Hotdata](https://www.hotdata.dev).

## Install

**Homebrew**

```sh
brew install hotdata-dev/tap/hotdata-cli
```

**Binary (macOS, Linux)**

Download a binary from [Releases](https://github.com/hotdata-dev/hotdata-cli/releases).

**Build from source** (requires Rust)

```sh
cargo build --release
cp target/release/hotdata /usr/local/bin/hotdata
```

## Connect

Run the following command to authenticate:

```sh
hotdata auth
```

This launches a browser window where you can authorize the CLI to access your Hotdata account.

Alternatively, authenticate with an API key using the `--api-key` flag:

```sh
hotdata <command> --api-key <api_key>
```

Or set the `HOTDATA_API_KEY` environment variable (also loaded from `.env` files):

```sh
export HOTDATA_API_KEY=<api_key>
hotdata <command>
```

API key priority (lowest to highest): config file → `HOTDATA_API_KEY` env var → `--api-key` flag.

## Commands

| Command | Subcommands | Description |
| :-- | :-- | :-- |
| `auth` | `status`, `logout` | Authenticate (run without subcommand to log in) |
| `workspaces` | `list`, `set`, `get`, `create`, `update` | Manage workspaces |
| `connections` | `list`, `get`, `create`, `refresh`, `update`, `delete`, `new` | Manage connections |
| `tables` | `list` | List tables and columns |
| `datasets` | `list`, `create` | Manage uploaded datasets |
| `query` | | Execute a SQL query |
| `results` | `list` | Retrieve stored query results |
| `skills` | `install`, `status` | Manage the hotdata-cli agent skill |

## Global options

| Option | Description | Type | Default |
| :-- | :-- | :-- | :-- |
| `--api-key` | API key (overrides env var and config) | string | |
| `-v, --version` | Print version | boolean | |
| `-h, --help` | Print help | boolean | |

## Workspaces

```sh
hotdata workspaces list [--format table|json|yaml]
hotdata workspaces set [<workspace_id>]
```

- `list` shows all workspaces with a `*` marker on the active one.
- `set` switches the active workspace. Omit the ID for interactive selection.
- The active workspace is used as the default for all commands that accept `--workspace-id`.

## Connections

```sh
hotdata connections list [--workspace-id <id>] [--format table|json|yaml]
hotdata connections get <connection_id> [--workspace-id <id>] [--format yaml|json|table]
hotdata connections refresh <connection_id> [--workspace-id <id>]
hotdata connections new [--workspace-id <id>]
```

- `list` returns `id`, `name`, `source_type` for each connection.
- `refresh` triggers a schema refresh for a connection.
- `new` launches an interactive connection creation wizard.

### Create a connection

```sh
# List available connection types
hotdata connections create list [--format table|json|yaml]

# Inspect schema for a connection type
hotdata connections create list <type_name> --format json

# Create a connection
hotdata connections create --name "my-conn" --type postgres --config '{"host":"...","port":5432,...}'
```

## Tables

```sh
hotdata tables list [--workspace-id <id>] [--connection-id <id>] [--schema <pattern>] [--table <pattern>] [--limit <n>] [--cursor <token>] [--format table|json|yaml]
```

- Without `--connection-id`: lists all tables with `table`, `synced`, `last_sync`.
- With `--connection-id`: includes column details (`column`, `data_type`, `nullable`).
- `--schema` and `--table` support SQL `%` wildcard patterns.
- Tables are displayed as `<connection>.<schema>.<table>` — use this format in SQL queries.

## Datasets

```sh
hotdata datasets list [--workspace-id <id>] [--limit <n>] [--offset <n>] [--format table|json|yaml]
hotdata datasets <dataset_id> [--workspace-id <id>] [--format table|json|yaml]
hotdata datasets create --file data.csv [--label "My Dataset"] [--table-name my_dataset]
hotdata datasets create --sql "SELECT ..." --label "My Dataset"
```

- Datasets are queryable as `datasets.main.<table_name>`.
- `--file`, `--sql`, and `--query-id` are mutually exclusive.
- Format is auto-detected from file extension or content.
- Piped stdin is supported: `cat data.csv | hotdata datasets create --label "My Dataset"`

## Query

```sh
hotdata query "<sql>" [--workspace-id <id>] [--connection <connection_id>] [--format table|json|csv]
```

- Default format is `table`, which prints results with row count and execution time.
- Use `--connection` to scope the query to a specific connection.

## Results

```sh
hotdata results <result_id> [--workspace-id <id>] [--format table|json|csv]
hotdata results list [--workspace-id <id>] [--limit <n>] [--offset <n>] [--format table|json|yaml]
```

- Query results include a `result-id` in the table footer — use it to retrieve past results without re-running queries.

## Configuration

Config is stored at `~/.hotdata/config.yml` keyed by profile (default: `default`).

| Variable | Description | Default |
| :-- | :-- | :-- |
| `HOTDATA_API_KEY` | API key (overrides config file) | |
| `HOTDATA_API_URL` | API base URL | `https://api.hotdata.dev/v1` |
| `HOTDATA_APP_URL` | App URL for browser login | `https://app.hotdata.dev` |

## Releasing

Releases use a two-phase workflow wrapping [`cargo-release`](https://github.com/crate-ci/cargo-release).

**Phase 1 — prepare**

```sh
scripts/release.sh prepare <version>
```

Creates a `release/<version>` branch, bumps the version, updates `CHANGELOG.md`, pushes the branch, and opens a pull request.

**Phase 2 — finish**

```sh
scripts/release.sh finish
```

Switches to `main`, pulls latest, tags the release, and triggers the dist workflow.
