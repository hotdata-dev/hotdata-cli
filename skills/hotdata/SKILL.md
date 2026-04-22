---
name: hotdata
description: Use this skill when the user wants to run hotdata CLI commands, query the Hotdata API, list workspaces, list connections, create connections, list tables, manage datasets, execute SQL queries, inspect query run history, search tables, manage indexes, manage sandboxes, manage workspace context (data model, glossary, and other named Markdown in the context API), sync context with local files via pull/push, or interact with the hotdata service. Activate when the user says "run hotdata", "query hotdata", "list workspaces", "list connections", "create a connection", "list tables", "list datasets", "create a dataset", "upload a dataset", "execute a query", "search a table", "list indexes", "create an index", "list query runs", "list past queries", "query history", "list sandboxes", "create a sandbox", "run a sandbox", "workspace context", "pull context", "push context", "data model", or asks you to use the hotdata CLI.
version: 0.1.11
---

# Hotdata CLI Skill

Use the `hotdata` CLI to interact with the Hotdata service. In this project, run it as:

```
hotdata <command> [args]
```

Or if installed on PATH: `hotdata <command> [args]`

## Authentication

Run `hotdata auth` to authenticate via browser login. Config is stored in `~/.hotdata/config.yml`.

API key resolution (lowest to highest priority):
1. Config file (saved by `hotdata auth`)
2. `HOTDATA_API_KEY` environment variable (or `.env` file)
3. `--api-key <key>` flag (works on any command)

API URL defaults to `https://api.hotdata.dev/v1` or overridden via `HOTDATA_API_URL`.

## Workspace ID

All commands that accept `--workspace-id` are optional. If omitted, the active workspace is used. Use `hotdata workspaces set` to switch the active workspace interactively, or pass a workspace ID directly: `hotdata workspaces set <workspace_id>`. The active workspace is shown with a `*` marker in `hotdata workspaces list`. **Omit `--workspace-id` unless you need to target a specific workspace.**

## Workspace context (API)

The workspace stores **named Markdown documents** through the Hotdata **context API** (`/v1/context`). The CLI maps each name to a file **`./<NAME>.md`** in the **current working directory** (only for `pull` / `push`; names follow SQL table–identifier rules: ASCII letters, digits, underscore; no dot in the API name; max 128 characters; SQL reserved words are not allowed as names).

**Agents (Claude and similar): use the context API as the canonical store.** Do not assume a data model exists only on disk in the repo.

1. **Before** planning queries, explaining schema, or making modeling decisions, load what the workspace already has: `hotdata context show DATAMODEL` (and `hotdata context list` to discover other stems such as `GLOSSARY`).
2. **After** you materially change the data model (or glossary), persist it for the workspace: write or edit `./DATAMODEL.md` in the project directory where CLI commands run, then `hotdata context push DATAMODEL`. If there is no local file yet, create `./DATAMODEL.md` from the template or from `context show` output, then edit and push.
3. **Optional:** `hotdata context pull DATAMODEL` refreshes `./DATAMODEL.md` from the server; use `--force` if the file already exists.

The standard stem for the workspace semantic model is **`DATAMODEL`** (local sync file **`DATAMODEL.md`**). Use additional stems the same way (for example **`GLOSSARY`** ↔ **`GLOSSARY.md`**) for other long-lived narrative context.

Template and deep modeling procedure still apply to the **content** you store: [references/DATA_MODEL.template.md](references/DATA_MODEL.template.md), [references/MODEL_BUILD.md](references/MODEL_BUILD.md). Keep those documents **out of agent skill install paths**; the authoritative copy for the team should live in **workspace context** (and optionally a synced local file in the repo).

## Multi-step workflows (Model, History, Chain, Indexes)

These are **patterns** built from the commands below—not separate CLI subcommands:

