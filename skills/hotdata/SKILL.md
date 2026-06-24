---
name: hotdata
description: Use this skill when the user wants to run core hotdata CLI commands — auth, workspaces, connections, managed databases, tables, basic SQL query, database context (context:DATAMODEL), jobs, and skill install. Activate for "run hotdata", "list workspaces", "list connections", "create a connection", "list databases", "managed database", "load parquet", "list tables", "execute a query", "database context", "context:DATAMODEL", or general Hotdata CLI usage. For full-text/vector search and retrieval indexes use hotdata-search; for OLAP analytics, query history, stored results, and Chain materializations use hotdata-analytics; for geospatial/GIS use hotdata-geospatial.
version: 0.7.0
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
| **`hotdata`** (this file) | Auth, workspaces, connections, databases, tables, basic `query`, context, jobs |
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

### Cold starts (worker wake-up)

A workspace's query worker scales to zero after inactivity. The **first** command against an idle workspace (e.g. `databases list`, `query`, `search`) blocks while it wakes — typically ~10s, up to ~20s — and the spinner upgrades to `waking up worker after inactivity (this can take ~20s)…`. **This is normal, not a hang:** don't kill the command, retry, or treat the pause as an error. Subsequent commands return promptly; warm workspaces are unaffected.

## Database context (API)

