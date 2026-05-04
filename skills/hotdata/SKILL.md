---
name: hotdata
description: Use this skill when the user wants to run hotdata CLI commands, query the Hotdata API, list workspaces, list connections, create connections, list tables, manage datasets, execute SQL queries, inspect query run history, search tables, manage indexes, manage sandboxes, manage workspace context and stored docs such as context:DATAMODEL via the context API (`hotdata context`), install or update the bundled agent skills (`hotdata skills`), generate shell completions (`hotdata completions`), or interact with the hotdata service. Activate when the user says "run hotdata", "query hotdata", "list workspaces", "list connections", "create a connection", "list tables", "list datasets", "create a dataset", "upload a dataset", "execute a query", "search a table", "list indexes", "create an index", "list query runs", "list past queries", "query history", "list sandboxes", "create a sandbox", "run a sandbox", "workspace context", "pull context", "push context", "data model", "context:DATAMODEL", or asks you to use the hotdata CLI.
version: 0.2.2
---

# Hotdata CLI Skill

Use the `hotdata` CLI to interact with the Hotdata service. In this project, run it as:

```
hotdata <command> [args]
```

Or if installed on PATH: `hotdata <command> [args]`

## Authentication

Run **`hotdata auth login`** (or **`hotdata auth`** with no subcommand—same behavior) to authenticate via browser login. Config is stored in `~/.hotdata/config.yml`.

API key resolution (lowest to highest priority):
1. Config file (saved by `hotdata auth login` / `hotdata auth`)
2. `HOTDATA_API_KEY` environment variable (or `.env` file)
3. `--api-key <key>` flag (works on any command)

API URL defaults to `https://api.hotdata.dev/v1` or overridden via `HOTDATA_API_URL`.

Optional: pass **`--debug`** on any command to print verbose HTTP request/response details.

## Workspace ID

Commands that accept `--workspace-id` default to the active workspace from config when omitted. Use `hotdata workspaces set` to switch interactively, or `hotdata workspaces set <workspace_id>` for a direct choice. In `hotdata workspaces list`, the `*` marker labels the **default** workspace the CLI resolves to.

**`hotdata queries` does not accept `--workspace-id`:** query run history always uses the active workspace—set it with `workspaces set` first if needed.

If **`HOTDATA_WORKSPACE`** is set in the environment, the workspace is **locked** to that value: passing a different `--workspace-id` is an error, and **`hotdata workspaces set` fails** (“workspace is locked”). **`workspaces set` is also blocked** while the current process was started under **`hotdata sandbox run`** (nested workspace changes are not allowed in that tree).

**Omit `--workspace-id` unless you need to target a specific workspace** (and it is not locked by env or session).

## Workspace context (API)

**Notation `context:<STEM>`:** In this skill, **`context:DATAMODEL`**, **`context:GLOSSARY`**, and **`context:<NAME>`** mean the **authoritative Markdown document** stored on the server under that **stem** via the Hotdata **context API** (`/v1/context`, `hotdata context …`). That is **not** the same as generic English (“a data model”, “a glossary”), and **not** the same as local `./DATAMODEL.md` except as **pull/push transport**. **CLI commands use the bare stem** (no `context:` prefix): e.g. `hotdata context show DATAMODEL`, `hotdata context push GLOSSARY`.

The workspace stores those documents only through the **context API**. The **authoritative** copy always lives on the server under the stem; common stems are **`context:DATAMODEL`** (semantic map) and **`context:GLOSSARY`** (glossary / runbooks).

The CLI command **`hotdata context push`** reads **`./<NAME>.md`** and **`pull`** writes that file in the **current working directory**—those files exist only as a **transport surface** for the API, not as a second source of truth. **`hotdata context show <name>`** prints Markdown to stdout so agents can read **`context:<NAME>`** without any local file. Stems follow SQL table–identifier rules (ASCII letters, digits, underscore; no dot in the API name; max 128 characters; SQL reserved words are not allowed). For **`show`**, **`pull`**, and **`push`**, the CLI accepts a trailing **`.md`** on the argument (e.g. **`USER.md`**) and treats it as stem **`USER`**—the workspace still stores **`USER`**, not `USER.md`.

