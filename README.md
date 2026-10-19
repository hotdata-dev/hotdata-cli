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

Load your data into a Hotdata database, then query, join, and search it with
plain SQL. Data can come from files on your machine, a URL, or an external
source such as Postgres, S3, Iceberg, Kafka, or one of ~150 API services.

📖 Full documentation: [hotdata.dev/docs](https://www.hotdata.dev/docs)

## Install

```sh
curl -fsSL https://hotdata.dev/install | sh
```

That's it. The installer uses Homebrew if you have it
(`brew install hotdata-dev/tap/cli`) and also installs the Hotdata skill for AI
coding agents such as Claude Code. You can also download a binary from
[Releases](https://github.com/hotdata-dev/hotdata-cli/releases).

The CLI tells you when a new version is out; upgrade with
`hotdata manage upgrade`.

## Quickstart

```sh
# 1. Log in (or create an account with: hotdata auth register)
hotdata auth login

# 2. Create a database and make it your current one
hotdata databases create --catalog demo
hotdata databases use demo

# 3. Load some data
hotdata databases load --catalog demo --table trips \
  --url https://d37ci6vzurychx.cloudfront.net/trip-data/yellow_tripdata_2024-01.parquet

# 4. Query it
hotdata query "SELECT count(*) FROM demo.public.trips"
```

Tables are addressed as `catalog.schema.table`. Here the catalog is `demo` and
the default schema is `public`. The examples below build on this `demo`
database. For a longer walkthrough, see the
[quick start guide](https://www.hotdata.dev/docs/quick-start).

## Load your data

**From a file or URL** (CSV, JSON, or Parquet). Point `--file` at your own
file, or `--url` at one online:

```sh
hotdata databases load --catalog demo --table my_table --file ./my-data.csv
```

A load replaces the table. To add rows instead, use `--mode append`:

```sh
hotdata databases load --catalog demo --table my_table --file ./more-data.csv --mode append
```

To change existing rows by key, use `--mode update`, `--mode delete`, or
`--mode upsert` with `--key <column>`.

**From an external source** such as Postgres, MySQL, S3, GCS, Iceberg, Kafka,
or an API service: first save the connection details as a *datasource*, then
create an *ingest* that copies data into your database once or on a
schedule.

```sh
hotdata ingest sources types        # see what you can connect to
hotdata ingest sources add          # save a datasource (you'll be prompted)
hotdata ingest create --source "prod postgres" --table orders --database-id <database-id>
hotdata ingest list                 # check on your ingests
```

More in the docs: [loading data](https://www.hotdata.dev/docs/push-data) and
[importing from external sources](https://www.hotdata.dev/docs/pull-data).

## Query

```sh
hotdata databases tables list                       # what can I query?
hotdata databases tables show demo.public.trips     # columns and types
hotdata query "SELECT * FROM demo.public.trips LIMIT 10"
```

SQL is PostgreSQL-style. Add `-o json` or `-o csv` to change the output format.
If you prefer another dialect, pass `--dialect duckdb`, `postgres`, or
`snowflake` and the query is translated for you.

You can also write with SQL. `INSERT`, `CREATE TABLE`, `CREATE TABLE AS`, and
`DROP TABLE` work on your current database's tables (your API key needs the
`write` permission):

```sh
hotdata query "CREATE TABLE events (id BIGINT, kind TEXT)"
hotdata query "INSERT INTO events VALUES (1, 'signup'), (2, 'login')"
```

`UPDATE` and `DELETE` aren't supported in SQL yet; use `databases load` with
`--mode update`, `--mode delete`, or `--mode upsert` instead.

Long-running queries finish in the background. The CLI prints an id you can
check with `hotdata query status <id>`.

Column names are case-sensitive: wrap names with capital letters in double
quotes, like `"PULocationID"`. See the [SQL reference](https://www.hotdata.dev/docs/sql)
for the supported syntax and functions.

## Join across databases

Keep reference data in its own database and attach it wherever you need it. No
data is copied. For example, load NYC's taxi zone names into a `zones`
database, attach it to `demo`, and find the busiest pickup zones:

```sh
hotdata databases create --catalog zones
hotdata databases load --catalog zones --table zones \
  --url https://d37ci6vzurychx.cloudfront.net/misc/taxi_zone_lookup.csv
hotdata databases attach zones

hotdata query 'SELECT z."Zone", count(*) AS trips
               FROM demo.public.trips t
               JOIN zones.public.zones z ON z."LocationID" = t."PULocationID"
               GROUP BY z."Zone" ORDER BY trips DESC LIMIT 5'
```

Attached tables are read-only. `hotdata databases detach zones` removes the
link without deleting anything.

## Search

Build a search index on a column, then search it. Using the `zones` table from
above:

```sh
hotdata search create zone_names --type text --from zones.public.zones --column Zone
hotdata search "airport" --index zone_names -d zones
```

That returns JFK, LaGuardia, and Newark airports. An index lives in the same
database as its table, so pass `-d` when that isn't your current database.

- `--type text` is full-text (keyword) search.
- `--type vector` is semantic search. Hotdata creates the embeddings for you,
  so you don't need an embeddings API key.
- `--type sorted` speeds up `WHERE` filters and range queries on a column.

## Your current workspace and database

Most commands run against your *current* workspace and database, so you don't
have to name them every time.

- Switch with `hotdata workspaces use` and `hotdata databases use`.
- Target a different one for a single command with `-w <workspace-id>` or
  `-d <database>`. `-d` accepts a database's name, catalog, or id.
- `workspaces list` and `databases list` mark the current one with `*`.

Your choice is saved on this machine and survives logging out and back in.

## Scripts, CI, and AI agents

- Add `-o json` or `-o yaml` to any listing for machine-readable output.
- Authenticate without a browser using an API key: `--api-key <key>`, or
  `HOTDATA_API_KEY` in your environment or a `.env` file.
- `hotdata query status` and `hotdata ingest run` return exit codes scripts can
  check: `0` done, `1` failed, `2` still running.
- `hotdata manage skills install` installs bundled agent skills — Markdown
  playbooks that teach AI coding agents (Claude Code and friends) the full CLI.
- `hotdata databases context push DATAMODEL` stores a Markdown description of
  your data model in Hotdata, so teammates and agents work from the same map.

### Claude Code plugin

To get the agent skill through Claude Code's plugin manager instead, which
updates it with each release:

```
/plugin marketplace add hotdata-dev/claude-plugins
/plugin install hotdata-cli@hotdata
```

Self-hosted marketplaces don't auto-update by default. To get each release
automatically, open `/plugin`, go to **Marketplaces**, select `hotdata`, and
choose **Enable auto-update**. Otherwise run `/plugin marketplace update hotdata`
when a new version ships.

Use either the plugin or the installer's skill, not both: if you installed the
CLI with the script above, install it with `HOTDATA_NO_SKILLS=1` or Claude Code
will list the skill twice.

What the plugin runs, sends, and fetches:

- **SessionStart hook** (`hooks/check-cli.sh`, Claude Code and Cowork only):
  checks whether `hotdata` is on `PATH` or in `~/.hotdata/cli`. If it's only in
  `~/.hotdata/cli`, it adds that folder to `PATH` for the session. If it's
  missing, it asks Claude to offer to install the CLI. The hook makes no
  network requests, installs nothing, and sends no data.
- **Installer**, only after you agree: `curl -fsSL https://hotdata.dev/install |
  HOTDATA_NO_SKILLS=1 sh` downloads the CLI from this repository's GitHub
  releases, or installs it with Homebrew if `brew` is present.
- **The skill** teaches Claude to run `hotdata` commands. Those commands talk to
  the Hotdata API (`https://api.hotdata.dev/v1`, or `HOTDATA_API_URL`) with your
  Hotdata credentials, and send the queries, files, and data source settings
  you ask Claude to work with. `hotdata auth login` opens
  `https://app.hotdata.dev` in your browser, and the CLI checks this
  repository's GitHub releases for new versions (`HOTDATA_NO_UPDATE_CHECK=1`
  turns that off). Neither the plugin nor the CLI collects telemetry.

## Getting help

- [Documentation](https://www.hotdata.dev/docs), including the
  [CLI reference](https://www.hotdata.dev/docs/cli-reference) and the
  [agent skills guide](https://www.hotdata.dev/docs/agent-skills).
- `hotdata <command> --help` shows every option for a command.
- File a support ticket from the terminal; replies go to your account email:

  ```sh
  hotdata support report -m "Queries have been timing out since 2pm" --subject "Query timeouts"
  ```

## Reference

<details>
<summary>All commands</summary>

| Command | What it does |
| :-- | :-- |
| `auth login` | Log in via browser |
| `auth register` | Create a new account via browser (GitHub OAuth; `--email` for email + password) |
| `auth logout` | Remove authentication for a profile |
| `auth status` | Show authentication status |
| `workspaces list` | List all workspaces |
| `workspaces use` | Switch the current workspace (remembered across logins) |
| `databases list` | List instant databases in the workspace |
| `databases count` | Count instant databases in the workspace |
| `databases show` | Show details for an instant database |
| `databases create` | Create a new instant database |
| `databases fork` | Fork a database into a new, independent database |
| `databases lineage` | Show a database's whole fork family tree |
| `databases attach` | Attach another database so its tables are queryable |
| `databases detach` | Detach a previously attached database |
| `databases use` | Switch the current database (remembered across logins) |
| `databases unset` | Clear the current database |
| `databases remove` | Delete a database and all its tables |
| `databases load` | Load a csv/json/parquet file or saved result into a table (`--mode replace\|append\|delete\|update\|upsert`) |
| `databases tables add` | Declare a table with its key and storage layout |
| `databases tables list` | List tables in a database |
| `databases tables show` | Show column definitions for a table |
| `databases tables load` | Same as `databases load`, addressed by database instead of catalog |
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
| `support report` | File a support ticket with the Hotdata team |

</details>

<details>
<summary>Configuration and environment variables</summary>

Config lives at `~/.hotdata/config.yml`.

| Variable | Description | Default |
| :-- | :-- | :-- |
| `HOTDATA_API_KEY` | API key (overrides config file; also read from `.env`) | |
| `HOTDATA_WORKSPACE` | Lock every command to one workspace (`workspaces use` refuses while set) | |
| `HOTDATA_DATABASE` | Database to use when `-d/--database` is not given; overrides the saved current database | |
| `HOTDATA_NO_UPDATE_CHECK` | Skip the interactive new-release check | |
| `HOTDATA_CONFIG_DIR` | Where config and credentials live | `~/.hotdata` |
| `HOTDATA_UPLOAD_CONCURRENCY` | Parallel parts for multipart uploads in `databases load` | `12` |
| `HOTDATA_API_URL` | API base URL | `https://api.hotdata.dev/v1` |
| `HOTDATA_APP_URL` | App URL for browser login | `https://app.hotdata.dev` |

An API key is taken from, in order of priority: `--api-key`, then
`HOTDATA_API_KEY`, then the config file.

Installer options: `HOTDATA_NO_BREW=1` skips Homebrew, `HOTDATA_NO_SKILLS=1`
skips the agent skill. Tab completion: `hotdata manage completions bash|zsh|fish`.

</details>

## Development

```sh
cargo build && cargo test
```

Build from source with `cargo install --path .` (requires Rust). Release
process: [docs/RELEASING.md](docs/RELEASING.md).
