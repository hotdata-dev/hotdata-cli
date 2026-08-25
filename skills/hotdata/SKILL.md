---
name: hotdata
description: Use this skill when the user wants to run core hotdata CLI commands — auth, workspaces, instant databases, tables, basic SQL query, database context (context:DATAMODEL), jobs, datasources/ingests/runs (pull external data), and skill install. Activate for "run hotdata", "list workspaces", "list databases", "instant database", "load parquet", "list tables", "show table columns", "execute a query", "database context", "context:DATAMODEL", "ingest", "datasource", "ingest run", "show a run", "schedule an ingest", "import data from", "connect a data source", "connector", "pull data from postgres/mysql/an API/S3 buckets/Iceberg", or general Hotdata CLI usage. This skill bundles three specialized guides under subskills/, loaded on demand: read subskills/search/SKILL.md for full-text/vector search and retrieval indexes, subskills/analytics/SKILL.md for OLAP analytics, query history, stored results, and Chain materializations, and subskills/geospatial/SKILL.md for geospatial/GIS.
version: 0.27.1
---

# Hotdata CLI Skill

Use the `hotdata` CLI to interact with the Hotdata service. In this project, run it as:

```
hotdata <command> [args]
```

Or if installed on PATH: `hotdata <command> [args]`

## Sub-skills (loaded on demand)

This is the only top-level hotdata skill. Three specialized guides ship **bundled inside it** under `subskills/` and are not separate skills — **`Read` the matching file only when the task needs it** (progressive disclosure), then follow it:

| When the task involves | Read | Covers |
|------------------------|------|--------|
| BM25 / vector search, `hotdata search`, bm25/vector indexes, embedding providers | [`subskills/search/SKILL.md`](subskills/search/SKILL.md) | Search & retrieval indexes |
| OLAP SQL, aggregations, query/results history, Chain materializations, sorted indexes | [`subskills/analytics/SKILL.md`](subskills/analytics/SKILL.md) | Analytics |
| PostGIS-style `ST_*`, WKB geometry, spatial joins, GIS | [`subskills/geospatial/SKILL.md`](subskills/geospatial/SKILL.md) | Geospatial |

Everything else — auth, workspaces, databases, tables, basic `query`, context, jobs, ingest sources/ingest/run — is in this file. The three sub-skills are referred to below by name (**`hotdata-search`**, **`hotdata-analytics`**, **`hotdata-geospatial`**); each name means the bundled file above, loaded on demand.

## Authentication

Run **`hotdata auth login`** to authenticate via browser login. Config is stored in `~/.hotdata/config.yml`.

API key resolution (lowest to highest priority):
1. Config file (saved by `hotdata auth login`)
2. `HOTDATA_API_KEY` environment variable (or `.env` file)
3. `--api-key <key>` flag (works on any command)

API URL defaults to `https://api.hotdata.dev/v1` or overridden via `HOTDATA_API_URL`.

Optional: pass **`--debug`** on any command to print verbose HTTP request/response details.

## Workspace ID

Commands that accept `--workspace-id` default to the active workspace from config when omitted. Use `hotdata workspaces use` to switch interactively, or `hotdata workspaces use <workspace_id>` for a direct choice. In `hotdata workspaces list`, the `*` marker labels the **default** workspace the CLI resolves to.

**`hotdata databases queries` does not accept `--workspace-id`:** query run history always uses the active workspace—set it with `workspaces use` first if needed.

If **`HOTDATA_WORKSPACE`** is set in the environment, the workspace is **locked** to that value: passing a different `--workspace-id` is an error, and **`hotdata workspaces use` fails** (“workspace is locked”).

**Omit `--workspace-id` unless you need to target a specific workspace** (and it is not locked by env or session).

### Cold starts (worker wake-up)

A workspace's query worker scales to zero after inactivity. The **first** command against an idle workspace (e.g. `databases list`, `query`, `search`) blocks while it wakes — typically ~10s, up to ~20s — and the spinner upgrades to `waking up worker after inactivity (this can take ~20s)…`. **This is normal, not a hang:** don't kill the command, retry, or treat the pause as an error. Subsequent commands return promptly; warm workspaces are unaffected.