**`context:<STEM>`** (e.g. **context:DATAMODEL**, **context:GLOSSARY**) is an authoritative Markdown document stored server-side under that stem via the context API — *not* generic English ("a data model"), and *not* a local `./DATAMODEL.md` (local files are only `push`/`pull` transport). CLI commands take the bare stem: `hotdata context show DATAMODEL`. Context is scoped to the **active database** (`hotdata databases set <id>`); target another with `--database-id` / `-d`. Stems follow SQL identifier rules and accept a trailing `.md` (stored without it). Command reference: [Database context (named Markdown)](#database-context-named-markdown).

**Agents — list before show.** Run `hotdata context list` (optionally `--prefix DATAMODEL`) first; run `hotdata context show DATAMODEL` *only if* the stem is listed. A missing stem makes `show` exit 1 — normal for a fresh database, not a failure: don't retry in a loop or run speculative `show` in parallel with other tools. Proceed without context:DATAMODEL until one exists.

**context:DATAMODEL is the durable, shared store** — entities, keys, cross-connection joins, and the naming/query conventions the whole team relies on. Keep task-scoped exploration (scratch SQL, hypotheses, one-off join checks) in the conversation or local notes; **promote** to context:DATAMODEL only when findings should outlive the session and guide everyone — reconcile against `context show DATAMODEL` (if listed), write `./DATAMODEL.md`, then `hotdata context push DATAMODEL`. No need to update it after every ad-hoc query. What to write inside the document: [references/DATA_MODEL.template.md](references/DATA_MODEL.template.md) and [references/MODEL_BUILD.md](references/MODEL_BUILD.md).

## Multi-step workflows

These are **patterns** built from the commands below—not separate CLI subcommands:

- **Model (`context:DATAMODEL`)** — The shared semantic map of the active database (entities, keys, joins across connections). Store and read it only via database context (`hotdata context list`, then `show DATAMODEL` **only when listed**, `push DATAMODEL`); refresh using `connections`, `connections refresh`, and `tables list`. For a deep pass (connector enrichment, indexes, per-table detail), see [references/MODEL_BUILD.md](references/MODEL_BUILD.md).
- **History / Chain / OLAP SQL** — See **`hotdata-analytics`** and [references/WORKFLOWS.md](references/WORKFLOWS.md).
- **Search / retrieval indexes** — See **`hotdata-search`**.

Catalog, skill decision tree, epic flows (onboard, chain, retrieval), and managed databases: [references/WORKFLOWS.md](references/WORKFLOWS.md).

## Available Commands

Top-level subcommands (each detailed below): **`auth`**, **`query`**, **`workspaces`**, **`connections`**, **`databases`**, **`tables`**, **`skills`**, **`results`**, **`jobs`**, **`indexes`**, **`embedding-providers`**, **`search`**, **`queries`**, **`context`**, **`usage`**, **`completions`**, **`upgrade`**. Search, indexes (bm25/vector), and embedding providers are documented in **`hotdata-search`**; query history, results, Chain, and OLAP patterns in **`hotdata-analytics`**.

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

**Parquet only:** `databases tables load` accepts **parquet** files (local `--file`, remote `--url`, or a pre-staged `--upload-id`).

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

# Attach a connection as a queryable catalog (enables cross-source queries — see below)
hotdata databases attach <connection_id|name> [--database <id>] [--alias <alias>]
hotdata databases detach <connection_id|name|alias> [--database <id>]

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
- `attach` — attaches a **connection** as a queryable catalog on a managed database, so the connection's **live** tables become visible inside that database's query scope. Defaults to the active database; target another with `--database`. `--alias` sets the SQL name the catalog answers to (defaults to the connection's name). This is how you query connection tables and **join across sources** — see [Querying across connections](#querying-across-connections-attach).
- `detach` — removes an attached connection catalog. Accepts the connection name/id **or** the alias you attached it under. Defaults to the active database.
- `create --attach <connection>[=<alias>]` — attach one or more connections at creation time (repeatable), e.g. `--attach github --attach salesdb=sales`.

Example:

```
hotdata databases create --catalog airbnb
hotdata databases load --catalog airbnb --table listings --url https://example.com/listings.parquet
hotdata query "SELECT count(*) FROM airbnb.public.listings"
```

#### Querying across connections (attach)

**A `hotdata query` runs inside exactly one managed database** — the active database (`hotdata databases set <id>`) or the one named by `--database`. With none set, the query fails with *"a database is required."* That database's query scope sees **only its own catalog plus any connection catalogs explicitly attached to it** — a workspace connection is **not** visible just because it exists. Referencing an unattached connection fails with *"table '\<catalog\>.\<schema\>.\<table\>' not found."*

To query a connection's tables, or **join a managed table against a connection table in one query**, attach the connection to the database first. The connection's data stays **live** (synced) — this is not a copy:

```
# Attach the 'github' connection (live) to the active database under alias 'gh'
hotdata databases attach github --alias gh

# Now both the database's own tables and the attached connection are in scope:
hotdata query "SELECT * FROM gh.github.issues WHERE state = 'OPEN' LIMIT 10"

# Cross-source join: a managed table JOINed against the live connection table
hotdata query "
  SELECT t.id, i.title
  FROM mycatalog.public.tickets t
  JOIN gh.github.issues i ON i.number = t.gh_issue
"

hotdata databases detach gh   # when finished (optional)
```

Without `--alias`, the catalog answers to the connection's own name (`github.github.issues`). Do **not** export a connection to parquet just to query it — attach is the live, sync-preserving path.

### List Tables and Columns
```
hotdata tables list [--workspace-id <workspace_id>] [--connection-id <connection_id>] [--schema <pattern>] [--table <pattern>] [--limit <int>] [--cursor <cursor>] [--output table|json|yaml]
```
- Default format is `table`.
- **Always use this command to inspect available tables and columns.** Do NOT use the `query` command to query `information_schema` for this purpose.
- Without `--connection-id`: lists all tables with `table`, `synced`, `last_sync`. The `table` column is formatted as `<connection>.<schema>.<table>`.
- With `--connection-id`: includes column definitions. Lists each column as its own row with `table`, `column`, `data_type`, `nullable`. Use this to inspect the schema before writing queries.
- **Always use the full `<connection>.<schema>.<table>` name when referencing tables in SQL queries.** A connection table is only queryable once its connection is attached to the active database (`hotdata databases attach <connection>`); see [Querying across connections (attach)](#querying-across-connections-attach).
- `--schema` and `--table` support SQL `%` wildcard patterns (e.g. `--table order%` matches `orders`, `order_items`, etc.).
- Results are paginated (default 100 per page). If more results are available, a `--cursor` token is printed — pass it to fetch the next page.

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
hotdata query status <query_run_id>
```

- Default output is `table` (row count and execution time).
- **A query runs inside one managed database** (active database or `--database`); with none set it fails *"a database is required."* The scope sees the database's own catalog **plus any attached connection catalogs only**. To query a connection's tables or join across sources, attach the connection first — see [Querying across connections (attach)](#querying-across-connections-attach).
- Use `hotdata tables list` for discovery — not `information_schema` via `query`. (Discovery lists every workspace table; queryability still requires the table's catalog to be in the active database's scope.)
- **PostgreSQL dialect.** Quote non-lowercase columns with double quotes.
- Async runs return `query_run_id` → poll with `query status` (do not re-run the same heavy SQL).
- **Large results are complete, not a preview.** The server returns inline rows only up to a bounded cap and persists the full set out-of-band; `hotdata query` transparently fetches the full result, so the printed rows and row count are the complete set. (If the full result can't be retrieved, the CLI prints the preview and a `warning:` to stderr.)
- **Backpressure is handled.** Under heavy concurrent load the server may shed a query with HTTP 429 (`OVERLOADED`); the CLI auto-retries (honoring `Retry-After`) before surfacing an error — no manual retry needed.
- **OLAP** (aggregations, history, Chain, sorted indexes): **`hotdata-analytics`** skill.
- **Search** (BM25, vector): **`hotdata-search`** skill.

### Jobs
```
hotdata jobs list [--workspace-id <workspace_id>] [--job-type <type>] [--status <status>] [--all] [--limit <n>] [--offset <n>] [--output table|json|yaml]
hotdata jobs <job_id> [--workspace-id <workspace_id>] [--output table|json|yaml]
```
- `list` shows only active jobs (`pending`, `running`) by default. Use `--all` to see all jobs.
- `--job-type`: `data_refresh_table`, `data_refresh_connection`, `create_index`.
- `--status`: `pending`, `running`, `succeeded`, `partially_succeeded`, `failed`.
- Use `hotdata jobs <job_id>` to inspect a specific job's status, error, and result.

### Usage
```
hotdata usage [--since <rfc3339>] [--workspace-id <workspace_id>] [--output table|json|yaml]
```
Workspace usage for the current billing window (or since `--since`): `query_count`, `bytes_scanned`, `storage_bytes`, and `storage_captured_at`.
- `query_count` and `bytes_scanned` accrue **per query in real time** (data reads).
- `storage_bytes` is a **periodic snapshot** taken at `storage_captured_at`, so it reflects uploads only after the next capture — not instantly.
- Table output renders byte counts human-readably (raw integers in `-o json`/`yaml`).

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

### Upgrade (`upgrade`)

```
hotdata upgrade
```

Upgrades the CLI in place to the latest release (`brew upgrade` for Homebrew installs, otherwise a direct binary download), refreshing bundled skills to match. After a successful upgrade, re-run your command.

A newer release can be incompatible with the API, so in an **interactive terminal** the CLI checks for a new release before running any API-touching command and prompts to upgrade. Declining (or `Ctrl-D`) exits without running the command — `hotdata upgrade` is then required to continue. The check is a **no-op in non-interactive sessions** (no TTY, `--no-input`, or `HOTDATA_NO_UPDATE_CHECK` set), so typical agent and CI usage is never blocked; set `HOTDATA_NO_UPDATE_CHECK=1` to disable it entirely.

### Auth
```
hotdata auth login          # Browser-based login (same as: hotdata auth)
hotdata auth                # Browser-based login (same as: hotdata auth login)
hotdata auth status         # Check current auth status
hotdata auth logout         # Remove saved auth for the default profile
```

### Interactive connection wizard

`hotdata connections new` creates a connection interactively (human-friendly); agents should prefer the programmatic `connections create` flow above.

## Workflows

End-to-end recipes — onboard a workspace, run a query, build a managed database (parquet), chain/materialize, add retrieval indexes — live in [references/WORKFLOWS.md](references/WORKFLOWS.md). The command sections above are the per-command reference; the workflows stitch them into sequences.
