---
name: hotdata-search
description: Use this skill when the user wants full-text search, BM25 keyword search, vector similarity search, semantic search, embeddings, or retrieval indexes in Hotdata. Activate for "hotdata search", "BM25", "full-text", "vector search", "semantic search", "similarity", "embedding", "embedding provider", "create an index" (bm25 or vector), "list indexes" for search, or SQL using bm25_search or vector_distance, filtered or joined vector search, hybrid BM25 + vector search (RRF), search latency, or running search SQL over the HTTP API. Do not load for general SQL analytics (aggregations, GROUP BY) or geospatial work — use hotdata-analytics or hotdata-geospatial instead. Requires the core hotdata skill for auth and workspace basics.
version: 0.46.0
---

# Hotdata Search Skill

Retrieval workloads in Hotdata: **BM25 full-text**, **vector similarity**, and the **indexes** and **embedding providers** that power them.

**Prerequisites:** Authenticate, set a workspace, and set a current database (`hotdata databases use <id>`) — see the **`hotdata`** skill. Use fully qualified table names: `<catalog>.<schema>.<table>`.

**Related sub-skills** (bundled alongside this one — `Read` on demand): **`hotdata-analytics`** ([`../analytics/SKILL.md`](../analytics/SKILL.md) — OLAP SQL, query history, materialized chains), **`hotdata-geospatial`** ([`../geospatial/SKILL.md`](../geospatial/SKILL.md) — PostGIS-style functions).

---

## Search CLI

Both run server-side. The search action addresses an index **by name** (`--index`, alias `--in`); the index's type, column, and provider come from the index itself.

```bash
# BM25 / text (requires a text index; address it by name)
hotdata search "<query>" --index <name> \
  [--database <db-id>] [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]

# Vector (requires a vector index; server auto-embeds the query text)
hotdata search "<query>" --index <name> \
  [--database <db-id>] [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]

# --in is an accepted alias for --index
hotdata search "<query>" --in <name>
```

| Type | Behavior |
|------|----------|
| **`bm25`** | Server generates `bm25_search(table, col, 'text')`. Results sort by score (descending). |
| **`vector`** | Pass plain-text query; name the **source text column** (e.g. `title`). Server embeds using the same provider/metric/dimensions as the index. SQL uses `vector_distance(col, 'text')`. Results sort by distance (ascending). |