## Database context (API)

**`context:<STEM>`** (e.g. **context:DATAMODEL**, **context:GLOSSARY**) is an authoritative Markdown document stored server-side under that stem via the context API — *not* generic English ("a data model"), and *not* a local `./DATAMODEL.md` (local files are only `push`/`pull` transport). CLI commands take the bare stem: `hotdata databases context show DATAMODEL`. Context is scoped to the **active database** (`hotdata databases use <id>`); target another with `--database` / `-d`. Stems follow SQL identifier rules and accept a trailing `.md` (stored without it). Command reference: [Database context (named Markdown)](#database-context-named-markdown).

**Agents — list before show.** Run `hotdata databases context list` (optionally `--prefix DATAMODEL`) first; run `hotdata databases context show DATAMODEL` *only if* the stem is listed. A missing stem makes `show` exit 1 — normal for a fresh database, not a failure: don't retry in a loop or run speculative `show` in parallel with other tools. Proceed without context:DATAMODEL until one exists.

**context:DATAMODEL is the durable, shared store** — entities, keys, cross-catalog joins, and the naming/query conventions the whole team relies on. Keep task-scoped exploration (scratch SQL, hypotheses, one-off join checks) in the conversation or local notes; **promote** to context:DATAMODEL only when findings should outlive the session and guide everyone — reconcile against `databases context show DATAMODEL` (if listed), write `./DATAMODEL.md`, then `hotdata databases context push DATAMODEL`. No need to update it after every ad-hoc query. What to write inside the document: [references/DATA_MODEL.template.md](references/DATA_MODEL.template.md) and [references/MODEL_BUILD.md](references/MODEL_BUILD.md).

## Multi-step workflows

These are **patterns** built from the commands below—not separate CLI subcommands:

- **Model (`context:DATAMODEL`)** — The shared semantic map of the active database (entities, keys, joins across sources). Store and read it only via database context (`hotdata databases context list`, then `show DATAMODEL` **only when listed**, `push DATAMODEL`); refresh using `databases tables list` and `databases tables show`. For a deep pass (indexes, per-table detail), see [references/MODEL_BUILD.md](references/MODEL_BUILD.md).
- **History / Chain / OLAP SQL** — See **`hotdata-analytics`** and [references/WORKFLOWS.md](references/WORKFLOWS.md).
- **Search / retrieval indexes** — See **`hotdata-search`**.

Catalog, skill decision tree, epic flows (onboard, chain, retrieval), and instant databases: [references/WORKFLOWS.md](references/WORKFLOWS.md).

## Available Commands

Top-level subcommands (each detailed below): **`auth`**, **`query`**, **`workspaces`**, **`databases`**, **`jobs`**, **`ingest`**, **`search`**, **`manage`**. Instant databases nest `databases tables`, `databases queries`, `databases results`, and `databases context`; `ingest` nests `ingest sources`, runs, and logs; `manage` nests `usage`, `completions`, `upgrade`, and `skills`. Search (bm25/vector), indexes, and embedding providers are documented in **`hotdata-search`**; query history, results, Chain, and OLAP patterns in **`hotdata-analytics`**.

Global CLI options: **`--api-key`**, **`-v` / `--version`**, **`-h` / `--help`**, **`--no-input`** (disable interactive prompts; commands that require input will error instead — useful in CI or non-TTY environments). Hidden developer flag: **`--debug`** (verbose HTTP logs).

### List Workspaces
```
hotdata workspaces list [--output table|json|yaml]
```
Returns workspaces with `public_id`, `name`, `active`, `favorite`, `provision_status`. Table output marks the default workspace with `*`.

### Instant databases (`databases`)

**Instant databases** are Hotdata-owned catalogs you create and populate yourself — no remote source to sync. Query them in SQL as **`<database_id>.<schema>.<table>`**. Prefer **`hotdata databases`** for this workflow.

**Parquet only:** `databases tables load` accepts **parquet** files (local `--file`, remote `--url`, or a pre-staged `--upload-id`).

**Active database:** `hotdata databases use <id>` saves the active database to config. `databases tables list`/`load`/`remove`, `databases queries`/`results`, and all `databases context` commands default to the active database; pass **`--database <id>`** to override per-command. (`databases tables show` instead takes a fully-qualified `catalog.schema.table`.)

**Always select databases by id** (`dbid...`, from `databases list`). Display names and catalog aliases are not unique — several databases can share a name, and a fork answers to the same catalog as its source — so name-based selection is ambiguous.

```
hotdata databases list [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases count [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases create [--name <display_name>] [--catalog <alias>] [--table <table> ...] [--schema public] [--expires-at <duration|timestamp>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases fork [<id>] [--name <display_name>] [--expires-at <duration|timestamp>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases use <id>
hotdata databases unset
hotdata databases <id> [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases remove <id> [--workspace-id <workspace_id>]

# Attach a catalog so its tables are queryable (enables cross-catalog queries — see below)
hotdata databases attach <catalog|name> [--database <id>] [--alias <alias>]
hotdata databases detach <catalog|name|alias> [--database <id>]

# Preferred: load by catalog alias (auto-declares table if needed)
hotdata databases load --catalog <alias> --table <table> [--schema public] (--file <path> | --url <url> | --upload-id <id> | --result-id <id>) [--workspace-id <workspace_id>]

# Also available via tables subcommand
hotdata databases tables list [--database <id>] [--schema <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata databases tables load <table> [--database <id>] [--schema public] (--file <path> | --url <url> | --upload-id <id> | --result-id <id>) [--workspace-id <workspace_id>]
hotdata databases tables remove <table> [--database <id>] [--schema public] [--workspace-id <workspace_id>]
```

- `list` — all instant databases in the workspace. Active database is marked with `*` under the DEFAULT column; CREATED shows when each database was made.
- `count` — the total number of instant databases in the workspace, across **all** pages (`list` shows one page). Prints a bare integer by default so it drops straight into scripts (`$(hotdata databases count)`); `--output json|yaml` render `{"count": N}` / `count: N`.
- `create` — creates a new instant database. `--name` is an optional human-readable display name. `--catalog` sets the SQL alias used in queries (`SELECT … FROM <catalog>.schema.table`); must be `[a-z_][a-z0-9_]*`. `--expires-at` accepts relative durations (`24h`, `7d`, `90m`) or an RFC 3339 timestamp; omitting means no expiry. Repeat `--table` to declare tables up front.
- `fork` — creates a new instant database that is an independent deep copy of an existing one (same schemas, tables, and data); the source is left unchanged and the two diverge freely afterwards. The source defaults to the active database; pass the database `<id>` to fork another. `--name` defaults to `<source>-fork` (so the two stay distinguishable in `list`); `--expires-at` accepts a relative duration or RFC 3339 timestamp, and when omitted a still-future source expiry is carried over. The fork becomes the active database on success. The fork answers to the **same catalog alias** as its source inside its own scope; catalogs attached to the source are **re-attached** to the fork, but indexes are **not** carried over. Only databases created with the current (DuckLake) storage engine can be forked — older parquet-backed databases return an error.
- `use` — saves the database **id** as the active database. Subsequent `databases tables` and `databases context` commands use it automatically. Note that a successful `fork` also updates this: the fork becomes the active database.
- `unset` — clears the active database from config.
- `<id>` — inspect one database (returns id, catalog, name, expires_at).
- `remove` — removes the instant database; clears the active-database config if it matched.
- `load` (top-level shorthand) — loads parquet into `--catalog.--schema.--table`. Accepts `--file`, `--url`, `--upload-id`, or `--result-id` (load a saved query result by id — from `hotdata databases results` or a query's `[result-id: …]` footer — instead of a file; the result must belong to the target database). If the table was not declared at create time, the CLI automatically deletes and recreates the database with the table declared, then retries the load.
- `tables list` — lists tables with `TABLE` (`<catalog>.<schema>.<table>`), `SYNCED`, `LAST_SYNC`. Uses active database when `--database` is omitted.
- `tables load` — publishes to an instant-database table (with **replace** mode) from a local parquet file (`--file`), a remote parquet URL (`--url`), a pre-staged upload (`--upload-id`), or a saved query result (`--result-id`, must belong to the target database).
- `tables remove` — drops a table from the instant database.
- `attach` — attaches a **catalog** to an instant database, so the catalog's **live** tables become visible inside that database's query scope. Defaults to the active database; target another with `--database`. `--alias` sets the SQL name the catalog answers to (defaults to the catalog's name). This is how you query an attached catalog's tables and **join across catalogs** — see [Querying across catalogs](#querying-across-catalogs-attach).
- `detach` — removes an attached catalog. Accepts the catalog name/id **or** the alias you attached it under. Defaults to the active database.
- `create --attach <catalog>[=<alias>]` — attach one or more catalogs at creation time (repeatable), e.g. `--attach github --attach salesdb=sales`.

Example:

```
hotdata databases create --catalog airbnb
hotdata databases load --catalog airbnb --table listings --url https://example.com/listings.parquet
hotdata query "SELECT count(*) FROM airbnb.public.listings"
```

#### Querying across catalogs (attach)

**A `hotdata query` runs inside exactly one instant database** — the active database (`hotdata databases use <id>`) or the one named by `--database`. With none set, the query fails with *"a database is required."* That database's query scope sees **only its own catalog plus any catalogs explicitly attached to it** — a workspace catalog is **not** visible just because it exists. Referencing an unattached catalog fails with *"table '\<catalog\>.\<schema\>.\<table\>' not found."*

To query an attached catalog's tables, or **join an instant database's table against an attached catalog's table in one query**, attach the catalog to the database first. The catalog's data stays **live** (synced) — this is not a copy:

```
# Attach the 'github' catalog (live) to the active database under alias 'gh'
hotdata databases attach github --alias gh

# Now both the database's own tables and the attached catalog are in scope:
hotdata query "SELECT * FROM gh.github.issues WHERE state = 'OPEN' LIMIT 10"

# Cross-catalog join: an instant database's table JOINed against the live attached-catalog table
hotdata query "
  SELECT t.id, i.title
  FROM mycatalog.public.tickets t
  JOIN gh.github.issues i ON i.number = t.gh_issue
"

hotdata databases detach gh   # when finished (optional)
```

Without `--alias`, the catalog answers to its own name (`github.github.issues`). Do **not** export a catalog to parquet just to query it — attach is the live, sync-preserving path.

### Tables

```
hotdata databases tables list [--workspace-id <workspace_id>] [--schema <pattern>] [--table <pattern>] [--limit <int>] [--cursor <cursor>] [--output table|json|yaml]
hotdata databases tables show <catalog.schema.table|schema.table> [--output table|json|yaml]
```

**`databases tables list`**
- **Always use this command to discover available tables.** Do NOT query `information_schema` via `hotdata query`.
- With an **active database set** (`hotdata databases use <id>`): lists tables in that database — format `<catalog>.<schema>.<table>`, columns `TABLE`, `SYNCED`, `LAST_SYNC`.
- With **no active database**: lists all tables across the workspace — format `<source>.<schema>.<table>`, same columns.
- `--schema` and `--table` support SQL `%` wildcard patterns (e.g. `--table order%`).
- Results are paginated (default 100 per page); a `--cursor` token is printed when more are available.

**`databases tables show`**
- Fetches column definitions (`COLUMN`, `DATA_TYPE`, `NULLABLE`) for a single table.
- **`catalog.schema.table`** — three-part form; the catalog resolves to an instant database or an attached source by name.
- **`schema.table`** — two-part form; uses the active database (errors if none is set).
- Copy the name directly from `databases tables list` output — both forms match what `list` prints.
- **Always use `databases tables show` to inspect columns before writing queries.**

### Database context (named Markdown)

Reads and writes **database-scoped context API** documents. Context is tied to the **active database** (set via `hotdata databases use`); pass **`--database <id>`** (short: **`-d`**) to target a specific database. **`show`** needs no local file; **`push`** / **`pull`** use **`./<NAME>.md`** in the current directory only as the CLI transport format. See [Database context (API)](#database-context-api).

```
hotdata databases context list [--database <id>] [--prefix <stem>] [--output table|json|yaml]
hotdata databases context show <name> [--database <id>]
hotdata databases context pull <name> [--database <id>] [--force] [--dry-run]
hotdata databases context push <name> [--database <id>] [--dry-run]
```

- `list` — names, `updated_at`, and character counts for each stored context in the active database. Use `--prefix` to narrow names (case-sensitive). **Agents:** call **`list` before `show`** for `DATAMODEL` (or any stem) so you do not rely on `show` failing when the document does not exist yet.
- `show` — print the Markdown body to **stdout** (use this when there is **no** local `./<NAME>.md`; ideal for agents). **Errors** if no context with that `name` exists (exit 1)—expected for a new database; use `list` first to avoid that path.
- `pull` — download context `name` to `./<NAME>.md`. Refuses to overwrite an existing file unless `--force`. `--dry-run` prints target path and size only.
- `push` — upload `./<NAME>.md` to upsert context `name` on the server. `--dry-run` prints size only. Body size must stay within the API limit (order of 512k characters).

**Convention:** **context:DATAMODEL** is the primary database semantic map; **context:GLOSSARY** (or other **`context:<STEM>`** docs) for additional narrative context. Same identifier rules as SQL table names. CLI: `hotdata databases context show DATAMODEL` (bare stem).

### Execute SQL Query

```
hotdata query "<sql>" [--workspace-id <workspace_id>] [--database <database>] [--output table|json|csv]
hotdata query status <query_run_id>
```

- Default output is `table` (row count and execution time).
- **A query runs inside one instant database** (active database or `--database`); with none set it fails *"a database is required."* The scope sees the database's own catalog **plus any attached catalogs only**. To query an attached catalog's tables or join across catalogs, attach the catalog first — see [Querying across catalogs (attach)](#querying-across-catalogs-attach).
- Use `hotdata databases tables list` and `hotdata databases tables show` for discovery — not `information_schema` via `query`. (Discovery lists every workspace table; queryability still requires the table's catalog to be in the active database's scope.)
- **PostgreSQL dialect.** Quote non-lowercase columns with double quotes.
- Async runs return `query_run_id` → poll with `query status <id>` (do not re-run the same heavy SQL). `query status` exit codes: `0` succeeded, `1` failed, `2` still running (poll again), `3` succeeded but the result is a truncated/incomplete preview.
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
- `--job-type`: `data_refresh_table`, `data_refresh_connection`, `create_index`, `managed_load`.
- `--status`: `pending`, `running`, `succeeded`, `partially_succeeded`, `failed`.
- Use `hotdata jobs <job_id>` to inspect a specific job's status, error, and result.

### Ingest external data (`ingest sources`, `ingest`)

Pull data from external sources (SQL databases, APIs, S3/GCS/Azure buckets, Iceberg catalogs, Kafka) into instant databases. **Three nouns, three ids, and an id is always what goes on the wire** — the service has no name lookup, because a display name is a label and nothing stops two rows sharing one:

- **source** (`ds_…`) — what a credential opens: a server, a bucket root, a catalog, a cluster. Holds config + credentials, loads no data. Managed under `ingest sources`.
- **ingest** (`ing_…`) — a saved load definition: `source + selector + destination + type/schedule`. One source can back many ingests.
- **run** (`run_…`) — one execution attempt, with snapshots of the config version, selector, and destination it used. Addressed under its ingest: `ingest logs <ing_…>` lists them, `ingest run <run_…>` shows one. There is no top-level `run` command — that word belongs to `jobs` and `databases queries`.

One flag softens that for typing, and only for typing: `ingest create --source` accepts a display name and resolves it to an id **client-side, before the request**, erroring with both ids if the name matches two sources rather than picking one. Every other argument, and every request the CLI sends, takes ids only.

Read commands (`ingest sources list|show|types|fields`, `ingest list|show|logs|run`) work with a login session JWT. Commands that persist a credential (`ingest sources add`, `ingest sources update-config`, `ingest create`) **require a workspace API key** (`HOTDATA_API_KEY` / `--api-key`, `hd_...`) — the run outlives the 5-minute JWT.

```bash
hotdata ingest sources types [filter]  # browse available source types. The FAMILY
                                       # column is what --family takes.

hotdata ingest sources fields [family] # THE FIELD REFERENCE. With a family: the
                                       # fields --config, --credentials and
                                       # --selector take, with types, which are
                                       # required, and what the family supports
                                       # (write modes, continuous, row filter).
                                       # With none: every family, one row each.
                                       # -o json returns the JSON Schema itself.
# The service generates it from the models that validate the request, so it
# cannot name a field the API rejects. Read it before writing source.json or a
# selector — do not guess field names, and do not carry your own list.

# --- sources -----------------------------------------------------------------
hotdata ingest sources test --family sql --config @source.json
# Persists NOTHING: checks the credentials and returns family-specific discovery
# (schemas/tables/…). Run it before add.

hotdata ingest sources add
# WITH NO --config ON A TERMINAL this asks: which source type, then that family's
# own fields — labels, accepted values and which answers are hidden all come from
# the service's field reference. --no-input, CI and a piped stdin skip it entirely
# and require --config, so agent/script invocations are unaffected. As an agent
# you are non-interactive: use the flag form below.

hotdata ingest sources add --family sql --config @source.json --display-name "prod postgres"
# source.json is family-specific and carries both halves:
#   {"config": {"dialect": "postgres", "host": …, "database": …},
#    "credentials": {"username": …, "password": …}}
# --config also accepts a bare config object, @- (stdin), or inline JSON.
# --credentials takes the secret half separately. Keep secrets out of argv.
# Families: sql, filesystem (buckets), kafka, iceberg, delta, ducklake, rest.
# The fields each half takes: hotdata ingest sources fields <family>.
# Two config fields have a flag of their own, which BUILDS that JSON:
hotdata ingest sources add --family filesystem --bucket-url s3://events-prod
#   --bucket-url <uri>     config.root_uri (+ provider, read off the scheme)
#   --catalog-type <t>     config.catalog_type (iceberg)
# They merge with --config, flag last. --no-wait returns without watching the
# new source settle; the wait is a poll and starts nothing.

hotdata ingest sources list [--family sql] [--state active]   # ids, families, states
hotdata ingest sources show <source-id>                       # state, config version, discovery
hotdata ingest sources update-config <source-id> --config @source.json
# Appends an immutable config version under the SAME id and moves the pointer.
# This is also how source credentials are rotated. Credential semantics:
#   (neither flag)   inherit the previous secret refs
#   --credentials …  replace them
#   --no-credentials drop them (public/no-auth sources)
hotdata ingest sources remove <source-id>   # 409 while any ingest references it

# --- ingests -----------------------------------------------------------------
hotdata ingest create --datasource-id ds_01J --type one-time \
  --selector @selector.json --destination @destination.json
# selector.json is family-specific (what subset to read) — its fields, and the
#   write modes this family accepts: hotdata ingest sources fields <family>.
# destination.json is {"database_id", "schema", "table", "write_mode"} —
#   write_mode: replace | upsert (upsert needs a continuous bucket ingest).
#   Selector and destination are both IMMUTABLE after creation.
# CREATE STARTS NOTHING, for every type. It returns no run id, and
# `ingest logs <id>` is EMPTY until the scheduler claims the ingest — normal,
# not a failure, and not something to retry. A one-time ingest is created DUE,
# so it is claimed on the next tick and exactly once. Poll `ingest logs` until
# a run appears, then `ingest run <run-id> --wait`.

hotdata ingest create --datasource-id ds_01J --type continuous \
  --selector @selector.json --destination @destination.json --every 5m [--next now]
# scheduled/continuous need --every (30s, 5m, 2h, 1d) or --schedule @schedule.json.

# SHORTHAND FLAGS build that same selector JSON, client-side. --selector stays
# the escape hatch; the two never produce different requests.
hotdata ingest create --source "prod postgres" --table orders --schema public \
  --database-id db_123
#   --source <name-or-id>  the datasource, BY DISPLAY NAME or ds_… id. A name is
#                          resolved here to an id; two matches is an error listing
#                          both, never a guess. --datasource-id takes ids only.
#   --table <name>         source table, REPEATABLE (sql, iceberg, ducklake)
#   --schema <name>        source schema (sql)
#   --format csv|jsonl|parquet, --glob "**/*.parquet"   (bucket sources)
#   --record-shape otel_traces|mqtt_observations        (bucket sources)
#   --all                  everything under a bucket root (needs --format)
#   --limit N              stop after N source rows
# Destination flags instead of --destination:
#   --database-id (required)  --dest-table (defaults to the single --table)
#   --dest-schema (default public)  --write-mode (default replace)

hotdata ingest create --datasource-id ds_01J --database-id db_123 \
  --sql "SELECT id, status FROM public.orders WHERE status = 'open' LIMIT 1000"
# Restricted SQL grammar, parsed CLIENT-side into the same structured selector +
# destination. The FROM table also names the destination table.

hotdata ingest create --datasource-id ds_01J --database-id db_123 \
  --raw-sql "SELECT customer_id, sum(amount) FROM orders GROUP BY 1" \
  --table order_totals [--limit 1000]
# The source engine's OWN dialect, run verbatim at the source: joins, aggregates,
# CTEs, window functions. Only the result set transfers, into --table. (A query
# has no source table, so --table names where the result lands.)

hotdata ingest list [--datasource-id ds_01J] [--type continuous] [--state active]
hotdata ingest show <ingest-id>
hotdata ingest pause <ingest-id>     # stops the active run AND future runs
hotdata ingest resume <ingest-id>    # clears the stop; starts NOTHING immediately
hotdata ingest schedule <ingest-id> --every 5m [--next now]
hotdata ingest remove <ingest-id>    # releases the destination table; data untouched

# --- runs --------------------------------------------------------------------
hotdata ingest logs <ingest-id> [--status failed]   # every attempt, newest first
hotdata ingest run <run-id>          # exits 0 succeeded / 1 failed|cancelled / 2 in flight
# --wait on either polls to a terminal status (--wait-timeout, default 300s;
# exit 2 on timeout). It WATCHES: the scheduler owns dispatch, so waiting cannot
# make a queued run start. `ingest schedule <id> --next now` is what does that.
```

Agent tips:
- **There is no `trigger-import` / run-now verb, by design.** Nothing you can call starts a run — the scheduler dispatches every one. A one-time ingest is created *due*, so it is claimed on the next tick; scheduled/continuous ones are claimed on their schedule, and each run recovers from the last committed state. To make the next scheduled run happen now: `hotdata ingest schedule <ingest-id> --next now`. To load again from scratch: create another one-time ingest.
- **An empty run list right after `ingest create` is normal.** The scheduler has not claimed the ingest yet. Poll `ingest logs <ingest-id>` until a run appears rather than treating the gap as a failed create — re-creating the ingest here is how you end up with two loads into one table.
- **`pause` means both halves** — stop the current run *and* stop future dispatch. `resume` is its inverse and is deliberately not a trigger.
- **Selector and destination are immutable.** Changing what an ingest reads or where it lands means a new ingest; the server rejects edits with `immutable_ingest_definition`.
- `--sql` is a **restricted grammar**: `SELECT <cols|*> FROM [<schema>.]<table> [WHERE …] [LIMIT n]` — no joins/GROUP BY/ORDER BY, and the FROM target names the **source table**, not a datasource. For anything richer use `--raw-sql`, which runs the statement verbatim at the source in its own dialect.
- **Every shorthand flag builds the same JSON `--selector`/`--destination` carry.** Nothing new reaches the API through them, so mixing a shorthand with the JSON for the same half is rejected rather than merged.
- Run `status` is a **closed set**: `queued` | `running` | `succeeded` | `failed` | `cancelled`. While running, the finer progress state (e.g. `extracting`, `loading`) appears in `stage` — informational only, never switch on it.
- Prefer `-o json` plus the `ingest run` exit codes for scripting; poll `ingest run` rather than holding a terminal open.
- Tables print oldest→newest; `-o json` is newest-first (`[0]` = latest).
- Errors carry a stable code alongside the message, e.g. `HTTP 409: … (destination_table_conflict)`. Branch on the code, not the sentence.
- Once a run has succeeded, the destination is a regular instant DB: query it with `hotdata query --database <db-id> "SELECT … FROM public.<table>"`.

### Usage
```
hotdata manage usage [--since <rfc3339>] [--workspace-id <workspace_id>] [--output table|json|yaml]
```
Workspace usage for the current billing window (or since `--since`): `query_count`, `bytes_scanned`, `storage_bytes`, and `storage_captured_at`.
- `query_count` and `bytes_scanned` accrue **per query in real time** (data reads).
- `storage_bytes` is a **periodic snapshot** taken at `storage_captured_at`, so it reflects uploads only after the next capture — not instantly.
- Table output renders byte counts human-readably (raw integers in `-o json`/`yaml`).

### Agent skills (`skills`)

A single top-level **`hotdata`** skill ships with the CLI release tarball; the specialized guides (`search`, `analytics`, `geospatial`) are bundled **inside it** under `subskills/` and load on demand, so only `hotdata` registers as an agent skill.

```
hotdata manage skills install [--project]
hotdata manage skills status
hotdata manage skills list
```

- **`install`** — Downloads and installs the skill to **`~/.hotdata/skills/hotdata`**, then symlinks it into **`~/.agents/skills`** and into **`~/.claude/skills`** / **`~/.pi/skills`** when those directories exist (the bundled sub-skills ride along inside the `hotdata` directory). **`--project`** instead copies into **`./.agents/skills/hotdata`** in the current directory (and links `./.claude` / `./.pi` when present). The CLI may auto-refresh skills after an upgrade when appropriate.
- **`status`** — Reports installed vs current CLI version and where skills are linked.
- **`list`** — Alias for `status`: lists installed skills, their versions, and where they are linked.

### Shell completions

```
hotdata manage completions <bash|zsh|fish>
```

Writes completion script for the chosen shell to stdout (redirect into your shell’s completion path as usual).

### Upgrade (`upgrade`)

```
hotdata manage upgrade
```

Upgrades the CLI in place to the latest release (`brew upgrade` for Homebrew installs, otherwise a direct binary download), refreshing bundled skills to match. After a successful upgrade, re-run your command.

A newer release can be incompatible with the API, so in an **interactive terminal** the CLI checks for a new release before running any API-touching command and prompts to upgrade. Declining (or `Ctrl-D`) exits without running the command — `hotdata manage upgrade` is then required to continue. The check is a **no-op in non-interactive sessions** (no TTY, `--no-input`, or `HOTDATA_NO_UPDATE_CHECK` set), so typical agent and CI usage is never blocked; set `HOTDATA_NO_UPDATE_CHECK=1` to disable it entirely.

### Auth
```
hotdata auth login            # Browser-based login
hotdata auth register         # Create a new account via browser (GitHub OAuth by default)
hotdata auth register --email # Create a new account via browser, using email + password instead of GitHub
hotdata auth status           # Check current auth status
hotdata auth logout           # Remove saved auth for the default profile
```

`login` and `register` (both GitHub and `--email`) are **browser-based** PKCE flows: the CLI opens a browser and waits on a local callback to complete sign-in/sign-up — account details (email/password) are entered in the browser, not via CLI flags. They require a browser and an interactive terminal, so they do **not** work under `--no-input` or in headless/CI. For automation, authenticate once interactively, then use the saved session or `HOTDATA_API_KEY`.

## Workflows

End-to-end recipes — onboard a workspace, run a query, build an instant database (parquet), chain/materialize, add retrieval indexes — live in [references/WORKFLOWS.md](references/WORKFLOWS.md). The command sections above are the per-command reference; the workflows stitch them into sequences.
