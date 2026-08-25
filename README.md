<p align="center">
  <img src="https://avatars.githubusercontent.com/u/226170140" alt="Hotdata" width="120">
  <br>
  <strong>Hotdata CLI</strong>
  <br>
  Command line interface for <a href="https://www.hotdata.dev">Hotdata</a>.
  <br><br>
  <a href="https://github.com/hotdata-dev/hotdata-cli/releases"><img src="https://img.shields.io/github/v/release/hotdata-dev/hotdata-cli" alt="release"></a>
  <a href="https://github.com/hotdata-dev/hotdata-cli/actions/workflows/ci.yml"><img src="https://github.com/hotdata-dev/hotdata-cli/actions/workflows/ci.yml/badge.svg" alt="build"></a>
  <a href="https://codecov.io/gh/hotdata-dev/hotdata-cli"><img src="https://codecov.io/gh/hotdata-dev/hotdata-cli/branch/main/graph/badge.svg" alt="coverage"></a>
</p>

---

Query, search, and join your data from one place — external databases, APIs,
cloud storage, Iceberg catalogs, and files you upload — with plain SQL and a few
commands.

## Install

```sh
brew install hotdata-dev/tap/cli        # Homebrew
cargo install --path .                  # from source (requires Rust)
```

Or grab a binary from [Releases](https://github.com/hotdata-dev/hotdata-cli/releases).
Stay current with `hotdata manage upgrade`; enable tab completion with
`hotdata manage completions bash|zsh|fish`.

## Quickstart

```sh
hotdata auth login                            # or: hotdata auth register
hotdata databases create --catalog demo
hotdata databases load --catalog demo --table trips \
  --url https://d37ci6vzurychx.cloudfront.net/trip-data/yellow_tripdata_2024-01.parquet
hotdata query "SELECT count(*) FROM demo.public.trips"
```

The core loop: create an **instant database**, put data in it, query it with
PostgreSQL-dialect SQL. Everything else builds on that.

## Getting your data in

**Upload a parquet file** directly (convert CSV/JSON first):

```sh
hotdata databases load --catalog demo --table listings --file ./listings.parquet
```

A load **replaces** the table by default. Add `--append` to add rows to an
existing table instead:

```sh
hotdata databases load --catalog demo --table listings --file ./more-listings.parquet --append
```

**Import from an external source** — Postgres/MySQL, S3/GCS buckets, Iceberg,
Kafka, ~150 API services — via a datasource (`ds_…`) and a saved ingest (`ing_…`):

```sh
hotdata ingest sources types                  # browse source types and families
hotdata ingest sources fields sql             # config/credentials/selector a family takes
hotdata ingest sources add                     # create a datasource (prompts, or --config @src.json)

hotdata ingest create --source "prod postgres" --table orders --database-id db_123
hotdata ingest logs <ing_…>                    # attempts for an ingest, newest first
hotdata ingest run <run_…> --wait              # show one attempt; exits 0 done / 1 failed / 2 in flight
```

`ingest sources update-config` rotates credentials; `ingest pause|resume|schedule`
control a scheduled ingest. Data lands in an instant database — query it like any
other.

## Query and explore

```sh
hotdata databases tables list                  # every queryable table
hotdata databases tables show <table>          # columns and types
hotdata query "<sql>" [-o table|json|csv]      # HotSQL (PostgreSQL dialect)
```

Write SQL in another dialect and the server transpiles it to HotSQL —
`--dialect` accepts `hotsql` (default), `duckdb`, `postgres`, `snowflake`
(read-only for a non-default dialect):

```sh
hotdata query "SELECT IFF(n > 0, 'pos', 'neg') FROM t" --dialect snowflake
```

Long queries go async and print a `query_run_id` — poll with
`hotdata query status <id>` (exit `0` done / `1` failed / `2` running). Re-fetch
past results with `hotdata databases results get <result-id>`; browse history
with `hotdata databases queries list`.

## Join across sources

Attach another catalog to an instant database and join its live tables directly,
no copying:

```sh
hotdata databases attach prod-replica --alias prod
hotdata query "SELECT t.id, o.total FROM demo.public.tickets t
               JOIN prod.public.orders o ON o.ticket_id = t.id"
```

## Search

Create an index once, then search server-side. Vector search auto-embeds the
column and the query — no embedding keys or client setup:

```sh
hotdata search create trips_notes --type text --from demo.public.trips --column notes
hotdata search "airport surcharge dispute" --index trips_notes
```

Use `--type vector` for semantic search. Indexes resolve in the active database
(`hotdata databases use <id>`); pass `-d/--database <id>` to target another.
Bring your own model with `hotdata search embeddings add`.

## Use it from scripts and agents

- Every listing command takes `-o json|yaml`; `query status` and `ingest run`
  expose script-friendly exit codes.
- Authenticate non-interactively with `--api-key`, or `HOTDATA_API_KEY` in the
  environment or a `.env` file.
- `hotdata manage skills install` installs bundled agent skills — Markdown
  playbooks that teach AI coding agents (Claude Code and friends) the full CLI.
- `hotdata databases context push|show DATAMODEL` stores your data model as
  shared, server-side Markdown so humans and agents query with the same map.

## Commands

The full command surface. The top level has eight groups — `auth`, `workspaces`, `databases`, `query`, `jobs`, `ingest`, `search`, and `manage`. Run `hotdata <command> --help` for full flags on any command.

| Command | What it does |
| :-- | :-- |
| `auth login` | Log in via browser |
| `auth register` | Create a new account via browser (GitHub OAuth; `--email` for email + password) |
| `auth logout` | Remove authentication for a profile |
| `auth status` | Show authentication status |
| `workspaces list` | List all workspaces |
| `workspaces use` | Set the default workspace |
| `databases list` | List instant databases in the workspace |
| `databases count` | Count instant databases in the workspace |
| `databases show` | Show details for an instant database |
| `databases create` | Create a new instant database |
| `databases fork` | Fork a database into a new, independent database |
| `databases attach` | Attach a catalog so its tables are queryable |
| `databases detach` | Detach a previously attached catalog |
| `databases use` | Set the current (default) database |
| `databases unset` | Clear the current database |
| `databases remove` | Delete a database and all its tables |
| `databases load` | Load a parquet file or saved result into a table (replace, or `--append`) |
| `databases tables list` | List tables in a database |
| `databases tables show` | Show column definitions for a table |
| `databases tables load` | Load parquet/result into a table (replace, or `--append`) |
| `databases tables remove` | Delete a table from a database |
| `databases context list` | List named contexts in a database |
| `databases context show` | Print context content to stdout |
| `databases context pull` | Download context to `./<NAME>.md` |
| `databases context push` | Upload `./<NAME>.md` as named context |
| `databases query` | Execute a SQL query against a database |
| `databases query status` | Check a running query and retrieve results |
| `databases queries list` | List query runs |
| `databases results get` | Show a stored query result by ID |
| `databases results list` | List stored query results |
| `query "<sql>"` | Execute a SQL query (shortcut for `databases query`) |
| `query status` | Check a running query and retrieve results |
| `jobs list` | List background jobs (active by default) |
| `jobs <id>` | Show one background job |
| `ingest create` | Create a load definition |
| `ingest list` | List the ingests in the workspace |
| `ingest show` | Show one ingest: state, selector, destination, schedule |
| `ingest pause` | Stop an ingest (cancel the active run and future runs) |
| `ingest resume` | Clear a stop and let the schedule dispatch again |
| `ingest schedule` | Change when a scheduled/continuous ingest runs next |
| `ingest logs` | List the runs of one ingest |
| `ingest run` | Show one run: status, snapshots, timings |
| `ingest remove` | Delete an ingest and release its destination table |
| `ingest sources test` | Check a config and credentials without creating anything |
| `ingest sources add` | Create a datasource and its first config version |
| `ingest sources list` | List the datasources in the workspace |
| `ingest sources show` | Show one datasource: state, config, discovery |
| `ingest sources update-config` | Append a config version (rotate credentials) |
| `ingest sources remove` | Delete a datasource |
| `ingest sources types` | Browse the catalog of source types |
| `ingest sources fields` | Show the fields a source family accepts |
| `search "<text>" --index <name>` | Run a full-text or vector search against an index |
| `search create` | Create a search index over a table column |
| `search list` | List search indexes |
| `search show` | Show one search index by name |
| `search remove` | Remove a search index by name |
| `search embeddings list` | List embedding providers |
| `search embeddings show` | Show one embedding provider |
| `search embeddings add` | Create a new embedding provider |
| `search embeddings update` | Update an embedding provider |
| `search embeddings remove` | Delete an embedding provider |
| `manage usage` | Show workspace usage: queries, bytes scanned, stored bytes |
| `manage completions` | Generate shell completions (`bash`, `zsh`, `fish`) |
| `manage upgrade` | Upgrade the CLI to the latest release |
| `manage skills install` | Install/update the agent skill into agent directories |
| `manage skills status` | Show the agent skill's installation status |
| `manage skills list` | List installed skills (alias for `status`) |

## Configuration

Config lives at `~/.hotdata/config.yml` (profile-keyed). Environment variables:

| Variable | Description | Default |
| :-- | :-- | :-- |
| `HOTDATA_API_KEY` | API key (overrides config file; also read from `.env`) | |
| `HOTDATA_WORKSPACE` | Lock every command to one workspace | |
| `HOTDATA_API_URL` | API base URL | `https://api.hotdata.dev/v1` |
| `HOTDATA_APP_URL` | App URL for browser login | `https://app.hotdata.dev` |

API-key precedence, lowest to highest: config file → `HOTDATA_API_KEY` → `--api-key`.

## Development

```sh
cargo build && cargo test
```

Release process: see [docs/RELEASING.md](docs/RELEASING.md).