- **Index name:** the index carries its own type, column, and provider — you only name the index. Use `search list` to see available index names.
- **`--select`:** defaults to the table's own columns. An auto-embed vector index materialises a `{column}_embedding` column on the table, and a search leaves it out — it is a 1536-float list in every row, tens of kilobytes per search nobody asked for. Ask for it back with `--select '*'`, or name it (`--select 'id,body_embedding'`); pair either with `--output json`, since `--output table` abbreviates long lists for display.
- **Custom embedding model, raw query vector, or no vector index?** Use `hotdata query` directly (e.g. `cosine_distance(col, [<vec>])`) — `search` only auto-embeds the query text via the index's own provider.
- **`ivf` indexes:** an `ivf` index created over a text column with `--provider` is searched by text like any vector index: `search` embeds the text with the index's provider and runs `SELECT <the table's columns>, vector_distance(<text col>, '<text>') AS dist FROM <table> ORDER BY dist LIMIT <k>`. It names the columns rather than using `*`, because an `ivf` index cannot serve a query that returns its vector column (`SELECT *` and `* EXCLUDE (...)` included); a `--select` you pass is used verbatim, so leave the generated `{column}_embedding` out of it. An `ivf` index over a column that already holds vectors has no model to embed the text, so `search` refuses it and prints the query to run instead: `hotdata query 'SELECT <columns>, <l2_distance|cosine_distance>(<col>, ARRAY[<query vector>]) AS dist FROM <catalog.schema.table> ORDER BY dist LIMIT <k>'` (the function matches the index's metric). In hand-written SQL against an `ivf` index, name the columns to return the same way; `vector_search(...)` reads `hnsw` indexes only and is refused on `ivf`.
- **Before search:** create the right index (`search create <name> --type text` or `--type vector`). See [references/INDEXES.md](references/INDEXES.md).
- Default `--limit` is 10.
- **Database:** the search commands resolve the index in the **current** database (`hotdata databases use <id>`). Pass `-d/--database <database>` (name, catalog alias, or id) to target a different database explicitly — it is required when no current database is set. The same `-d/--database` works on `search show` and `search remove`.
- **Current database:** with `hotdata databases use <db>`, an index created with a `schema.table` `--from` resolves the current database's catalog automatically. Or create it with the full `catalog.schema.table` form. Do **not** use the internal `__db_<id>` label or raw catalog ID prefix — `bm25_search`/`vector_distance` resolve a catalog attached to the current database, so an `__db_…` or `conn…` prefix errors with *catalog … is not attached*.

---

## Search in SQL

`hotdata search` only runs one fixed query shape. For filters, joins, hybrid ranking or server-side use, write the SQL yourself and run it with `hotdata query` (or the HTTP API, below). The examples use `db.s.chunks(id, doc_id, court, text)` with a text index and an embedding-backed vector index on `text`, and `db.s.docs(id, title)`.

### `bm25_search` output

```sql
SELECT id, doc_id, text, score
FROM bm25_search('db.s.chunks', 'text', 'breach of fiduciary duty')
ORDER BY score DESC
LIMIT 10
```

- **Columns:** every column of the source table, plus `score` (`Float32`, higher is better). The hits *are* the source rows, so there is nothing to join back.
- **Order:** rows come back in table order, not by relevance. Always write `ORDER BY score DESC`.
- **Limit:** a `LIMIT` on that `ORDER BY score DESC` reaches the search. Without one (an aggregate, a `GROUP BY`, a second sort key, a filter on `score`), the search stops at **1000** hits. An optional 4th argument caps it explicitly: `bm25_search('db.s.chunks', 'text', 'q', 200)`.
- **`WHERE`:** a filter on the table's own columns (`WHERE court = 'NY'`) still fills the `LIMIT` when enough rows match: the search reads deeper into the ranking and, for a very selective filter, pre-scans the filter instead.

### Filtered vector search

```sql
-- ANN step on the indexed table alone; the join goes outside
WITH hits AS (
  SELECT id, doc_id, text, vector_distance(text, 'breach of fiduciary duty') AS dist
  FROM db.s.chunks
  WHERE court = 'NY'
  ORDER BY dist
  LIMIT 50
)
SELECT h.id, d.title, h.text, h.dist
FROM hits h JOIN db.s.docs d ON d.id = h.doc_id
ORDER BY h.dist
LIMIT 10
```

- **A `WHERE` on the indexed table's own columns is a pre-filter, not a post-filter.** It is applied before the nearest-neighbour search, so you still get `k` rows whenever `k` rows match.
  - `hnsw`: the engine collects the keys that pass the filter and searches only those; when the filter keeps about 5% of the table or less, it computes exact distances over the passing rows instead.
  - `ivf`: the filter is applied while the clusters are probed, and the probe widens until enough rows pass. It falls back to an exact scan when the filter names a column the index does not hold, calls a volatile function, or fewer than `k` indexed rows pass.
- **Keep the index-served step to one table:** `SELECT … vector_distance(...) AS dist FROM <indexed table> [WHERE <its columns>] ORDER BY dist LIMIT k`. Anything else under that `ORDER BY` (a `JOIN`, a subquery filter, a filter on another table) is not served by the index. It falls back to exact distances over every row, which is correct but slow on a large table. `vector_distance` written directly over a join may not resolve at all. Do the ANN step in a CTE or subquery, as above, and join its result.
- **Filter on a joined table?** Copy the column onto the indexed table, or over-fetch in the CTE (`LIMIT 200`) and filter after the join, accepting that fewer than `k` rows may survive.
- **`ivf`:** name the columns to return. An `ivf` index cannot serve a query that returns the vector column (`SELECT *` and `* EXCLUDE (...)` included).
- **Check the plan** with `hotdata query 'EXPLAIN <the SQL>'`. An index-served `hnsw` search shows `USearchExec`. If the plan shows a full scan of the table instead, the index is not being used.

### Hybrid search (BM25 + vector, RRF)

There is no built-in hybrid mode. Combine the two rankings in SQL with reciprocal rank fusion (RRF). A table can carry a text index and an embedding-backed vector index side by side (each needs its own index name). If creating the text index on a large table fails, that is the BM25 build-size limit, not a clash with the vector index.

```sql
WITH b AS (
  SELECT id, ROW_NUMBER() OVER (ORDER BY score DESC) AS r
  FROM (SELECT id, score FROM bm25_search('db.s.chunks', 'text', 'breach of fiduciary duty')
        ORDER BY score DESC LIMIT 100) AS bq
),
v AS (
  SELECT id, ROW_NUMBER() OVER (ORDER BY dist) AS r
  FROM (SELECT id, vector_distance(text, 'breach of fiduciary duty') AS dist FROM db.s.chunks
        ORDER BY dist LIMIT 100) AS vq
)
SELECT COALESCE(b.id, v.id) AS id,
       COALESCE(1.0 / (60 + b.r), 0) + COALESCE(1.0 / (60 + v.r), 0) AS rrf
