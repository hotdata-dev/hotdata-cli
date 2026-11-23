---
name: hotdata-analytics
description: Use this skill when the user wants OLAP-style SQL analytics in Hotdata — aggregations, GROUP BY, JOINs, reporting, exploratory queries, query run history, stored results, materialized follow-up tables (Chain into instant databases), or rollups (`hotdata rollups`, a table kept pre-aggregated by time). Activate for "analyze", "aggregate", "rollup", "pre-aggregate", "time buckets", "dashboard is slow", "pivot", "report", "metrics", "GROUP BY", "query history", "past queries", "query runs", "stored results", "materialize", "chain", "intermediate table", or sorted indexes for filters/range scans. Do not load for BM25/vector search or geospatial SQL — use hotdata-search or hotdata-geospatial. Requires the core hotdata skill for tables and auth.
version: 0.46.0
---

# Hotdata Analytics Skill

**OLAP-style analytics** in Hotdata: PostgreSQL-dialect SQL, query execution, run history, stored results, **Chain** materializations, **sorted** indexes for filters and joins, and **rollups** for aggregates over long time ranges.

**Prerequisites:** Authenticate, workspace, and catalog discovery via the **`hotdata`** skill (`ingest sources`/`ingest`, `databases tables`, `databases`).

**Related sub-skills** (bundled alongside this one — `Read` on demand): **`hotdata-search`** ([`../search/SKILL.md`](../search/SKILL.md) — BM25, vector, retrieval indexes), **`hotdata-geospatial`** ([`../geospatial/SKILL.md`](../geospatial/SKILL.md) — spatial SQL).

---

## Execute SQL

```bash
hotdata query "<sql>" [--workspace-id <workspace_id>] [--database <database>] [--dialect hotsql|duckdb|postgres|snowflake|clickhouse] [--output table|json|csv]
hotdata query status <query_run_id>
```

