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

The core loop: create a **managed database**, put data in it, query it with
PostgreSQL-dialect SQL. Everything else builds on that.

## Getting your data in

**Upload a parquet file** directly (convert CSV/JSON first):

```sh
hotdata databases load --catalog demo --table listings --file ./listings.parquet
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
control a scheduled ingest. Data lands in a managed database — query it like any
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

Attach another catalog to a managed database and join its live tables directly,
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

Run `hotdata <command> --help` for full flags on any command.

| Command | What it does |
| :-- | :-- |
| `auth` | `login`, `register`, `status`, `logout` |
| `workspaces` | List workspaces, set the active one |
| `databases` | Managed databases: create, load, fork, attach — plus `tables`, `query`, `queries`, `results`, `context` inside them |
| `query` | Run SQL (`--dialect` transpiles DuckDB/Postgres/Snowflake); `status` polls async runs |
| `search` | BM25 and vector search; `create`/`list`/`remove` indexes; `embeddings` |
| `ingest` | External sources (`sources`) and saved load definitions |
| `jobs` | Background jobs (refreshes, index builds) |
| `manage` | `skills`, `completions`, `upgrade`, `usage` |

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