FROM b FULL JOIN v ON b.id = v.id
ORDER BY rrf DESC
LIMIT 10
```

- Keep each leg as its own `ORDER BY … LIMIT` subquery on the base table, so each one is served by its index. Put a filter inside both legs, as a `WHERE` on the table's columns.
- `60` is the usual RRF constant. Weight a leg by scaling its term. Join the fused ids back to `db.s.chunks` (or `docs`) for display columns.

### Latency (`execution_time_ms`)

- For a vector search by text, `execution_time_ms` includes embedding the query text with the index's provider (one call to e.g. OpenAI, typically a few hundred ms). The text is embedded while the query is planned, and the timer starts before planning.
- Embeddings are cached in the query engine process by provider, model, dimensions and text. A repeat of the **same** text usually skips the provider call. A different text, or a request served by another engine process, pays for it again.
- The response does not split embedding time from search time. To isolate the search, run the same SQL twice and compare the second run, or run `EXPLAIN`: a warm search that is still slow usually means the plan fell back to an exact scan (see **Filtered vector search**).
- BM25 searches make no provider call.

### Programmatic access (HTTP)

Anything `hotdata query` or `hotdata search` runs can be sent from a server to the query API:

```bash
curl -s https://api.hotdata.dev/v1/query \
  -H "Authorization: Bearer $HOTDATA_API_KEY" \
  -H "X-Workspace-Id: <workspace_id>" \
  -H "X-Database-Id: <database_id>" \
  -H "Content-Type: application/json" \
  -d '{"sql": "SELECT id, text, score FROM bm25_search('\''db.s.chunks'\'', '\''text'\'', '\''fiduciary duty'\'') ORDER BY score DESC LIMIT 10"}'
```

- **Headers:** `Authorization: Bearer <api key>` and `X-Workspace-Id` always. An instant database's tables also need the database: send `X-Database-Id: <database_id>`, or `"database_id"` in the body (if both are set they must match).
- **Response:** `{"result_id", "columns", "rows", "row_count", "execution_time_ms"}`. Add `"async": true` (optionally `"async_after_ms": 1000`–`30000`) to get a query run id for a long query instead, and poll `GET /v1/query-runs/{id}`.
- **SDKs:** the generated SDKs send `X-Workspace-Id` but not `X-Database-Id`. Pass `database_id` in the request body, or add the header as an extra request header.
- There is no separate search endpoint: `hotdata search` builds a `bm25_search(...) ORDER BY score DESC LIMIT k` or `vector_distance(...) AS dist … ORDER BY dist LIMIT k` query and sends it here, so send the same SQL.

---

## Indexes (text and vector)

Indexes are an **instant-database** concept, and belong to the database's own tables — an attached database is read-only, so index its tables there rather than here. Create names the index (positional) and attaches to a table via `--from` — `catalog.schema.table` (the database's own catalog), or `schema.table` with a current database set. `list` narrows to `-d/--database <database>`, else the **current database** when one is set; without either it scans the whole workspace. `show`/`remove` resolve the index by name in the current database (or `--database <database>`).

```bash
# List — -d/--database <database>, else the current database, else whole-workspace scan
hotdata search list [-d <db-id>] [--schema <schema>] [--table <table>] \
  [--workspace-id <ws>] [--output table|json|yaml]

# Create — index name is positional; --from is an instant database's table
# (catalog.schema.table, or schema.table in -d/--database or the current database)
hotdata search create <name> --type text|vector --from <catalog.schema.table> \
  --column <col> [-d <db-id>] \
  [--metric l2|cosine|dot] [--async] \
  [--algorithm hnsw|ivf] [--nlist <n>] [--probe-fraction <f>] \
  [--vector-precision float64|float32|float16|float8|int8] \
  [--provider <id>] [--dimensions <n>] [--output-column <name>] [--description <text>]

