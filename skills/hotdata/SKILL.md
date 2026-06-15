---
name: hotdata
description: Use this skill when the user wants to run core hotdata CLI commands — auth, workspaces, connections, managed databases, datasets, tables, basic SQL query, database context (context:DATAMODEL), jobs, and skill install. Activate for "run hotdata", "list workspaces", "list connections", "create a connection", "list databases", "managed database", "load parquet", "list tables", "list datasets", "create a dataset", "execute a query", "database context", "context:DATAMODEL", or general Hotdata CLI usage. For full-text/vector search and retrieval indexes use hotdata-search; for OLAP analytics, query history, stored results, and Chain materializations use hotdata-analytics; for geospatial/GIS use hotdata-geospatial.
version: 0.4.1
---

# Hotdata CLI Skill

Use the `hotdata` CLI to interact with the Hotdata service. In this project, run it as:

```
hotdata <command> [args]
```

Or if installed on PATH: `hotdata <command> [args]`

## Bundled sub-skills

Install all skills with **`hotdata skills install`**. Load specialized skills only when the task needs them:

| Skill | Use for |
|-------|---------|
| **`hotdata`** (this file) | Auth, workspaces, connections, databases, datasets, tables, basic `query`, context, jobs |
| **`hotdata-search`** | BM25, vector search, `hotdata search`, bm25/vector indexes, embedding providers |
| **`hotdata-analytics`** | OLAP SQL, aggregations, query/results history, Chain materializations, sorted indexes |
| **`hotdata-geospatial`** | PostGIS-style `ST_*`, WKB, spatial joins |

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

If **`HOTDATA_WORKSPACE`** is set in the environment, the workspace is **locked** to that value: passing a different `--workspace-id` is an error, and **`hotdata workspaces set` fails** (“workspace is locked”).

**Omit `--workspace-id` unless you need to target a specific workspace** (and it is not locked by env or session).

## Database context (API)

**Notation `context:<STEM>`:** In this skill, **`context:DATAMODEL`**, **`context:GLOSSARY`**, and **`context:<NAME>`** mean the **authoritative Markdown document** stored on the server under that **stem** via the Hotdata **context API** (`/v1/databases/{database_id}/context`, `hotdata context …`). That is **not** the same as generic English (“a data model”, “a glossary”), and **not** the same as local `./DATAMODEL.md` except as **pull/push transport**. **CLI commands use the bare stem** (no `context:` prefix): e.g. `hotdata context show DATAMODEL`, `hotdata context push GLOSSARY`.

Context is **scoped to the active database** (set via `hotdata databases set <id>`). All context commands operate against the database returned by the active-database config unless you pass **`--database-id <id>`** (short: **`-d`**) explicitly. The **authoritative** copy always lives on the server under the stem; common stems are **`context:DATAMODEL`** (semantic map) and **`context:GLOSSARY`** (glossary / runbooks).

The CLI command **`hotdata context push`** reads **`./<NAME>.md`** and **`pull`** writes that file in the **current working directory**—those files exist only as a **transport surface** for the API, not as a second source of truth. **`hotdata context show <name>`** prints Markdown to stdout so agents can read **`context:<NAME>`** without any local file. Stems follow SQL table–identifier rules (ASCII letters, digits, underscore; no dot in the API name; max 128 characters; SQL reserved words are not allowed). For **`show`**, **`pull`**, and **`push`**, the CLI accepts a trailing **`.md`** on the argument (e.g. **`USER.md`**) and treats it as stem **`USER`**—the database still stores **`USER`**, not `USER.md`.

> **Agents: do not blindly run `hotdata context show DATAMODEL` on session start.** Run **`hotdata context list`** first (optional `--prefix DATAMODEL`). Call **`hotdata context show DATAMODEL` only if** the list includes the `DATAMODEL` stem. If **`show` exits 1** with *no context named …*, that is **normal** when nothing has been pushed yet—**not a hard failure**; do not retry in a loop, and **avoid speculative `show` in parallel** with other shell tools where one failure cancels sibling calls. Proceed without **context:DATAMODEL** until the user asks to create or load one.