- **Model** — Markdown semantic map of your workspace (entities, keys, joins). **Store and read it via workspace context** (`hotdata context show DATAMODEL`, `context push DATAMODEL`); refresh content using `connections`, `connections refresh`, `tables list`, and `datasets list`. For a **deep** modeling pass (connector enrichment, indexes, per-table detail), see [references/MODEL_BUILD.md](references/MODEL_BUILD.md).
- **History** — Inspect prior activity via `hotdata queries list` (query runs) and `hotdata results list` / `results <id>` (row data).
- **Chain** — Follow-ups via **`datasets create`** then `query` against `datasets.main.<table>`.
- **Indexes** — Review SQL and schema, compare to existing indexes, create **sorted**, **bm25**, or **vector** indexes when it clearly helps; see [references/WORKFLOWS.md](references/WORKFLOWS.md#indexes).

Full step-by-step procedures: [references/WORKFLOWS.md](references/WORKFLOWS.md).

**Legacy / optional local paths:** Older docs may refer to `DATA_MODEL.md` under `docs/` or the project root. **Going forward, prefer workspace context** (`DATAMODEL` + `context show` / `push` / `pull`). If the user keeps a git-tracked file, treat it as a **synced copy**: `context pull` when starting, `context push` after edits.

## Available Commands

### List Workspaces
```
hotdata workspaces list [--format table|json|yaml]
```
Returns workspaces with `public_id`, `name`, `active`, `favorite`, `provision_status`.

### List Connections
```
hotdata connections list [-w <workspace_id>] [-o table|json|yaml]
hotdata connections <connection_id> [-w <workspace_id>] [-o table|json|yaml]
```
- `list` returns `id`, `name`, `source_type` for each connection.
- Pass a connection ID to view details (id, name, source type, table counts).

### Refresh connection schema
```
hotdata connections refresh <connection_id> [-w <workspace_id>]
```
- Refreshes the connection’s catalog so new or changed tables and columns appear in `hotdata tables list` and queries.
- Use after DDL or other changes in the source database when the workspace view is stale.

### Create a Connection

#### Step 1 — Discover available connection types
```
hotdata connections create list [--workspace-id <workspace_id>] [--format table|json|yaml]
```
Returns all available connection types with `name` and `label`.

#### Step 2 — Inspect the schema for a specific type
```
hotdata connections create list <name> [--workspace-id <workspace_id>] [--format json]
```
Returns `config` and `auth` JSON Schema objects describing all required and optional fields for that connection type. Use `--format json` to get the full schema detail.

- `config` — connection configuration fields (host, port, database, etc.). May be `null` for services that need no configuration.
- `auth` — authentication fields (password, token, credentials, etc.). May be `null` for services that need no authentication. May be a `oneOf` with multiple authentication method options.

#### Step 3 — Create the connection
```
hotdata connections create \
  --name "my-connection" \
  --type <source_type> \
  --config '<json object>' \
  [--workspace-id <workspace_id>]
```

The `--config` JSON object must contain all **required** fields from `config` plus the **auth fields** merged in at the top level. Auth fields are not nested — they sit alongside config fields in the same object.

Example for PostgreSQL (required: `host`, `port`, `user`, `database` + auth field `password`):
```
hotdata connections create \
  --name "my-postgres" \
  --type postgres \
  --config '{"host":"db.example.com","port":5432,"user":"myuser","database":"mydb","password":"..."}'
```

**Security: never expose credentials in plain text.** Passwords, tokens, API keys, and any field with `"format": "password"` in the schema must never be hardcoded as literal strings in CLI commands. Always use one of these safe approaches:

- Read from an environment variable:
  ```
  --config "{\"host\":\"db.example.com\",\"port\":5432,\"user\":\"myuser\",\"database\":\"mydb\",\"password\":\"$DB_PASSWORD\"}"
  ```
- Read a credential from a file and inject it:
  ```
  --config "{\"token\":\"$(cat ~/.secrets/my-token)\"}"
  ```

**Field-building rules from the schema:**

- Include all fields listed in `config.required` — these are mandatory.
- Include optional config fields only if the user provides values for them.
- For `auth` with a single method (no `oneOf`): include all `auth.required` fields in the config object.
- For `auth` with `oneOf`: pick one authentication method and include only its required fields.
- Fields with `"format": "password"` are credentials — apply the security rules above.
- Fields with `"type": "integer"` must be JSON numbers, not strings (e.g. `"port": 5432` not `"port": "5432"`).
- Fields with `"type": "boolean"` must be JSON booleans (e.g. `"use_tls": true`).
- Fields with `"type": "array"` must be JSON arrays (e.g. `"spreadsheet_ids": ["abc", "def"]`).
- Nested `oneOf` fields must be a JSON object including a `"type"` discriminator field matching the chosen variant's `const` value.

### List Tables and Columns
```
hotdata tables list [--workspace-id <workspace_id>] [--connection-id <connection_id>] [--schema <pattern>] [--table <pattern>] [--limit <int>] [--cursor <cursor>] [--format table|json|yaml]
```
- Default format is `table`.
- **Always use this command to inspect available tables and columns.** Do NOT use the `query` command to query `information_schema` for this purpose.
- Without `--connection-id`: lists all tables with `table`, `synced`, `last_sync`. The `table` column is formatted as `<connection>.<schema>.<table>`.
- With `--connection-id`: includes column definitions. Lists each column as its own row with `table`, `column`, `data_type`, `nullable`. Use this to inspect the schema before writing queries.
- **Always use the full `<connection>.<schema>.<table>` name when referencing tables in SQL queries.**
- `--schema` and `--table` support SQL `%` wildcard patterns (e.g. `--table order%` matches `orders`, `order_items`, etc.).
- Results are paginated (default 100 per page). If more results are available, a `--cursor` token is printed — pass it to fetch the next page.

### Datasets

Datasets are managed files uploaded to Hotdata and queryable as tables.

#### List datasets
```
hotdata datasets list [--workspace-id <workspace_id>] [--limit <int>] [--offset <int>] [--format table|json|yaml]
```
- Default format is `table`.
- Returns `id`, `label`, `table_name`, `created_at`.
- Results are paginated (default 100). Use `--offset` to fetch further pages.

#### Get dataset details
```
hotdata datasets <dataset_id> [--workspace-id <workspace_id>] [--format table|json|yaml]
```
- Shows dataset metadata and a full column listing with `name`, `data_type`, `nullable`.
- Use this to inspect schema before querying.

#### Create a dataset
```
hotdata datasets create --label "My Dataset" --file data.csv [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --sql "SELECT * FROM ..." [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --query-id <saved_query_id> [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --url "https://example.com/data.parquet" [--table-name my_dataset] [--workspace-id <workspace_id>]
```
- `--file` uploads a local file. Omit to pipe data via stdin: `cat data.csv | hotdata datasets create --label "My Dataset"`
- `--sql` creates a dataset from a SQL query result.
- `--query-id` creates a dataset from a previously saved query.
- `--url` imports data directly from a URL (supports csv, json, parquet).
- `--file`, `--sql`, `--query-id`, and `--url` are mutually exclusive.
- Format is auto-detected from file extension (`.csv`, `.json`, `.parquet`) or file content.
- `--label` is optional when `--file` is provided — defaults to the filename without extension. Required for `--sql` and `--query-id`.
- `--table-name` is optional — derived from the label if omitted.

#### Querying datasets

Datasets are queryable using the catalog `datasets` and schema `main`. Always reference dataset tables as:
```
datasets.main.<table_name>
```
For example:
```
hotdata query "SELECT * FROM datasets.main.my_dataset LIMIT 10"
```
Use `hotdata datasets <dataset_id>` to look up the `table_name` before writing queries.

### Workspace context (named Markdown)

Syncs workspace **context API** documents with **`./<NAME>.md`** in the current directory. See [Workspace context (API)](#workspace-context-api) for agent rules.

```
hotdata context list [-w <workspace_id>] [--prefix <stem>] [-o table|json|yaml]
hotdata context show <name> [-w <workspace_id>]
hotdata context pull <name> [-w <workspace_id>] [--force] [--dry-run]
hotdata context push <name> [-w <workspace_id>] [--dry-run]
```

- `list` — names, `updated_at`, and character counts for each stored context. Use `--prefix` to narrow names (case-sensitive).
- `show` — print the Markdown body to **stdout** (use this when there is **no** local `./<NAME>.md`; ideal for agents).
- `pull` — download context `name` to `./<NAME>.md`. Refuses to overwrite an existing file unless `--force`. `--dry-run` prints target path and size only.
- `push` — upload `./<NAME>.md` to upsert context `name` on the server. `--dry-run` prints size only. Body size must stay within the API limit (order of 512k characters).

**Convention:** `DATAMODEL` is the primary workspace data model; `GLOSSARY` (or other stems) for additional narrative context. Same identifier rules as SQL table names.

### Execute SQL Query
```
hotdata query "<sql>" [-w <workspace_id>] [--connection <connection_id>] [-o table|json|csv]
hotdata query status <query_run_id> [-o table|json|csv]
```
- Default output is `table`, which prints results with row count and execution time.
- Use `--connection` to scope the query to a specific connection.
- Use `hotdata tables list` to discover tables and columns — do not query `information_schema` directly.
- **Always use PostgreSQL dialect SQL.**
- Long-running queries automatically fall back to async execution and return a `query_run_id`.
- Use `hotdata query status <query_run_id>` to poll for results.
- Exit codes for `query status`: `0` = succeeded, `1` = failed, `2` = still running (poll again).
- **When a query returns a `query_run_id`, use `query status` to poll rather than re-running the query.**

### Query results
#### List stored results
```
hotdata results list [-w <workspace_id>] [--limit <int>] [--offset <int>] [-o table|json|yaml]
```
- Lists recent stored query results with `id`, `status`, and `created_at`.
- Results are paginated; when more are available, the CLI prints a hint with the next `--offset`.
- Use a row’s `id` with `hotdata results <result_id>` below.

#### Get result by ID
```
hotdata results <result_id> [-w <workspace_id>] [-o table|json|csv]
```
- Retrieves a previously executed query result by its result ID.
- Query output also includes a `result-id` in the footer (e.g. `[result-id: rslt...]`).
- **Always use `results list` / `results <id>` to retrieve past query results rather than re-running the same query.** Re-running queries wastes resources and may return different results.

### Query Run History
```
hotdata queries list [--limit <int>] [--cursor <token>] [--status <csv>] [-o table|json|yaml]
hotdata queries <query_run_id> [-o table|json|yaml]
```
- `list` shows query runs with status, creation time, duration, row count, and a truncated SQL preview (default limit 20).
- `--status` filters by run status (comma-separated, e.g. `--status running,failed`).
- View a run by ID to see full metadata (timings, `result_id`, snapshot, hashes) and the formatted, syntax-highlighted SQL.
- If a run has a `result_id`, fetch its rows with `hotdata results <result_id>`.

### Search
```
# BM25 full-text search
hotdata search "query text" --table <connection.schema.table> --column <column> [--select <columns>] [--limit <n>] [-o table|json|csv]

# Vector search with --model (calls OpenAI to embed the query)
hotdata search "query text" --table <table> --column <vector_column> --model text-embedding-3-small [--limit <n>]

# Vector search with piped embedding
echo '[0.1, -0.2, ...]' | hotdata search --table <table> --column <vector_column> [--limit <n>]
```
- Without `--model` and with query text: BM25 full-text search. Requires a BM25 index on the target column.
- With `--model`: generates an embedding via OpenAI and performs vector search using `l2_distance`. Requires `OPENAI_API_KEY` env var. Supported models: `text-embedding-3-small`, `text-embedding-3-large`.
- Without query text and with piped stdin: reads a vector (raw JSON array or OpenAI embedding response) and performs vector search.
- BM25 results are ordered by relevance score (descending). Vector results are ordered by distance (ascending).
- `--select` specifies which columns to return (comma-separated, defaults to all).
- Default limit is 10.
- **For BM25 search, create a BM25 index on the target column first. For vector search, create a vector index.**

### Indexes
```
hotdata indexes list -c <connection_id> --schema <schema> --table <table> [-w <workspace_id>] [-o table|json|yaml]
hotdata indexes create -c <connection_id> --schema <schema> --table <table> --name <name> --columns <cols> [--type sorted|bm25|vector] [--metric l2|cosine|dot] [--async]
```
- `list` shows indexes on a table with name, type, columns, status, and creation date.
- `create` creates an index. Use `--type bm25` for full-text search, `--type vector` for vector search (requires `--metric`).
- `--async` submits index creation as a background job. Use `hotdata jobs <job_id>` to check status.

### Jobs
```
hotdata jobs list [--workspace-id <workspace_id>] [--job-type <type>] [--status <status>] [--all] [--format table|json|yaml]
hotdata jobs <job_id> [--workspace-id <workspace_id>] [--format table|json|yaml]
```
- `list` shows only active jobs (`pending`, `running`) by default. Use `--all` to see all jobs.
- `--job-type`: `data_refresh_table`, `data_refresh_connection`, `create_index`.
- `--status`: `pending`, `running`, `succeeded`, `partially_succeeded`, `failed`.
- Use `hotdata jobs <job_id>` to inspect a specific job's status, error, and result.

### Auth
```
hotdata auth                # Browser-based login
hotdata auth status         # Check current auth status
hotdata auth logout         # Remove saved auth for the default profile
```

### Sandboxes

Sandboxes are for **ad-hoc, exploratory work** that does not need to be long-lived. They group related CLI activity (queries, dataset operations, etc.) under a single context so it can be tracked and cleaned up together. **Datasets created inside a sandbox are tied to that sandbox and will be removed when the sandbox ends.** If you need data to persist beyond the sandbox, create datasets outside of a sandbox context.

> **IMPORTANT: If `HOTDATA_SANDBOX` is set in the environment, you are inside an active sandbox. NEVER attempt to unset, override, or work around this variable. Do not clear it, do not start a new sandbox, do not run `sandbox run` or `sandbox new` or `sandbox set`. All your work should be attributed to the current sandbox. Attempting to nest or escape a sandbox will fail with an error.**

```
hotdata sandbox list [-w <workspace_id>] [-o table|json|yaml]
hotdata sandbox <sandbox_id> [-w <workspace_id>] [-o table|json|yaml]
hotdata sandbox new [--name "Sandbox Name"] [-o table|json|yaml]
hotdata sandbox set [<sandbox_id>]
hotdata sandbox read
hotdata sandbox update [<sandbox_id>] [--name "New Name"] [--markdown "..."] [-o table|json|yaml]
hotdata sandbox run <cmd> [args...]
hotdata sandbox <sandbox_id> run <cmd> [args...]
```

- `list` shows all sandboxes with a `*` marker on the active one.
- `new` creates a sandbox and sets it as active. Blocked inside an existing sandbox.
- `set` switches the active sandbox. Omit the ID to clear. Blocked inside an existing sandbox.
- `read` prints the markdown content of the current sandbox. Use this to retrieve sandbox state at the start of work or between steps.
- `update` modifies a sandbox's name or markdown. Defaults to the active sandbox if no ID is given. The `--markdown` field is for writing details about the work being done in the sandbox — observations, intermediate findings, next steps, etc. This state persists for the life of the sandbox and is the primary way to record context that should survive across commands or agent invocations within the sandbox.
- `run` launches a command with `HOTDATA_SANDBOX` and `HOTDATA_WORKSPACE` set in the child process environment. Creates a new sandbox unless a sandbox ID is provided before `run`. Blocked inside an existing sandbox.
- When inside a sandbox (HOTDATA_SANDBOX is set), all API requests automatically include the sandbox ID — no extra flags needed.

#### Example: Building a data model in a sandbox

Use a sandbox to explore tables and iteratively build a model description in the sandbox markdown.

1. Start a sandbox:
   ```
   hotdata sandbox new --name "Model: sales pipeline"
   ```
2. Inspect tables and columns:
   ```
   hotdata tables list --connection-id <connection_id>
   ```
3. Run exploratory queries to understand relationships, cardinality, and key columns:
   ```
   hotdata query "SELECT DISTINCT status FROM sales.public.deals LIMIT 20"
   hotdata query "SELECT count(*), count(DISTINCT account_id) FROM sales.public.deals"
   ```
4. Write findings into the sandbox markdown as you go:
   ```
   hotdata sandbox update --markdown "## sales pipeline model
   
   ### deals (sales.public.deals)
   - PK: id
   - FK: account_id -> accounts.id
   - status: open | won | lost
   - ~50k rows, one row per deal
   
   ### accounts (sales.public.accounts)
   - PK: id
   - name, industry, created_at
   - ~12k rows, one row per company
   
   ### TODO
   - check how line_items joins to deals
   - confirm revenue column semantics"
   ```
5. Continue exploring and update the markdown as the model takes shape. The sandbox markdown is the living artifact for **that sandbox**.
6. When the model should **outlive the sandbox** or be shared with the whole workspace, promote it to workspace context: save the consolidated Markdown as `./DATAMODEL.md` in the project directory and run `hotdata context push DATAMODEL` (or merge with `hotdata context show DATAMODEL` first, then push).

Other commands (not covered in detail above): `hotdata connections new` (interactive connection wizard), `hotdata skills install|status`, `hotdata completions <bash|zsh|fish>`.

## Workflow: Running a Query

0. (Recommended for agents) Load the workspace data model when available: run `hotdata context show DATAMODEL`. If the command errors because no context exists yet, proceed without a stored model.
1. List connections:
   ```
   hotdata connections list
   ```
2. Inspect available tables:
   ```
   hotdata tables list
   ```
3. Inspect columns for a specific connection:
   ```
   hotdata tables list --connection-id <connection_id>
   ```
4. Run SQL:
   ```
   hotdata query "SELECT 1"
   ```

## Workflow: Creating a Connection

1. List available connection types:
   ```
   hotdata connections create list
   ```
2. Inspect the schema for the desired type:
   ```
   hotdata connections create list <type_name> --format json
   ```
3. Collect required config and auth field values from the user or environment. **Never hardcode credentials — use env vars or files.**
4. Create the connection:
   ```
   hotdata connections create --name "my-connection" --type <type_name> --config '<json>'
   ```