> **Agents: do not blindly run `hotdata context show DATAMODEL` on session start.** Run **`hotdata context list`** first (optional `--prefix DATAMODEL`). Call **`hotdata context show DATAMODEL` only if** the list includes the `DATAMODEL` stem. If **`show` exits 1** with *no context named …*, that is **normal** when nothing has been pushed yet—**not a hard failure**; do not retry in a loop, and **avoid speculative `show` in parallel** with other shell tools where one failure cancels sibling calls. Proceed without **context:DATAMODEL** until the user asks to create or load one.

**Agents (Claude and similar):** use workspace context as the only durable store for **context:DATAMODEL**, **context:GLOSSARY**, and any other **`context:<STEM>`** documents you introduce. Keep transient analysis notes in **sandbox markdown** or the conversation until you **promote** them into **context:DATAMODEL** when they should guide the whole workspace ([details below](#analysis-modeling-vs-contextdatamodel)).

1. **Before** planning non-trivial queries, explaining schema to others, or editing **context:DATAMODEL**, **discover** stored names with `hotdata context list` (and other stems such as **context:GLOSSARY** as needed). **Only if** `DATAMODEL` appears in the list, load it: `hotdata context show DATAMODEL`. If it is **absent**, skip `show` and treat **context:DATAMODEL** as unset—use [references/DATA_MODEL.template.md](references/DATA_MODEL.template.md) when the user wants to bootstrap, then `push` when ready.
2. **After** you change **context:DATAMODEL**, persist with **`hotdata context push DATAMODEL`**. The CLI requires a local `./DATAMODEL.md` for that step: write the body there (from `context show`, the template, or your edits), then run `push` from the project directory.
3. Use **`hotdata context pull DATAMODEL`** when you intentionally want a local `./DATAMODEL.md` copy (for example a human editor); it still reflects API state for **context:DATAMODEL**, not a parallel document.

The standard stem for the workspace semantic map is **`DATAMODEL`** (skill notation **context:DATAMODEL**). Add other stems the same way (e.g. **`GLOSSARY`** → **context:GLOSSARY**) for glossary or runbooks.

### Analysis modeling vs context:DATAMODEL

Keep two layers separate:

- **Analysis modeling (day to day)** — Understanding data *for the current task*: exploratory SQL, join checks, column semantics for one report, hypotheses, scratch notes. Often conversational or short-lived. **Sandbox markdown** (`sandbox update --markdown`) is the right home while you explore; it dies with the sandbox unless you copy it elsewhere.

- **context:DATAMODEL (Hotdata workspace data model)** — A **durable, workspace-wide** map stored only via the **context API**: entities and tables across connections, PK/FK relationships, how datasets tie back to sources, naming and query conventions the **whole team** should rely on. This is **higher-level shared structure**, not a transcript of one investigation.

**Promotion:** When analysis findings should **outlive** the sandbox or session and **guide everyone**, merge them into **context:DATAMODEL** (`hotdata context list` → if `DATAMODEL` is listed, `hotdata context show DATAMODEL` → reconcile → `hotdata context push DATAMODEL`). You do **not** need to update **context:DATAMODEL** after every ad-hoc query—only when the workspace story or join graph meaningfully changes.

Use [references/DATA_MODEL.template.md](references/DATA_MODEL.template.md) and [references/MODEL_BUILD.md](references/MODEL_BUILD.md) for **what to write inside** the Markdown you store under **context:** stems. Never put workspace-specific model text inside agent skill install paths—only in **workspace context** (and transient `./<NAME>.md` for push/pull when needed).

## Multi-step workflows (Model, History, Chain, Indexes)

These are **patterns** built from the commands below—not separate CLI subcommands:

- **Model (`context:DATAMODEL`)** — The **shared** Markdown semantic map of the workspace (entities, keys, joins across connections). **Store and read it only via workspace context** (`hotdata context list`, then `hotdata context show DATAMODEL` **only when listed**, `context push DATAMODEL`); refresh using `connections`, `connections refresh`, `tables list`, and `datasets list`. For a **deep** pass (connector enrichment, indexes, per-table detail), see [references/MODEL_BUILD.md](references/MODEL_BUILD.md). Contrast **analysis modeling** in sandboxes or chat (see [Analysis modeling vs context:DATAMODEL](#analysis-modeling-vs-contextdatamodel)).
- **History** — Inspect prior activity via `hotdata queries list` (query runs) and `hotdata results list` / `results <id>` (row data).
- **Chain** — Follow-ups via **`datasets create`** then `query` against `datasets.<schema>.<table>`.
- **Indexes** — Review SQL and schema, compare to existing indexes, create **sorted**, **bm25**, or **vector** indexes when it clearly helps; see [references/WORKFLOWS.md](references/WORKFLOWS.md#indexes).

Full step-by-step procedures: [references/WORKFLOWS.md](references/WORKFLOWS.md).

## Available Commands

Top-level subcommands (each detailed below): **`auth`**, **`datasets`**, **`query`**, **`workspaces`**, **`connections`**, **`tables`**, **`skills`**, **`results`**, **`jobs`**, **`indexes`**, **`embedding-providers`**, **`search`**, **`queries`**, **`sandbox`**, **`context`**, **`completions`**.

Global CLI options: **`--api-key`**, **`-v` / `--version`**, **`-h` / `--help`**. Hidden developer flag: **`--debug`** (verbose HTTP logs).

### List Workspaces
```
hotdata workspaces list [--output table|json|yaml]
```
Returns workspaces with `public_id`, `name`, `active`, `favorite`, `provision_status`. Table output marks the default workspace with `*`.

### List Connections
```
hotdata connections list [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata connections <connection_id> [--workspace-id <workspace_id>] [--output table|json|yaml]
```
- `list` returns `id`, `name`, `source_type` for each connection.
- Pass a connection ID to view details (id, name, source type, table counts).

### Refresh connection schema or data
```
hotdata connections refresh <connection_id> [--workspace-id <workspace_id>] [--data] [--schema <name> --table <name>] [--async] [--include-uncached]
```
- Default (no flags) refreshes the connection’s catalog so new or changed tables and columns appear in `hotdata tables list` and queries. Use after DDL or other changes in the source database when the workspace view is stale.
- `--data` re-syncs cached row data from the source instead of refreshing the catalog.
- `--schema` and `--table` narrow a data refresh to a single table (must be supplied together).
- `--async` submits a data refresh as a background job and returns a job ID; poll with `hotdata jobs <job_id>`. Only valid with `--data` — schema refresh is always synchronous.
- `--include-uncached` includes tables that haven't been cached yet in a connection-wide data refresh. Only valid with `--data` and no `--table`.

### Create a Connection

#### Step 1 — Discover available connection types
```
hotdata connections create list [--workspace-id <workspace_id>] [--output table|json|yaml]
```
Returns all available connection types with `name` and `label`.

#### Step 2 — Inspect the schema for a specific type
```
hotdata connections create list <name> [--workspace-id <workspace_id>] [--output json]
```
Returns `config` and `auth` JSON Schema objects describing all required and optional fields for that connection type. Use **`--output json`** to get the full schema detail.

- `config` — connection configuration fields (host, port, database, etc.). May be `null` for services that need no configuration.
- `auth` — authentication fields (password, token, credentials, etc.). May be `null` for services that need no authentication. May be a `oneOf` with multiple authentication method options.

#### Step 3 — Create the connection
```
hotdata connections create \
  --name "my-connection" \
  --type <source_type> \
  --config '<json object>' \
  [--workspace-id <workspace_id>] [--output table|json|yaml]
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
hotdata tables list [--workspace-id <workspace_id>] [--connection-id <connection_id>] [--schema <pattern>] [--table <pattern>] [--limit <int>] [--cursor <cursor>] [--output table|json|yaml]
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
hotdata datasets list [--workspace-id <workspace_id>] [--limit <int>] [--offset <int>] [--output table|json|yaml]
```
- Default format is `table`.
- Returns `id`, `label`, and `created_at`; table output includes a **`FULL NAME`** column (`datasets.<schema>.<table>`).
- Results are paginated (default 100). Use `--offset` to fetch further pages.
- **There is no filter for “this sandbox only.”** `datasets list` always returns **all** datasets in the workspace. To tell sandbox-scoped datasets from workspace-wide ones, read **`FULL NAME`**: the middle segment is the sandbox id (e.g. `datasets.s_ufmblmvq.tac_csat`) for sandbox data, and usually **`main`** (e.g. `datasets.main.my_table`) for ordinary uploads.

#### Get dataset details
```
hotdata datasets <dataset_id> [--workspace-id <workspace_id>] [--output table|json|yaml]
```
- Shows dataset metadata and a full column listing with `name`, `data_type`, `nullable`.
- Use this to inspect schema before querying.
- For the **qualified SQL name**, prefer **`FULL NAME` from `datasets list`** or the **`full_name` printed by `datasets create`**—especially for sandbox datasets, where the schema is **`datasets.<sandbox_id>`**, not `datasets.main`.

#### Update a dataset
```
hotdata datasets update <dataset_id> [--label <label>] [--table-name <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
```
- The CLI requires **at least one** of **`--label`** or **`--table-name`**.

#### Create a dataset
```
hotdata datasets create --label "My Dataset" --file data.csv [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --sql "SELECT * FROM ..." [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --query-id <saved_query_id> [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --url "https://example.com/data.parquet" [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --upload-id <upload_id> [--format csv|json|parquet] [--table-name my_dataset] [--workspace-id <workspace_id>]
```
- `--file` uploads a local file. Omit to pipe data via stdin: `cat data.csv | hotdata datasets create --label "My Dataset"`
- `--sql` creates a dataset from a SQL query result.
- `--query-id` creates a dataset from a previously saved query.
- `--url` imports data directly from a URL (supports csv, json, parquet).
- `--upload-id` uses an upload the API already accepted; **`--format`** (default `csv`) applies only with `--upload-id`.
- `--file`, `--sql`, `--query-id`, `--url`, and `--upload-id` are mutually exclusive.
- Format is auto-detected from file extension (`.csv`, `.json`, `.parquet`) or file content.
- `--label` is optional when `--file` is provided — defaults to the filename without extension. Required for `--sql` and `--query-id`.
- `--table-name` is optional — derived from the label if omitted.
- After **`datasets create`**, the CLI prints a **`full_name`** line (for example `datasets.main.my_table` or `datasets.s_ufmblmvq.tac_csat`). **Always use that `full_name` in SQL**—do not assume `datasets.main`.

#### Refresh a dataset
```
hotdata datasets refresh <dataset_id> [--workspace-id <workspace_id>] [--async]
```
- Re-runs the dataset's source (URL fetch or saved query) and creates a **new version**. Use after the upstream source has changed.
- **Not supported for upload-source datasets** — those have no remote source to re-pull from. The CLI surfaces the server's `400` directly.
- `--async` submits the refresh as a background job and returns a `job_id`; poll with **`hotdata jobs <job_id>`**.

#### Querying datasets

Qualified dataset tables are **`datasets.<schema>.<table_name>`**: **`main`** for workspace-scoped datasets (created outside a sandbox), or the **sandbox id** for sandbox-created data (e.g. `datasets.s_ufmblmvq.tac_csat`). The create output’s **`full_name`** is authoritative—copy it into `FROM` / `JOIN` clauses instead of guessing `datasets.main.…`.

Example (workspace dataset on `main`):
```
hotdata query "SELECT * FROM datasets.main.my_dataset LIMIT 10"
```

Use `hotdata datasets <dataset_id>` to inspect schema and names before writing queries.

### Workspace context (named Markdown)

Reads and writes workspace **context API** documents. **`show`** needs no local file; **`push`** / **`pull`** use **`./<NAME>.md`** in the current directory only as the CLI transport format. See [Workspace context (API)](#workspace-context-api).

```
hotdata context list [--workspace-id <workspace_id>] [--prefix <stem>] [--output table|json|yaml]
hotdata context show <name> [--workspace-id <workspace_id>]
hotdata context pull <name> [--workspace-id <workspace_id>] [--force] [--dry-run]
hotdata context push <name> [--workspace-id <workspace_id>] [--dry-run]
```

- `list` — names, `updated_at`, and character counts for each stored context. Use `--prefix` to narrow names (case-sensitive). **Agents:** call **`list` before `show`** for `DATAMODEL` (or any stem) so you do not rely on `show` failing when the document does not exist yet.
- `show` — print the Markdown body to **stdout** (use this when there is **no** local `./<NAME>.md`; ideal for agents). **Errors** if no context with that `name` exists (exit 1)—expected for a new workspace; use `list` first to avoid that path.
- `pull` — download context `name` to `./<NAME>.md`. Refuses to overwrite an existing file unless `--force`. `--dry-run` prints target path and size only.
- `push` — upload `./<NAME>.md` to upsert context `name` on the server. `--dry-run` prints size only. Body size must stay within the API limit (order of 512k characters).

**Convention:** **context:DATAMODEL** is the primary workspace semantic map; **context:GLOSSARY** (or other **`context:<STEM>`** docs) for additional narrative context. Same identifier rules as SQL table names. CLI: `hotdata context show DATAMODEL` (bare stem).

### Execute SQL Query
```
hotdata query "<sql>" [--workspace-id <workspace_id>] [--connection <connection_id>] [--output table|json|csv]
hotdata query status <query_run_id> [--output table|json|csv]
```
- Default output is `table`, which prints results with row count and execution time.
- Use `--connection` to scope the query to a specific connection.
- Use `hotdata tables list` to discover tables and columns — do not query `information_schema` directly.
- **Always use PostgreSQL dialect SQL.** Column names that are **not** all-lowercase (e.g. from CSV headers like `CustomerName`) are **case-sensitive**; quote them with **double quotes** in SQL, e.g. `"CustomerName"`.
- Long-running queries automatically fall back to async execution and return a `query_run_id`.
- Use `hotdata query status <query_run_id>` to poll for results.
- Exit codes for `query status`: `0` = succeeded, `1` = failed, `2` = still running (poll again).
- **When a query returns a `query_run_id`, use `query status` to poll rather than re-running the query.**

### Query results
#### List stored results
```
hotdata results list [--workspace-id <workspace_id>] [--limit <int>] [--offset <int>] [--output table|json|yaml]
```
- Lists recent stored query results with `id`, `status`, and `created_at`.
- Results are paginated; when more are available, the CLI prints a hint with the next `--offset`.
- Use a row’s `id` with `hotdata results <result_id>` below.

#### Get result by ID
```
hotdata results <result_id> [--workspace-id <workspace_id>] [--output table|json|csv]
```
- Retrieves a previously executed query result by its result ID.
- Query output also includes a `result-id` in the footer (e.g. `[result-id: rslt...]`).
- **Always use `results list` / `results <id>` to retrieve past query results rather than re-running the same query.** Re-running queries wastes resources and may return different results.

### Query Run History
```
hotdata queries list [--limit <int>] [--cursor <token>] [--status <csv>] [--output table|json|yaml]
hotdata queries <query_run_id> [--output table|json|yaml]
```
These commands use the **active workspace only** (the `queries` command has no `--workspace-id` flag); set the default workspace with `workspaces set` if needed.
- `list` shows query runs with status, creation time, duration, row count, and a truncated SQL preview (default limit 20).
- `--status` filters by run status (comma-separated, e.g. `--status running,failed`).
- View a run by ID to see full metadata (timings, `result_id`, snapshot, hashes) and the formatted, syntax-highlighted SQL.
- If a run has a `result_id`, fetch its rows with `hotdata results <result_id>`.

To create a dataset from a **saved query** still registered for the workspace, use **`hotdata datasets create --query-id <saved_query_id>`** (this CLI does not expose separate saved-query create/run subcommands).

### Search

`--type` is **required**. Pass `vector` or `bm25`. Both run entirely server-side.

```
# BM25 full-text search (requires BM25 index on the column)
hotdata search "<query>" --type bm25 --table <connection.schema.table> --column <column> \
  [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]

# Vector similarity search via server-side auto-embed (requires a vector index on the column)
hotdata search "<query>" --type vector --table <connection.schema.table> --column <source_text_column> \
  [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]
```
- **`--type vector`** — pass the query as **plain text** and name the **source text column** (e.g. `title`). The server embeds the query at the same time, using the same provider that auto-embedded the column when the index was built — distance metric, model, and dimensions match automatically. No client-side embedding, no `OPENAI_API_KEY` required. Generated SQL: `vector_distance(col, 'text')`.
- **`--type bm25`** generates `bm25_search(table, col, 'text')` server-side; requires a BM25 index on the column.
- **No vector index on the column, or want a different embedding model?** `hotdata search` won't help — drop down to raw SQL via `hotdata query` (e.g. `SELECT *, cosine_distance(col, [<vec>]) FROM ...`). See the SQL reference for available distance functions and table UDFs.
- BM25 results sort by score (descending). Vector results sort by distance (ascending).
- `--select` specifies which columns to return (comma-separated, defaults to all).
- Default limit is 10.
- **For BM25 search, create a BM25 index on the target column first (`hotdata indexes create ... --type bm25`). For vector search, create a vector index, optionally with auto-embedding on a text column.**
- The earlier `--model` flag and stdin-piped-vector path have both been removed. They hardcoded `l2_distance` regardless of the index's metric (silently wrong on cosine indexes). For client-side embedding or precomputed-vector workflows, use raw SQL via `hotdata query`.

### Indexes

Indexes attach to either a connection-table (`--connection-id` + `--schema` + `--table`) or a dataset (`--dataset-id`) — the two scopes are mutually exclusive for **create** / **delete**. **`indexes list`** supports three ways to scope (below).

For **create**, `--type` is required (no default).

```
# List — default: all indexes on connection tables in the workspace (from information_schema; parallel fetch).
# Narrow the scan with any subset of: --connection-id (-c), --schema, --table. With all three set, uses one table API call.
# Dataset indexes are not included; use --dataset-id per dataset.
hotdata indexes list [--connection-id <connection_id>] [--schema <schema>] [--table <table>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata indexes list --dataset-id <dataset_id> [--workspace-id <workspace_id>] [--output table|json|yaml]

# Connection-table scope — create / delete
hotdata indexes create --connection-id <connection_id> --schema <schema> --table <table> \
  --name <name> --columns <cols> --type sorted|bm25|vector \
  [--metric l2|cosine|dot] [--async] \
  [--embedding-provider-id <id>] [--dimensions <n>] [--output-column <name>] [--description <text>]
hotdata indexes delete --connection-id <connection_id> --schema <schema> --table <table> --name <name>

# Dataset scope — create / delete (same --dataset-id flag)
hotdata indexes create --dataset-id <dataset_id> --name <name> --columns <cols> --type sorted|bm25|vector ...
hotdata indexes delete --dataset-id <dataset_id> --name <name>
```
- **`indexes list`:** With no `--dataset-id`, lists indexes on **connection** tables (workspace scan or filtered scan). **Dataset** indexes are listed only via `--dataset-id` (one dataset per invocation).
- `--type` accepts `sorted` (B-tree-like; range/exact lookups), `bm25` (full-text), or `vector` (similarity). It is **required** for **create**.
- `--type vector` requires exactly one column.
- `--async` submits index creation as a background job; poll with `hotdata jobs <job_id>`.
- **Auto-embedding:** with `--type vector` on a **text** column, the server generates embeddings automatically. Pass `--embedding-provider-id` to pick a specific provider; if omitted, the first system provider is used. The generated column defaults to `{column}_embedding` (override with `--output-column`).

### Embedding providers
```
hotdata embedding-providers list [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata embedding-providers get <id> [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata embedding-providers create --name <name> --provider-type service|local \
  [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>]
hotdata embedding-providers update <id> [--name <name>] [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata embedding-providers delete <id> [--workspace-id <workspace_id>]
```
- System providers (e.g. `sys_emb_openai`) come pre-configured. `list` shows IDs to pass to `--embedding-provider-id`.
- `--provider-api-key` (the embedding service's own key, e.g. an OpenAI `sk-...`) auto-creates a managed secret. Pairs with `--provider-type`; named to avoid colliding with the global `--api-key` (Hotdata auth). `--secret-name` references an existing secret. Mutually exclusive.

### Jobs
```
hotdata jobs list [--workspace-id <workspace_id>] [--job-type <type>] [--status <status>] [--all] [--limit <n>] [--offset <n>] [--output table|json|yaml]
hotdata jobs <job_id> [--workspace-id <workspace_id>] [--output table|json|yaml]
```
- `list` shows only active jobs (`pending`, `running`) by default. Use `--all` to see all jobs.
- `--job-type`: `data_refresh_table`, `data_refresh_connection`, `dataset_refresh`, `create_index`, `create_dataset_index`.
- `--status`: `pending`, `running`, `succeeded`, `partially_succeeded`, `failed`.
- Use `hotdata jobs <job_id>` to inspect a specific job's status, error, and result.

### Agent skills (`skills`)

Bundled Markdown skills (**`hotdata`**, **`hotdata-geospatial`**) ship with the CLI release tarball.

```
hotdata skills install [--project]
hotdata skills status
```

- **`install`** — Downloads and installs skills to **`~/.hotdata/skills/<skill>`**, then symlinks into **`~/.agents/skills`** and into **`~/.claude/skills`** / **`~/.pi/skills`** when those directories exist. **`--project`** instead copies into **`./.agents/skills/<skill>`** in the current directory (and links `./.claude` / `./.pi` when present). The CLI may auto-refresh skills after an upgrade when appropriate.
- **`status`** — Reports installed vs current CLI version and where skills are linked.

### Shell completions

```
hotdata completions <bash|zsh|fish>
```

Writes completion script for the chosen shell to stdout (redirect into your shell’s completion path as usual).

### Auth
```
hotdata auth login          # Browser-based login (same as: hotdata auth)
hotdata auth                # Browser-based login (same as: hotdata auth login)
hotdata auth status         # Check current auth status
hotdata auth logout         # Remove saved auth for the default profile
```

### Sandboxes

Sandboxes are for **ad-hoc, exploratory work** that does not need to be long-lived. They group related CLI activity (queries, dataset operations, etc.) under a single context so it can be tracked and cleaned up together. **Datasets created inside a sandbox are tied to that sandbox and will be removed when the sandbox ends.** If you need data to persist beyond the sandbox, create datasets outside of a sandbox context.

**Active sandbox in config vs `sandbox run`:** If you already have the right sandbox selected (`hotdata sandbox new` or `hotdata sandbox set <sandbox_id>` shows it with `*` in `sandbox list`), run follow-up commands **directly** (`hotdata datasets create …`, `hotdata query …`, etc.). The CLI attaches the sandbox from saved config to API requests. **`hotdata sandbox run <cmd>` with no sandbox ID before `run` always creates a brand-new sandbox** and runs the child under that new ID—it does **not** reuse the active sandbox from config. To wrap a command in an **existing** sandbox, use **`hotdata sandbox <sandbox_id> run <cmd> [args…]`**.

> **IMPORTANT: If `HOTDATA_SANDBOX` is set in the environment, you are inside an active sandbox. NEVER attempt to unset, override, or work around this variable. Do not clear it, do not start a new sandbox, do not run `sandbox run` or `sandbox new` or `sandbox set`. All your work should be attributed to the current sandbox. Attempting to nest or escape a sandbox will fail with an error.**

```
hotdata sandbox list [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata sandbox <sandbox_id> [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata sandbox new [--name "Sandbox Name"] [--output table|json|yaml]
hotdata sandbox set [<sandbox_id>]
hotdata sandbox read
hotdata sandbox update [<sandbox_id>] [--name "New Name"] [--markdown "..."] [--output table|json|yaml]
hotdata sandbox run <cmd> [args...]
hotdata sandbox <sandbox_id> run <cmd> [args...]
```

- `list` shows all sandboxes with a `*` marker on the active one.
- `new` creates a sandbox and sets it as active. Blocked inside an existing sandbox.
- `set` switches the active sandbox. Omit the ID to clear. Blocked inside an existing sandbox.
- `read` prints the markdown content of the current sandbox. Use this to retrieve sandbox state at the start of work or between steps.
- `update` modifies a sandbox's name or markdown. Defaults to the active sandbox if no ID is given. The `--markdown` field is for writing details about the work being done in the sandbox — observations, intermediate findings, next steps, etc. This state persists for the life of the sandbox and is the primary way to record context that should survive across commands or agent invocations within the sandbox.
- `run` launches a command with `HOTDATA_SANDBOX` and `HOTDATA_WORKSPACE` set in the child process environment. **`hotdata sandbox run <cmd>`** (no ID before `run`) **always POSTs a new sandbox**; it never picks up the active sandbox from `sandbox set` / `sandbox new`. Use **`hotdata sandbox <sandbox_id> run <cmd>`** to run under an existing sandbox. Blocked inside an existing sandbox.
- When `HOTDATA_SANDBOX` is set **or** a sandbox is the saved default (`sandbox new` / `sandbox set`), the CLI includes sandbox scope on API calls — no extra sandbox flags on `query`, `datasets`, etc.

**Sandbox-scoped data access:** Queries and other operations against **sandbox-only** resources must run with sandbox context attached—either the **active sandbox** in config (`sandbox set`) or a child process started with **`hotdata sandbox <sandbox_id> run …`** (which sets `HOTDATA_SANDBOX`). Running `hotdata query` or similar **with no sandbox in config and not under `sandbox … run`** can produce **access denied** for tables or datasets that exist only inside a sandbox.

#### Example: Building a sales pipeline

Use a sandbox to explore tables and capture **analysis-oriented** notes in sandbox markdown (keys, joins, open questions)—**day-to-day modeling** for this investigation, not **context:DATAMODEL** until you promote it.

1. Start a sandbox:
   ```
   hotdata sandbox new --name "Sales pipeline"
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
5. Continue exploring and update the markdown as your **analysis picture** takes shape. Sandbox markdown is the living artifact for **that sandbox** only.
6. When that picture should become **context:DATAMODEL** (outlive the sandbox or be shared with everyone), promote it: save consolidated Markdown as `./DATAMODEL.md` in the project directory and run `hotdata context push DATAMODEL` (if **context:DATAMODEL** already exists on the server, merge with `hotdata context show DATAMODEL` first—confirm `DATAMODEL` appears in `hotdata context list` before `show`).

**Also available:** `hotdata connections new` — interactive connection wizard (no substitute for the programmatic **`connections create`** flow above).

## Workflow: Running a Query

0. (Recommended for agents) When the query depends on **workspace-wide** table relationships or naming conventions, run **`hotdata context list`** first; **only if** `DATAMODEL` is listed, run `hotdata context show DATAMODEL` to load **context:DATAMODEL**. If it is **not** listed, **do not** run `show`—ad-hoc analysis does not require populated **context:DATAMODEL**.
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
4. Run SQL, quoting **mixed-case or upper-case** column names with **double quotes** (PostgreSQL treats unquoted identifiers as lowercased):
   ```
   hotdata query "SELECT 1"
   hotdata query "SELECT \"CustomerName\" FROM datasets.main.my_csv LIMIT 10"
   ```

## Workflow: Creating a Connection

1. List available connection types:
   ```
   hotdata connections create list
   ```
2. Inspect the schema for the desired type:
   ```
   hotdata connections create list <type_name> --output json
   ```
3. Collect required config and auth field values from the user or environment. **Never hardcode credentials — use env vars or files.**
4. Create the connection:
   ```
   hotdata connections create --name "my-connection" --type <type_name> --config '<json>'
   ```
