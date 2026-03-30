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
| `workspaces` | `list`, `set` | Manage workspaces |
| `connections` | `list`, `create`, `refresh`, `new` | Manage connections |
| `tables` | `list` | List tables and columns |
| `datasets` | `list`, `create` | Manage uploaded datasets |
| `query` | | Execute a SQL query |
| `queries` | `list`, `create`, `update`, `run` | Manage saved queries |
| `search` | | Full-text search across a table column |
| `indexes` | `list`, `create` | Manage indexes on a table |
| `results` | `list` | Retrieve stored query results |
| `jobs` | `list` | Manage background jobs |
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
hotdata connections list [-w <id>] [-o table|json|yaml]
hotdata connections <connection_id> [-w <id>] [-o table|json|yaml]
hotdata connections refresh <connection_id> [-w <id>]
hotdata connections new [-w <id>]
```

- `list` returns `id`, `name`, `source_type` for each connection.
- Pass a connection ID to view details (id, name, source type, table counts).
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
hotdata datasets create --url "https://example.com/data.parquet" --label "My Dataset"
```

- Datasets are queryable as `datasets.main.<table_name>`.
- `--file`, `--sql`, `--query-id`, and `--url` are mutually exclusive.
- `--url` imports data directly from a URL (supports csv, json, parquet).
- Format is auto-detected from file extension or content.
- Piped stdin is supported: `cat data.csv | hotdata datasets create --label "My Dataset"`

## Query

```sh
hotdata query "<sql>" [--workspace-id <id>] [--connection <connection_id>] [--format table|json|csv]
```

- Default format is `table`, which prints results with row count and execution time.
- Use `--connection` to scope the query to a specific connection.

## Saved Queries

```sh
hotdata queries list [--limit <n>] [--offset <n>] [--format table|json|yaml]
hotdata queries <query_id> [--format table|json|yaml]
hotdata queries create --name "My Query" --sql "SELECT ..." [--description "..."] [--tags "tag1,tag2"]
hotdata queries update <query_id> [--name "New Name"] [--sql "SELECT ..."] [--description "..."] [--tags "tag1,tag2"]
hotdata queries run <query_id> [--format table|json|csv]
```

- `list` shows saved queries with name, description, tags, and version.
- View a query by ID to see its formatted and syntax-highlighted SQL.
- `create` requires `--name` and `--sql`. Tags are comma-separated.
- `update` accepts any combination of fields to change.
- `run` executes a saved query and displays results like the `query` command.

## Search

```sh
hotdata search "<query>" --table <connection.schema.table> --column <column> [--select <columns>] [--limit <n>] [--format table|json|csv]
```

- Full-text search using BM25 across a table column.
- Requires a BM25 index on the target column (see `indexes create`).
- Results are ordered by relevance score (descending).
- `--select` specifies which columns to return (comma-separated, defaults to all). The `score` column is automatically appended when `--select` is used.

## Indexes

```sh
hotdata indexes list --connection-id <id> --schema <schema> --table <table> [--workspace-id <id>] [--format table|json|yaml]
hotdata indexes create --connection-id <id> --schema <schema> --table <table> --name <name> --columns <cols> [--type sorted|bm25|vector] [--metric l2|cosine|dot] [--async]
```

- `list` shows indexes on a table with name, type, columns, status, and creation date.
- `create` creates an index. Use `--type bm25` for full-text search, `--type vector` for vector search (requires `--metric`).
- `--async` submits index creation as a background job.

## Results

```sh
hotdata results <result_id> [--workspace-id <id>] [--format table|json|csv]
hotdata results list [--workspace-id <id>] [--limit <n>] [--offset <n>] [--format table|json|yaml]
```

- Query results include a `result-id` in the table footer — use it to retrieve past results without re-running queries.

## Jobs

```sh
hotdata jobs list [--workspace-id <id>] [--job-type <type>] [--status <status>] [--all] [--limit <n>] [--offset <n>] [--format table|json|yaml]
hotdata jobs <job_id> [--workspace-id <id>] [--format table|json|yaml]
```

- `list` shows only active jobs (`pending` and `running`) by default. Use `--all` to see all jobs.
- `--job-type` accepts: `data_refresh_table`, `data_refresh_connection`, `create_index`.
- `--status` accepts: `pending`, `running`, `succeeded`, `partially_succeeded`, `failed`.

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