- **PostgreSQL dialect.** Quote mixed-case identifiers: `"CustomerName"`.
- **`--dialect`** (default `hotsql`): write SQL in `duckdb`/`postgres`/`snowflake`/`clickhouse` and the server transpiles it to HotSQL before running (e.g. Snowflake `IFF(...)`, DuckDB `len(...)`, ClickHouse `countIf(...)`). Writes follow the same rules as `hotsql`.
- **Writes:** `INSERT` (append only), `CREATE TABLE`, `CREATE TABLE AS SELECT`, and `DROP TABLE` run through `query` against the current database's own tables and return one row, `rows_affected`; `UPDATE`/`DELETE` are not supported in SQL yet (use keyed `hotdata databases load --mode update|delete|upsert`). Rules and refusals in the **`hotdata`** skill → Execute SQL Query.
- Use **`hotdata databases tables list`** for schema discovery — not `information_schema` via `query`.
- Fully qualified names: `<catalog>.<schema>.<table>`, `<database>.<schema>.<table>`.
- **Query scope:** every query runs inside one instant database (active or `--database`); it sees that database's own catalog plus whatever is **attached** to it only. To read another database's table, or **join your own table against one**, attach that database first: `hotdata databases attach <database>` — see **`hotdata`** skill → [Querying across databases](../../SKILL.md#querying-across-databases-attach). No instant database set → *"a database is required."*
- Long-running queries may return `query_run_id` → poll with **`query status`** — exit `0` succeeded, `1` failed, `2` still running (poll again), `3` succeeded but the printed result is an incomplete/truncated preview. Do not re-run identical heavy SQL while polling.
- **Big result sets:** use `-o csv` or `-o json` — both stream the full persisted result with flat memory. `-o table` is a human view capped at 10,000 rows; past that the footer reads `N of TOTAL rows — INCOMPLETE PREVIEW` and the command exits `3`. Details in the **`hotdata`** skill → Execute SQL Query.
- For **workspace-wide** joins and naming, load **context:DATAMODEL** when listed (`hotdata databases context list` → `show DATAMODEL`) — see **`hotdata`** skill.

### OLAP patterns

Typical analytics SQL (all via `hotdata query`):

- **Aggregations:** `COUNT`, `SUM`, `AVG`, `MIN`, `MAX` with `GROUP BY`
- **Joins:** `INNER` / `LEFT JOIN` across `<catalog>.<schema>.<table>` names — every referenced catalog (the instant database's own or an attached one) must be in the current database's scope; attach catalogs first (`hotdata databases attach`)
- **Filtering:** `WHERE` on partition-friendly columns (consider **sorted** indexes below)
- **Ordering:** `ORDER BY` on metrics or dimensions
- **Bounded exploration:** always `LIMIT` while iterating; widen once validated

Column names from CSV uploads may be case-sensitive — use double quotes when not all-lowercase.

---

## Query run history

Scoped to the **current database** (set with `hotdata databases use`); pass `-d/--database <database>` (name, catalog alias, or id) to target another, `-w/--workspace-id` for another workspace.

```bash
hotdata databases queries list [-d <db-id>] [--workspace-id <workspace_id>] [--limit <int>] [--cursor <token>] [--status <csv>] [--output table|json|yaml]
hotdata databases queries <query_run_id> [-d <db-id>] [--workspace-id <workspace_id>] [--output table|json|yaml]
```

- `list` — status, duration, row count, SQL preview (default limit 20). Filter: `--status running,failed`.
- `<query_run_id>` — full metadata, formatted SQL, `result_id` when present.
- Use history to find recurring `WHERE` / `JOIN` / `GROUP BY` patterns before adding indexes (search skill) or chains.

---

## Stored results

```bash
hotdata databases results list [-d <db-id>] [--workspace-id <workspace_id>] [--limit <int>] [--offset <int>] [--output table|json|yaml]
hotdata databases results get <result_id> [-d <db-id>] [--workspace-id <workspace_id>] [--output table|json|csv]
```

- Prefer **`databases results get <id>`** over re-running identical heavy queries. It renders like `query`: `csv`/`json` stream the whole result, `table` is capped at 10,000 rows and exits `3` when the result is larger. Results and query runs scope to the current database; pass `-d/--database <database>` (name, catalog alias, or id) to target another.
- Query footers may include `[result-id: rslt...]`; also available from `databases queries <query_run_id>`.
- `databases results list --limit` defaults to **100** (max **1000**) — unlike `databases queries list`, which defaults to **20**.

---

## Chain (materialized follow-ups)

**Pattern:** run SQL → materialize a smaller table → query the materialized name.

1. **Base query**

   ```bash
   hotdata query "SELECT ..."
   hotdata query status <query_run_id>   # if async
   ```

2. **Materialize** into an instant database (parquet)

   ```bash
   hotdata databases create --catalog analytics   # prints the new id
   hotdata databases use <id>                     # create does not switch
   hotdata databases load --catalog analytics --table slice --file ./slice.parquet
   ```

3. **Chain query** — use the catalog-qualified name `<catalog>.public.<table>`:

   ```bash
   hotdata query "SELECT * FROM analytics.public.slice WHERE ..."
   ```

Document stable chains in **context:DATAMODEL → Derived tables (Chain)**.

Full procedure: [references/WORKFLOWS.md](references/WORKFLOWS.md).

---

## Sorted indexes (filters and range scans)

For equality, range, and sort-heavy OLAP — not full-text or vector (see **`hotdata-search`**):

```bash
hotdata search create idx_orders_created --type sorted \
  --from <catalog-alias>.<schema>.<table> --column created_at [--async]
```

List and remove use the same `hotdata search` commands as in the search skill; only **`--type sorted`** is the analytics focus here. With `--async`, track the build via **`hotdata jobs list`** (see **`hotdata`** skill → Jobs).

---

## Rollups (pre-aggregated time series)

A rollup keeps a table pre-aggregated by time: one row per time bucket and combination of dimension values, with the aggregates you choose. Use one for dashboards and reports that aggregate a large, time-stamped table over days or weeks. Queries are **not** rewritten: keep querying the table, and an aggregate query the rollup can answer reads it instead. Answers are the same as reading the table, except percentiles, which are approximate. Guide: <https://www.hotdata.dev/docs/rollups>.

```bash
hotdata rollups create <schema.table> --time-column <col> --bucket <duration> \
  --measure <kind[:column]> [--measure ...] [--dimensions <a,b>] [--filter "<sql>"] \
  [--window <duration>] [--database <database>] [--async] [--output table|json|yaml]
hotdata rollups list [--database <database>] [--schema <schema>] [--table <schema.table>] [--output table|json|yaml]
hotdata rollups show <rollup_id> [--database <database>] [--output table|json|yaml]
hotdata rollups rebuild <rollup_id> [--database <database>] [--async] [--output table|json|yaml]
hotdata rollups window <schema.table> <duration> [--database <database>] [--output table|json|yaml]
hotdata rollups remove <rollup_id> [--database <database>]
```

```bash
hotdata rollups create public.requests --time-column ts --bucket 1h \
  --dimensions region,status \
  --measure count --measure numeric:bytes --measure approx_percentile:latency_ms
```

- **Table:** `schema.table` in `--database` / the current database, or `catalog.schema.table`. It must already hold data, and `--time-column` must be a UTC timestamp.
- **Durations** (`--bucket`, `--window`, `rollups window`): `30s`, `5m`, `1h`, `1d`, or a bare number of seconds. `--bucket` is the finest time grouping the rollup answers.
- **`--measure`** (repeat it): `count` (or `count:col` for non-null values), `sum:col`, `min:col`, `max:col`, `numeric:col` (answers `count`, `sum`, `avg`, `min`, `max` of the column — there is no `avg` kind), `approx_percentile:col[:centroids]` (answers `approx_percentile_cont` / `approx_median`).
- **`--dimensions`** (alias `--by`): every column the queries group or filter by, besides time. A query that filters on any other column cannot use the rollup.
- **`--filter`:** keeps only matching rows. A query uses the rollup only when its `WHERE` contains the same condition written the same way.
- **Waiting:** `create` and `rebuild` wait until the rollup is `ready` and exit `1` if the build fails; they print the rollup id (and job id) to stderr first, so the id survives a timeout. A first build of a large table can take a long time — pass `--async` to return at once, then poll `hotdata rollups show <id>` until `status` is `ready`.
- **`-o json|yaml`** prints the API objects unchanged (`bucket_width_secs`, `window_secs` in seconds). `create`/`rebuild` print `{rollup, job_id}`; `list` prints an array.
- **A rollup cannot be edited.** `show` prints measures in the `--measure` grammar; to change one, `remove` it and `create` it again (which rebuilds from the start).

**Did my query use it?** `hotdata query` shows it: the table footer ends with `[rollup: <outcome>]`, `-o json` carries a `rollup` field, and `hotdata databases queries <query_run_id>` shows it for past runs. No field means no table the query read has a rollup.

| `rollup` | Meaning | What to do |
| :-- | :-- | :-- |
| `used` | A rollup answered the query. | — |
| `not_matched` | A rollup is ready but cannot answer this query's shape. | Use only the rollup's measures; group time with `date_trunc`/`date_bin` at the bucket width or a multiple; filter and group only by its dimensions and time column; include its `--filter` condition verbatim. |
| `not_ready` | The rollup is building, needs a rebuild, or its build failed. | `hotdata rollups show <id>`; `hotdata rollups rebuild <id>` if it says so. |
| `unavailable` | Skipped this once; the query read the table. | Nothing — the same query later can use it. |

**Creating a rollup restricts writes to its table** (tell the user before creating one on a table they edit):

- Rows older than the **window** (default `1h`, shared by every rollup on the table) can no longer be changed or deleted. Change it with `hotdata rollups window <schema.table> <duration>`; a wider window applies as time passes — rows already past the old window stay closed.
- SQL `DELETE` and `UPDATE` on the table are refused. Use a keyed `hotdata databases load --mode update|delete|upsert` whose key **includes the time column**.
- Appends are always accepted. A `--mode replace` load is accepted too, but leaves the table's rollups `needs_rebuild`: queries read the table until `hotdata rollups rebuild <id>`.
- A refused write is a `409` with code `ROLLUP_COVERAGE_EDIT`; retrying it does not help — its message says which rule it broke.

Forking a database does not copy its rollups; dropping a table drops them.