# Show — by index name, in the current database (or -d/--database <database>)
hotdata search show <name> [-d <db-id>] [--output table|json|yaml]

# Remove — by index name, in the current database (or -d/--database <database>)
hotdata search remove <name> [-d <db-id>]
```

- **`--type` is required** on create: `text` (BM25; one or more text columns, comma-separated in `--column`) or `vector` (exactly one column; often embeddings or auto-embedded text). (`sorted` is also a valid `--type`, covered in **`hotdata-analytics`** — [`../analytics/SKILL.md`](../analytics/SKILL.md).)
- **`sorted`** indexes (range/equality for OLAP filters) are documented in **`hotdata-analytics`** ([`../analytics/SKILL.md`](../analytics/SKILL.md)) — this skill focuses on retrieval types.
- **`-d/--database <id>`** on create picks the database for a `schema.table` `--from` (instead of the active one); it cannot be combined with a `catalog.schema.table` `--from`.
- **`search show`** prints a vector index's `metric`, `algorithm`, `nlist`, `probe_fraction` and `vector_precision` when the API reports them; `search list` adds `ALGORITHM` and `NLIST` columns, and its `-o json|yaml` carries all of them. For an embedding-backed vector index (`hnsw` or `ivf` over a text column with a provider), `search show` reports the text column as `column` and the generated one as `embedding_column`, and `search list` shows `COLUMNS` as `<col>_embedding (from <col>)` (`source_column` in `-o json|yaml`). `nlist` is the number of clusters an `ivf` index was built with, which can be fewer than the `--nlist` requested on a small table.
- **`--async`:** poll with `hotdata jobs <job_id>` (see **`hotdata`** skill **Jobs**). An index is listed by `search list` only once its build finishes; while builds are running, `search list` prints a note with their count.
- **Vector algorithm (`--algorithm`, default `hnsw`):** `hnsw` searches an in-memory graph whose memory grows with the table. `ivf` clusters the vectors and reads only the clusters nearest each search, so its memory follows how much a search reads rather than the table's size. `ivf` works on a column that already holds vectors, or on a text column with `--provider` (embeddings are generated as for `hnsw`), and needs the `l2` or `cosine` metric (`dot` is refused). IVF-only tuning: `--nlist <n>` (number of clusters, 1–65536; default chosen from the table's size) and `--probe-fraction <f>` (share of the index a search reads, 0 < f ≤ 1; higher = better recall, slower); both are rejected unless `--algorithm ivf`.
- **`--vector-precision`:** how compactly the index stores each vector value. Defaults: the column's own precision for `hnsw`, `int8` for `ivf`. `ivf` accepts `int8` and `float32` only; `hnsw` accepts everything except `int8` (`float8` is its 8-bit option). Lower precision shrinks the index at some recall cost — measure on your own data. Changing it means removing and recreating the index.
- **Auto-embedding:** `--type vector` on a **text** column generates embeddings server-side. Optional `--provider`; default output column `{column}_embedding` (override with `--output-column`). The generated column becomes part of the table, so it shows up in `information_schema.columns` and in a hand-written `SELECT *` — `hotdata search` leaves it out of its own projection (see `--select` above).

Full workflow (gather workload → compare existing → create → verify): [references/INDEXES.md](references/INDEXES.md).

---

## Embedding providers

```bash
hotdata search embeddings list [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata search embeddings show <id> [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata search embeddings add --name <name> --provider-type service|local \
  [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata search embeddings update <id> [--name <name>] [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata search embeddings remove <id> [--workspace-id <workspace_id>]
```

- System providers (e.g. `sys_emb_openai`) are pre-configured; use `list` for IDs to pass to `--provider`.
- `--provider-api-key` is the **embedding service** key (not Hotdata `--api-key`). `--secret-name` references an existing secret.

---

## Quick workflow

1. `hotdata databases use <id>` — set a current database, then `hotdata databases tables list` to confirm column types.
2. `hotdata search list` — avoid duplicate indexes (scoped to active DB automatically).
3. `hotdata search create <name> --type text|vector --from <catalog.schema.table> --column <col>` (add `--async` if large).
4. `hotdata search "..." --index <name>` — address the index you created by name.
5. Record what exists in **context:DATAMODEL** (core skill) when the workspace should remember index choices.
