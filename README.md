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

Query, search, and join your data from one place — live databases, APIs, cloud
storage, Iceberg catalogs, and files you upload — with plain SQL and a few
commands.

## Install

**Homebrew**

```sh
brew install hotdata-dev/tap/cli
```

**Binary (macOS, Linux)** — download from [Releases](https://github.com/hotdata-dev/hotdata-cli/releases).

**From source** (requires Rust)

```sh
cargo install --path .
```

Stay current with `hotdata upgrade`, and enable tab completion with
`hotdata completions bash|zsh|fish`.

## Quickstart

```sh
# 1. Sign in (or create an account with `hotdata auth register`)
hotdata auth login

# 2. Create a database and load some data — any parquet file, local or by URL
hotdata databases create --catalog demo
hotdata databases load --catalog demo --table trips \
  --url https://d37ci6vzurychx.cloudfront.net/trip-data/yellow_tripdata_2024-01.parquet

# 3. Query it
hotdata query "SELECT count(*) AS trips, round(avg(fare_amount), 2) AS avg_fare
               FROM demo.public.trips"
```

That's the core loop: create a **managed database**, put data in it, query it
with PostgreSQL-dialect SQL. Everything else builds on that.

## Getting your data in

There are two ways to make your data queryable, and they compose:

- **Connect** — a live, synced view of an external source. The data stays
  where it is; Hotdata keeps the schema and cache fresh.
- **Import** — copy data *into* a managed database, either from a file you
  have or from an external source via `hotdata ingest`.

### Connect a live database

```sh
hotdata connections new                     # guided wizard
# or non-interactively:
hotdata connections create --name "prod-replica" --type postgres \
  --config '{"host":"db.example.com","port":5432,"user":"reader","database":"app","password":"'"$DB_PASSWORD"'"}'

hotdata tables list                         # see what's now queryable
```

`hotdata connections create list` shows every supported type, and
`hotdata connections create list <type> -o json` prints the exact config
fields it takes.

### Import from external sources (`ingest`)

Pull data from SQL databases, APIs, S3/GCS/Azure buckets, or Iceberg catalogs
into a managed database:

```sh
hotdata ingest datasources                  # browse: SQL dialects, ~150 API services,
                                            # buckets, iceberg, api (bring-your-own)

# Add a datasource — validates credentials and discovers the schema, loads no data.
# Keep secrets out of argv with --config @file.json or @- (stdin):
hotdata ingest new-datasource --service postgres --config @conn.json --schema public
hotdata ingest new-datasource --service buckets --bucket-url s3://bucket/prefix --format parquet
hotdata ingest new-datasource --service iceberg --config @catalog.json --table ns.orders

# --name sets the FROM target (default: the connector name), so several
# datasources of one connector can coexist:
hotdata ingest new-datasource --service postgres --name prod_pg --config @conn.json

# Import — a plain SELECT against the datasource; returns immediately:
hotdata ingest new-import "SELECT * FROM prod_pg.orders WHERE status = 'open'"
hotdata ingest new-import --source prod_pg --all
hotdata ingest status <import-id> --wait    # or one-shot: exits 0 done / 1 failed / 2 running

# The import lands in a managed database — query it like any other:
hotdata query --database <db-id> "SELECT count(*) FROM public.orders"
```

Public buckets need no credentials. Re-run an import any time with
`hotdata ingest trigger-import <import-id>` — it refreshes the same database
from the source.

### Upload files

Managed databases load **parquet** directly (convert CSV/JSON first):

```sh
hotdata databases load --catalog demo --table listings --file ./listings.parquet
```

## Query and explore

```sh
hotdata tables list                          # every queryable table, as connection.schema.table
hotdata tables list --connection-id <id>     # with column names and types
hotdata query "<sql>" [-o table|json|csv]    # PostgreSQL dialect
```

Long-running queries fall back to async and print a `query_run_id` — poll with
`hotdata query status <id>` (exit codes: `0` done, `1` failed, `2` running).
Every result gets a `result-id`; re-fetch past results with
`hotdata results <result-id>` instead of re-running, and browse history with
`hotdata queries list`.

## Join across sources

A query runs inside one managed database and sees its own tables plus any
**attached** connections — attach a live source to join against it directly,
no copying:

```sh
hotdata databases attach prod-replica --alias prod

hotdata query "
  SELECT t.id, o.total
  FROM demo.public.tickets t
  JOIN prod.public.orders o ON o.ticket_id = t.id
"
```

## Search

Create an index once, then search server-side — no embedding keys or client
setup needed for vector search (the column is auto-embedded, and queries use
the same model automatically):

```sh
hotdata indexes create --catalog demo --schema public --table trips \
  --column notes --type bm25
hotdata search "airport surcharge dispute" --table demo.public.trips

hotdata indexes create --catalog demo --schema public --table trips \
  --column notes --type vector
hotdata search "rides that mention lost items" --table demo.public.trips --type vector
```

System embedding providers come pre-configured; bring your own with
`hotdata embedding-providers create`.

## Use it from scripts and agents

The CLI is built to be driven programmatically:

- Every listing command takes `-o json|yaml`; long-running commands expose
  script-friendly exit codes (`query status`, `ingest status`).
- Authenticate non-interactively with an API key: `--api-key`, or
  `HOTDATA_API_KEY` in the environment or a `.env` file.
- `hotdata databases run <cmd>` launches a child process (an agent, a script)
  with credentials scoped to a single database.
- `hotdata skills install` installs bundled agent skills — Markdown playbooks
  that teach AI coding agents (Claude Code and friends) the full CLI surface.
- `hotdata context push|show DATAMODEL` stores your data model as shared,
  server-side Markdown so humans and agents query with the same map.

## Commands

Run `hotdata <command> --help` for full flags on any command.

| Command | What it does |
| :-- | :-- |
| `auth` | `login`, `register`, `status`, `logout` |
| `workspaces` | List workspaces, set the active one |
| `connections` | Live external sources: create, list, refresh |
| `databases` | Managed databases: create, load parquet, attach connections, scoped `run` |
| `tables` | List queryable tables and columns |
| `query` | Run SQL; `status` polls async runs |
| `queries` / `results` | Query history and stored results |
| `search` | BM25 and vector search over indexed columns |
| `indexes` | Create/list/delete `sorted`, `bm25`, `vector` indexes |
| `embedding-providers` | Manage embedding providers for vector indexes |
| `ingest` | Import from databases, APIs, buckets, Iceberg |
| `context` | Shared server-side Markdown (`DATAMODEL`, glossaries) |
| `jobs` | Background jobs (refreshes, index builds) |
| `skills` | Install/inspect the bundled agent skills |
| `completions` | Shell tab-completion scripts |
| `upgrade` | Upgrade the CLI in place |

## Configuration

Config lives at `~/.hotdata/config.yml` (profile-keyed). Environment variables:

| Variable | Description | Default |
| :-- | :-- | :-- |
| `HOTDATA_API_KEY` | API key (overrides config file; also read from `.env`) | |
| `HOTDATA_WORKSPACE` | Lock every command to one workspace | |
| `HOTDATA_API_URL` | API base URL | `https://api.hotdata.dev/v1` |
| `HOTDATA_APP_URL` | App URL for browser login | `https://app.hotdata.dev` |

Precedence for the API key, lowest to highest: config file → `HOTDATA_API_KEY` → `--api-key`.

## Development

```sh
cargo build && cargo test
```

Release process: see [docs/RELEASING.md](docs/RELEASING.md).
