<p align="center">
  <img src="https://avatars.githubusercontent.com/u/226170140" alt="Hotdata" width="120">
  <br>
  <strong>Hotdata CLI</strong>
  <br>
  Command line interface for <a href="https://www.hotdata.dev">Hotdata</a>.
  <br><br>
  <img src="https://img.shields.io/badge/version-0.4.2-blue" alt="version">
  <a href="https://github.com/hotdata-dev/hotdata-cli/actions/workflows/ci.yml"><img src="https://github.com/hotdata-dev/hotdata-cli/actions/workflows/ci.yml/badge.svg" alt="build"></a>
  <a href="https://codecov.io/gh/hotdata-dev/hotdata-cli"><img src="https://codecov.io/gh/hotdata-dev/hotdata-cli/branch/main/graph/badge.svg" alt="coverage"></a>
</p>

---

## Install

**Homebrew**

```sh
brew install hotdata-dev/tap/cli
```

**Binary (macOS, Linux)**

Download a binary from [Releases](https://github.com/hotdata-dev/hotdata-cli/releases).

**Build from source** (requires Rust)

```sh
cargo build --release
cp target/release/hotdata /usr/local/bin/hotdata
```

## Connect

Run either of the following (they are equivalent):

```sh
hotdata auth login
# or
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
| `auth` | `login`, `status`, `logout` | `login` or bare `auth` opens browser login; `status` / `logout` manage the saved profile |
| `workspaces` | `list`, `set` | Manage workspaces |
| `connections` | `list`, `create`, `refresh`, `new` | Manage connections |
| `databases` | `list`, `create`, `delete`, `tables` | Managed databases (create and load tables via parquet) |
| `tables` | `list` | List tables and columns |
| `datasets` | `list`, `create`, `update` | Manage uploaded datasets |
| `context` | `list`, `show`, `pull`, `push` | Workspace Markdown context (e.g. data model `DATAMODEL`) via the context API |
| `query` | | Execute a SQL query |
| `queries` | `list` | Inspect query run history |
| `search` | | Full-text search across a table column |
| `indexes` | `list`, `create`, `delete` | Manage indexes on a table or dataset |
| `embedding-providers` | `list`, `get`, `create`, `update`, `delete` | Manage embedding providers used by vector indexes |
| `results` | `list` | Retrieve stored query results |
| `jobs` | `list` | Manage background jobs |
| `skills` | `install`, `status` | Manage the hotdata agent skill |

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
hotdata connections refresh <connection_id> [-w <id>] [--data] [--schema <name> --table <name>] [--async] [--include-uncached]
hotdata connections new [-w <id>]
```

- `list` returns `id`, `name`, `source_type` for each connection.
- Pass a connection ID to view details (id, name, source type, table counts).
- `refresh` triggers a schema refresh by default. Pass `--data` to refresh cached row data instead.
- `--schema` and `--table` narrow a data refresh to a single table (must be supplied together).
- `--async` submits a data refresh as a background job and returns a job ID; poll with `hotdata jobs <job_id>`. Only valid with `--data` — schema refresh is always synchronous.
- `--include-uncached` includes tables that haven't been cached yet in a connection-wide data refresh. Only valid with `--data` and no `--table`.
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

## Databases

Managed databases are Hotdata-owned catalogs you create and populate yourself (no remote source to sync). Query them with SQL as `<catalog>.schema.table`.

```sh
hotdata databases list [-w <id>] [-o table|json|yaml]
hotdata databases create [--name <display_name>] [--catalog <alias>] [--table <table> ...] [--schema public] [--expires-at <duration|timestamp>] [-o table|json|yaml]
hotdata databases set <id>
hotdata databases unset
hotdata databases <name_or_id> [-o table|json|yaml]
hotdata databases delete <name_or_id>
hotdata databases run [--database <id>] [--name <label>] [--schema public] [--table <table> ...] [--expires-at <duration|timestamp>] <cmd> [args...]
hotdata databases <id> run <cmd> [args...]

# Preferred: load by catalog alias (auto-declares table if needed)
hotdata databases load --catalog <alias> --table <table> [--schema public] (--file <path> | --url <url> | --upload-id <id>)

# Also available: explicit database flag
hotdata databases tables list [--database <id_or_name>] [--schema <name>] [-o table|json|yaml]
hotdata databases tables load <table> [--database <id_or_name>] [--schema public] (--file <path> | --url <url> | --upload-id <id>)
hotdata databases tables delete <table> [--database <id_or_name>] [--schema public]
```

- `create` registers a managed connection with no external credentials. `--name` is a human-readable display name; `--catalog` sets the SQL alias used in queries (`SELECT … FROM <catalog>.schema.table`) and must be `[a-z_][a-z0-9_]*`.
- `set` / `unset` — save or clear the active database. All `databases tables` and `context` commands default to it. The active database is marked with `*` in `databases list`.
- `load` (top-level shorthand) — loads a parquet file into `--catalog.--schema.--table`. If the table was not declared at create time, the CLI automatically deletes and recreates the database with the table declared, then retries the load.
- `tables load` uploads a **parquet** file (or uses a staged `upload_id` from `POST /v1/files`) and publishes it as the table generation (`replace` mode).
- `run` mints a database-scoped JWT and execs `<cmd>` with `HOTDATA_DATABASE_TOKEN`, `HOTDATA_DATABASE_REFRESH_TOKEN`, `HOTDATA_DATABASE`, `HOTDATA_WORKSPACE`, and `HOTDATA_API_URL` injected into its environment.
- For CSV/JSON uploads without a managed database, use `hotdata datasets create` instead (`datasets.main.*`).

Example:

```sh
hotdata databases create --catalog airbnb
hotdata databases load --catalog airbnb --table listings --url https://example.com/listings.parquet
hotdata query "SELECT count(*) FROM airbnb.public.listings"
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
hotdata datasets update <dataset_id> [--label "New Label"] [--table-name new_table]
hotdata datasets refresh <dataset_id> [--workspace-id <id>] [--async]
```

- Datasets are queryable as `datasets.main.<table_name>`.
- `--file`, `--sql`, `--query-id`, and `--url` are mutually exclusive.
- `--url` imports data directly from a URL (supports csv, json, parquet).
- Format is auto-detected from file extension or content.
- Piped stdin is supported: `cat data.csv | hotdata datasets create --label "My Dataset"`
- `refresh` re-runs the dataset's source (URL fetch or saved query) and creates a new version. Not supported for upload-source datasets.
- `--async` submits the refresh as a background job and returns a job ID; poll with `hotdata jobs <job_id>`.

## Workspace context

Named Markdown documents for a workspace (data model, glossary, etc.) are stored in the **context API**. The CLI treats the server as the **source of truth**; local files are only used where the tool requires a path on disk.

```sh
hotdata context list [-w <id>] [--prefix <stem>] [-o table|json|yaml]
hotdata context show <name> [-w <id>]
hotdata context pull <name> [-w <id>] [--force] [--dry-run]
hotdata context push <name> [-w <id>] [--dry-run]
```

- **`show`** prints Markdown to stdout (no local file needed). Use this to read the workspace data model in scripts or agents.
- **`pull`** writes `./<name>.md` in the **current directory** from the API. Refuses to overwrite an existing file unless `--force`.
- **`push`** reads `./<name>.md` and upserts that name in the workspace. Use after editing the file in your project directory.
- Names follow SQL identifier rules (ASCII letters, digits, underscore; max 128 characters; SQL reserved words are not allowed). The usual stem for the semantic data model is **`DATAMODEL`** (file **`DATAMODEL.md`** for push/pull only).

## Query

```sh
hotdata query "<sql>" [-w <id>] [-d <database>] [-o table|json|csv]
hotdata query status <query_run_id> [-o table|json|csv]
```

- Default output is `table`, which prints results with row count and execution time.
- Use `-d`/`--database` to run the query against a specific managed database.
- Long-running queries automatically fall back to async execution and return a `query_run_id`.
- Use `hotdata query status <query_run_id>` to poll for results.
- Exit codes for `query status`: `0` = succeeded, `1` = failed, `2` = still running (poll again).

## Query Run History

```sh
hotdata queries list [--limit <n>] [--cursor <token>] [--status <csv>] [-o table|json|yaml]
hotdata queries <query_run_id> [-o table|json|yaml]
```

- `list` shows past query executions with status, creation time, duration, row count, and a truncated SQL preview (default limit 20).
- `--status` filters by run status (comma-separated, e.g. `--status running,failed`).
- View a run by ID to see full metadata (timings, `result_id`, snapshot, hashes) and the formatted, syntax-highlighted SQL.
- If a run has a `result_id`, fetch its rows with `hotdata results <result_id>`.

## Search

Both run entirely server-side. `--type` and `--column` are **optional** when the table has exactly one search index — they are inferred automatically. Pass them explicitly when multiple indexes exist.

```sh
# BM25 full-text search (requires a BM25 index on the column)
hotdata search "<query>" --table <connection.schema.table> [--type bm25] [--column <column>] [--select <columns>] [--limit <n>] [-o table|json|csv]

# Vector search (requires a vector index with auto-embedding on the column)
hotdata search "<query>" --table <table> [--type vector] [--column <source_text_column>] [--limit <n>]
```

- **`--type vector`** — pass your query as **plain text**, name the **source text column** (e.g. `title`). The server embeds the query at the same time, using the same provider that auto-embedded the column when the index was built — so distance metric, model, and dimensions all match automatically. No `OPENAI_API_KEY`, no client-side embedding, no need to know about the auto-generated `_embedding` column. Generated SQL: `vector_distance(col, 'query')` server-side.
- **`--type bm25`** runs `bm25_search(table, col, 'query')` — requires a BM25 index on the column.
- **No vector index, or want to use a different model than the index?** Skip `hotdata search` and use raw SQL via `hotdata query` (e.g. `SELECT *, cosine_distance(col, [<your_vec>]) FROM ...`). The SQL reference covers the available distance functions and table UDFs.
- BM25 results sort by score (descending). Vector results sort by distance (ascending).
- `--select` specifies which columns to return (comma-separated, defaults to all).
- The previous `--model` flag and stdin-piped-vector path are **removed** — both hardcoded `l2_distance` regardless of the index's actual metric, which silently produced wrong rankings on cosine indexes. For client-side embedding or precomputed-vector workflows, use raw SQL via `hotdata query` (e.g. `SELECT *, cosine_distance(col, [<vec>]) ...`).

## Indexes

Indexes attach to either a connection-table (`--connection-id` + `--schema` + `--table`) or a dataset (`--dataset-id`). The two scopes are mutually exclusive.

```sh
# Managed database scope (catalog alias resolves via active database)
hotdata indexes create --catalog <alias> --schema <schema> --table <table> \
  --column <cols> --type bm25|vector|sorted \
  [--name <name>] [--metric l2|cosine|dot] [--async] \
  [--embedding-provider-id <id>] [--dimensions <n>] [--output-column <name>] [--description <text>]

# Connection-table scope (for non-managed connections)
hotdata indexes list   --connection-id <id> --schema <schema> --table <table> [-o table|json|yaml]
hotdata indexes create --connection-id <id> --schema <schema> --table <table> \
  --column <cols> --type sorted|bm25|vector [--name <name>] ...
hotdata indexes delete --connection-id <id> --schema <schema> --table <table> --name <name>

# Dataset scope
hotdata indexes list   --dataset-id <id> [-o table|json|yaml]
hotdata indexes create --dataset-id <id> --column <cols> --type sorted|bm25|vector [--name <name>] ...
hotdata indexes delete --dataset-id <id> --name <name>
```

- `--type` is **required** — choose `sorted` (B-tree-like), `bm25` (full-text), or `vector` (similarity).
- `--type vector` requires exactly one column.
- `--async` submits index creation as a background job and returns a job ID; poll with `hotdata jobs <job_id>`.
- **Auto-embedding (text → vector):** when `--type vector` is used on a text column, embeddings are generated automatically. The embedding provider can be specified with `--embedding-provider-id`; if omitted, the first system provider is used. The generated column defaults to `{column}_embedding` and can be overridden with `--output-column`.

## Embedding providers

```sh
hotdata embedding-providers list [-o table|json|yaml]
hotdata embedding-providers get <id> [-o table|json|yaml]
hotdata embedding-providers create --name <name> --provider-type service|local \
  [--config '<json>'] [--provider-api-key <key> | --secret-name <name>]
hotdata embedding-providers update <id> [--name <name>] [--config '<json>'] \
  [--provider-api-key <key> | --secret-name <name>]
hotdata embedding-providers delete <id>
```

- `list`/`get` show registered providers (system providers like `sys_emb_openai` come pre-configured).
- `--provider-api-key` auto-creates a managed secret for the provider; `--secret-name` references an existing secret. They are mutually exclusive.
- `--provider-api-key` pairs with `--provider-type` and avoids colliding with the global `--api-key` (Hotdata auth).

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
- `--job-type` accepts: `data_refresh_table`, `data_refresh_connection`, `dataset_refresh`, `create_index`, `create_dataset_index`.
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