**Agents (Claude and similar):** use database context as the only durable store for **context:DATAMODEL**, **context:GLOSSARY**, and any other **`context:<STEM>`** documents you introduce. Keep transient analysis notes in the conversation or local scratch until you **promote** them into **context:DATAMODEL** when they should guide the whole database ([details below](#analysis-modeling-vs-contextdatamodel)).

1. **Before** planning non-trivial queries, explaining schema to others, or editing **context:DATAMODEL**, **discover** stored names with `hotdata context list` (and other stems such as **context:GLOSSARY** as needed). **Only if** `DATAMODEL` appears in the list, load it: `hotdata context show DATAMODEL`. If it is **absent**, skip `show` and treat **context:DATAMODEL** as unset—use [references/DATA_MODEL.template.md](references/DATA_MODEL.template.md) when the user wants to bootstrap, then `push` when ready.
2. **After** you change **context:DATAMODEL**, persist with **`hotdata context push DATAMODEL`**. The CLI requires a local `./DATAMODEL.md` for that step: write the body there (from `context show`, the template, or your edits), then run `push` from the project directory.
3. Use **`hotdata context pull DATAMODEL`** when you intentionally want a local `./DATAMODEL.md` copy (for example a human editor); it still reflects API state for **context:DATAMODEL**, not a parallel document.

The standard stem for the database semantic map is **`DATAMODEL`** (skill notation **context:DATAMODEL**). Add other stems the same way (e.g. **`GLOSSARY`** → **context:GLOSSARY**) for glossary or runbooks.

### Analysis modeling vs context:DATAMODEL

Keep two layers separate:

- **Analysis modeling (day to day)** — Understanding data *for the current task*: exploratory SQL, join checks, column semantics for one report, hypotheses, scratch notes. Often conversational or short-lived. **The conversation or local scratch notes** are the right home while you explore; keep them there until you decide they are worth promoting.

- **context:DATAMODEL (Hotdata database data model)** — A **durable, database-scoped** map stored only via the **context API**: entities and tables across connections, PK/FK relationships, how datasets tie back to sources, naming and query conventions the **whole team** should rely on. This is **higher-level shared structure**, not a transcript of one investigation.

**Promotion:** When analysis findings should **outlive the current session** and **guide everyone**, merge them into **context:DATAMODEL** (`hotdata context list` → if `DATAMODEL` is listed, `hotdata context show DATAMODEL` → reconcile → `hotdata context push DATAMODEL`). You do **not** need to update **context:DATAMODEL** after every ad-hoc query—only when the database story or join graph meaningfully changes.

Use [references/DATA_MODEL.template.md](references/DATA_MODEL.template.md) and [references/MODEL_BUILD.md](references/MODEL_BUILD.md) for **what to write inside** the Markdown you store under **context:** stems. Never put database-specific model text inside agent skill install paths—only in **database context** (and transient `./<NAME>.md` for push/pull when needed).

## Multi-step workflows

These are **patterns** built from the commands below—not separate CLI subcommands:

- **Model (`context:DATAMODEL`)** — The **shared** Markdown semantic map of the active database (entities, keys, joins across connections). **Store and read it only via database context** (`hotdata context list`, then `hotdata context show DATAMODEL` **only when listed**, `context push DATAMODEL`); refresh using `connections`, `connections refresh`, `tables list`, and `datasets list`. For a **deep** pass (connector enrichment, indexes, per-table detail), see [references/MODEL_BUILD.md](references/MODEL_BUILD.md). Contrast **analysis modeling** in the conversation or local scratch (see [Analysis modeling vs context:DATAMODEL](#analysis-modeling-vs-contextdatamodel)).
- **History / Chain / OLAP SQL** — See **`hotdata-analytics`** and [references/WORKFLOWS.md](references/WORKFLOWS.md).
- **Search / retrieval indexes** — See **`hotdata-search`**.

Catalog, skill decision tree, epic flows (onboard, chain, retrieval), and datasets vs databases: [references/WORKFLOWS.md](references/WORKFLOWS.md).

## Available Commands

Top-level subcommands (each detailed below): **`auth`**, **`datasets`**, **`query`**, **`workspaces`**, **`connections`**, **`databases`**, **`tables`**, **`skills`**, **`results`**, **`jobs`**, **`indexes`**, **`embedding-providers`**, **`search`**, **`queries`**, **`context`**, **`completions`**. Search, indexes (bm25/vector), and embedding providers are documented in **`hotdata-search`**; query history, results, Chain, and OLAP patterns in **`hotdata-analytics`**.

Global CLI options: **`--api-key`**, **`-v` / `--version`**, **`-h` / `--help`**, **`--no-input`** (disable interactive prompts; commands that require input will error instead — useful in CI or non-TTY environments). Hidden developer flag: **`--debug`** (verbose HTTP logs).

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

### Managed databases (`databases`)

**Managed databases** are Hotdata-owned catalogs you create and populate yourself — no remote source to sync. Query them in SQL as **`<database_id>.<schema>.<table>`**. Prefer **`hotdata databases`** for this workflow.

**Parquet vs datasets:** `databases tables load` accepts **parquet only**. For SQL-query or saved-query materializations, use **`hotdata datasets create`**.

**Active database:** `hotdata databases set <id_or_description>` saves the active database to config. All `databases tables` subcommands and all `context` commands default to the active database; pass **`--database <id>`** to override per-command.

```
hotdata databases list [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases create [--name <display_name>] [--catalog <alias>] [--table <table> ...] [--schema public] [--expires-at <duration|timestamp>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases set <id_or_name>
hotdata databases unset
hotdata databases <id_or_name> [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases delete <id_or_name> [--workspace-id <workspace_id>]
hotdata databases run [--database <id>] [--name <label>] [--schema public] [--table <table> ...] [--expires-at <duration|timestamp>] [--workspace-id <workspace_id>] <cmd> [args...]
hotdata databases <id> run <cmd> [args...]

# Preferred: load by catalog alias (auto-declares table if needed)
hotdata databases load --catalog <alias> --table <table> [--schema public] (--file <path> | --url <url> | --upload-id <id>) [--workspace-id <workspace_id>]

# Also available via tables subcommand
hotdata databases tables list [--database <id_or_name>] [--schema <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases tables load <table> [--database <id_or_name>] [--schema public] (--file <path> | --url <url> | --upload-id <id>) [--workspace-id <workspace_id>]
hotdata databases tables delete <table> [--database <id_or_name>] [--schema public] [--workspace-id <workspace_id>]
```

- `list` — all managed databases in the workspace. Active database is marked with `*`.
- `create` — creates a new managed database. `--name` is an optional human-readable display name. `--catalog` sets the SQL alias used in queries (`SELECT … FROM <catalog>.schema.table`); must be `[a-z_][a-z0-9_]*`. `--expires-at` accepts relative durations (`24h`, `7d`, `90m`) or an RFC 3339 timestamp; omitting means no expiry. Repeat `--table` to declare tables up front.
- `set` — saves `<id_or_name>` as the active database. Subsequent `databases tables` and `context` commands use it automatically.
- `unset` — clears the active database from config.
- `<id_or_name>` — inspect one database (id, catalog, name, expires_at).
- `delete` — removes the managed database; clears the active-database config if it matched.
- `load` (top-level shorthand) — loads parquet into `--catalog.--schema.--table`. Accepts `--file`, `--url`, or `--upload-id`. If the table was not declared at create time, the CLI automatically deletes and recreates the database with the table declared, then retries the load.
- `tables list` — lists tables with `TABLE` (`<catalog>.<schema>.<table>`), `SYNCED`, `LAST_SYNC`. Uses active database when `--database` is omitted.
- `tables load` — uploads a local parquet file (`--file`), a remote parquet URL (`--url`), or a pre-staged upload (`--upload-id`) and publishes with **replace** mode.
- `tables delete` — drops a table from the managed database.
- `run` — mints a database-scoped JWT (via `POST /v1/auth/database`) and execs `<cmd>` with `HOTDATA_DATABASE_TOKEN`, `HOTDATA_DATABASE_REFRESH_TOKEN`, `HOTDATA_DATABASE`, `HOTDATA_WORKSPACE`, and `HOTDATA_API_URL` injected. Pass a database id as a group positional (`hotdata databases <id> run ...`) or via `--database <id>`; omit both to auto-create a scratch database using `--name` / `--schema` / `--table` / `--expires-at`. Use this to launch an agent or child process whose API access is scoped to a single database. The minted JWT carries `database`, `workspaces`, `permissions:["read","write"]`, `source:"database_token"`. The session is persisted at `~/.hotdata/database_session.json` (mode `0600`); the child's exit code is propagated.

Example:

```
hotdata databases create --catalog airbnb
hotdata databases load --catalog airbnb --table listings --url https://example.com/listings.parquet
hotdata query "SELECT count(*) FROM airbnb.public.listings"
```

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
- `datasets list` always returns **all** datasets in the workspace. Read **`FULL NAME`** to identify the schema: the middle segment is usually **`main`** (e.g. `datasets.main.my_table`) for ordinary uploads.

#### Get dataset details
```
hotdata datasets <dataset_id> [--workspace-id <workspace_id>] [--output table|json|yaml]
```
- Shows dataset metadata and a full column listing with `name`, `data_type`, `nullable`.
- Use this to inspect schema before querying.
- For the **qualified SQL name**, prefer **`FULL NAME` from `datasets list`** or the **`full_name` printed by `datasets create`**—do not assume `datasets.main`.

#### Update a dataset
```
hotdata datasets update <dataset_id> [--description <label>] [--name <table_name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
```
- The CLI requires **at least one** of **`--description`** or **`--name`**.

#### Create a dataset
```
hotdata datasets create --name <table_name> [--description "My Dataset"] (--sql "SELECT ..." | --query-id <saved_query_id>) [--workspace-id <workspace_id>]
```
- **`--name`** (required) — SQL table name the dataset is addressable as (e.g. `my_view`).
- **`--description`** (optional) — human-readable display label; defaults to `--name` when omitted.
- **Exactly one** of **`--sql`** or **`--query-id`** is required:
  - `--sql` — create from an inline SQL query result.
  - `--query-id` — create from a previously saved query.
- For parquet/CSV file uploads use **`hotdata databases tables load`** instead.
- After **`datasets create`**, the CLI prints a **`full_name`** line (e.g. `datasets.main.my_view`). **Always use that `full_name` in SQL**—do not assume `datasets.main`.

#### Refresh a dataset
```
hotdata datasets refresh <dataset_id> [--workspace-id <workspace_id>] [--async]
```
- Re-runs the dataset's source (URL fetch or saved query) and creates a **new version**. Use after the upstream source has changed.
- **Not supported for upload-source datasets** — those have no remote source to re-pull from. The CLI surfaces the server's `400` directly.
- `--async` submits the refresh as a background job and returns a `job_id`; poll with **`hotdata jobs <job_id>`**.

#### Querying datasets

Qualified dataset tables are **`datasets.<schema>.<table_name>`**, normally **`datasets.main.<table_name>`**. The create output’s **`full_name`** is authoritative—copy it into `FROM` / `JOIN` clauses instead of guessing `datasets.main.…`.

Example (workspace dataset on `main`):
```
hotdata query "SELECT * FROM datasets.main.my_dataset LIMIT 10"
```

Use `hotdata datasets <dataset_id>` to inspect schema and names before writing queries.

### Database context (named Markdown)

Reads and writes **database-scoped context API** documents. Context is tied to the **active database** (set via `hotdata databases set`); pass **`--database-id <id>`** (short: **`-d`**) to target a specific database. **`show`** needs no local file; **`push`** / **`pull`** use **`./<NAME>.md`** in the current directory only as the CLI transport format. See [Database context (API)](#database-context-api).

```
hotdata context list [--database-id <id>] [--prefix <stem>] [--output table|json|yaml]
hotdata context show <name> [--database-id <id>]
hotdata context pull <name> [--database-id <id>] [--force] [--dry-run]
hotdata context push <name> [--database-id <id>] [--dry-run]
```

- `list` — names, `updated_at`, and character counts for each stored context in the active database. Use `--prefix` to narrow names (case-sensitive). **Agents:** call **`list` before `show`** for `DATAMODEL` (or any stem) so you do not rely on `show` failing when the document does not exist yet.
- `show` — print the Markdown body to **stdout** (use this when there is **no** local `./<NAME>.md`; ideal for agents). **Errors** if no context with that `name` exists (exit 1)—expected for a new database; use `list` first to avoid that path.
- `pull` — download context `name` to `./<NAME>.md`. Refuses to overwrite an existing file unless `--force`. `--dry-run` prints target path and size only.
- `push` — upload `./<NAME>.md` to upsert context `name` on the server. `--dry-run` prints size only. Body size must stay within the API limit (order of 512k characters).

**Convention:** **context:DATAMODEL** is the primary database semantic map; **context:GLOSSARY** (or other **`context:<STEM>`** docs) for additional narrative context. Same identifier rules as SQL table names. CLI: `hotdata context show DATAMODEL` (bare stem).

### Execute SQL Query

```
hotdata query "<sql>" [--workspace-id <workspace_id>] [--database <database>] [--output table|json|csv]
hotdata query status <query_run_id> [--output table|json|csv]
```

- Default output is `table` (row count and execution time).
- Use `hotdata tables list` for discovery — not `information_schema` via `query`.
- **PostgreSQL dialect.** Quote non-lowercase columns with double quotes.
- Async runs return `query_run_id` → poll with `query status` (do not re-run the same heavy SQL).
- **OLAP** (aggregations, history, Chain, sorted indexes): **`hotdata-analytics`** skill.
- **Search** (BM25, vector): **`hotdata-search`** skill.

To create a dataset from a saved query: **`hotdata datasets create --query-id <saved_query_id>`**.

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

Bundled Markdown skills (**`hotdata`**, **`hotdata-search`**, **`hotdata-analytics`**, **`hotdata-geospatial`**) ship with the CLI release tarball.

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

### Analysis notes and promotion to context:DATAMODEL

Exploratory analysis notes (keys, joins, open questions for the current task) belong in **the conversation or local scratch notes**. When those findings should guide the whole database and be shared with everyone, promote them to **context:DATAMODEL**: save consolidated Markdown as `./DATAMODEL.md` and run `hotdata context push DATAMODEL` (merge with `hotdata context show DATAMODEL` first if `DATAMODEL` is already listed in `hotdata context list`).

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

## Workflow: Creating a managed database (parquet)

1. Create the database with a catalog alias:
   ```
   hotdata databases create --catalog mydb
   ```
2. Load parquet per table (tables are auto-declared if needed):
   ```
   hotdata databases load --catalog mydb --table events --file ./events.parquet
   hotdata databases load --catalog mydb --table events --url https://example.com/events.parquet
   ```
3. Confirm tables and query:
   ```
   hotdata databases tables list
   hotdata query "SELECT * FROM mydb.public.events LIMIT 10"
   ```

For CSV/JSON file uploads, use **`hotdata datasets create`** instead.

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
