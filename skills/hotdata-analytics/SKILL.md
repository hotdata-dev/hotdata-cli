---
name: hotdata-analytics
description: Use this skill when the user wants OLAP-style SQL analytics in Hotdata — aggregations, GROUP BY, JOINs, reporting, exploratory queries, query run history, stored results, or materialized follow-up tables (Chain via datasets or managed databases). Activate for "analyze", "aggregate", "rollup", "pivot", "report", "metrics", "GROUP BY", "query history", "past queries", "query runs", "stored results", "materialize", "chain", "intermediate table", or sorted indexes for filters/range scans. Do not load for BM25/vector search or geospatial SQL — use hotdata-search or hotdata-geospatial. Requires the core hotdata skill for connections, tables, datasets, and auth.
version: 0.2.5
---

# Hotdata Analytics Skill

**OLAP-style analytics** in Hotdata: PostgreSQL-dialect SQL, query execution, run history, stored results, **Chain** materializations, and **sorted** indexes for filters and joins.

**Prerequisites:** Authenticate, workspace, and catalog discovery via the **`hotdata`** skill (`connections`, `tables`, `datasets`, `databases`).

**Related skills:** **`hotdata-search`** (BM25, vector, retrieval indexes), **`hotdata-geospatial`** (spatial SQL).

---

## Execute SQL

```bash
hotdata query "<sql>" [--workspace-id <workspace_id>] [--connection <connection_id>] [--output table|json|csv]
hotdata query status <query_run_id> [--output table|json|csv]
```

- **PostgreSQL dialect.** Quote mixed-case identifiers: `"CustomerName"`.
- Use **`hotdata tables list`** for schema discovery — not `information_schema` via `query`.
- Fully qualified names: `<connection>.<schema>.<table>`, `datasets.<schema>.<table>`, `<database>.<schema>.<table>`.
- Long-running queries may return `query_run_id` → poll with **`query status`** (exit `2` = still running). Do not re-run identical heavy SQL while polling.
- For **workspace-wide** joins and naming, load **context:DATAMODEL** when listed (`hotdata context list` → `show DATAMODEL`) — see **`hotdata`** skill.

### OLAP patterns

Typical analytics SQL (all via `hotdata query`):

- **Aggregations:** `COUNT`, `SUM`, `AVG`, `MIN`, `MAX` with `GROUP BY`
- **Joins:** `INNER` / `LEFT JOIN` across `<connection>.<schema>.<table>` names
- **Filtering:** `WHERE` on partition-friendly columns (consider **sorted** indexes below)
- **Ordering:** `ORDER BY` on metrics or dimensions
- **Bounded exploration:** always `LIMIT` while iterating; widen once validated

Column names from CSV uploads may be case-sensitive — use double quotes when not all-lowercase.

---

## Query run history

Uses the **active workspace only** (no `--workspace-id`; set with `hotdata workspaces set`).

```bash
hotdata queries list [--limit <int>] [--cursor <token>] [--status <csv>] [--output table|json|yaml]
hotdata queries <query_run_id> [--output table|json|yaml]
```

- `list` — status, duration, row count, SQL preview (default limit 20). Filter: `--status running,failed`.
- `<query_run_id>` — full metadata, formatted SQL, `result_id` when present.
- Use history to find recurring `WHERE` / `JOIN` / `GROUP BY` patterns before adding indexes (search skill) or chains.

---

## Stored results

```bash
hotdata results list [--workspace-id <workspace_id>] [--limit <int>] [--offset <int>] [--output table|json|yaml]
hotdata results <result_id> [--workspace-id <workspace_id>] [--output table|json|csv]
```

- Prefer **`results <id>`** over re-running identical heavy queries.
- Query footers may include `[result-id: rslt...]`; also available from `queries <query_run_id>`.

---

## Chain (materialized follow-ups)

**Pattern:** run SQL → materialize a smaller table → query the materialized name.

1. **Base query**

   ```bash
   hotdata query "SELECT ..."
   hotdata query status <query_run_id>   # if async
   ```

2. **Materialize** (pick one)

   ```bash
   hotdata datasets create --label "chain slice" --sql "SELECT ..." [--table-name chain_slice]
   hotdata datasets create --label "from saved" --query-id <query_id> [--table-name ...]
   ```

   Or managed parquet:

   ```bash
   hotdata databases create --name analytics --table slice
   hotdata databases tables load analytics slice --file ./slice.parquet
   ```

3. **Chain query** — use printed **`full_name`** or `datasets list` **FULL NAME** column:

   ```bash
   hotdata query "SELECT * FROM datasets.main.chain_slice WHERE ..."
   hotdata query "SELECT * FROM analytics.public.slice WHERE ..."
   ```

Document stable chains in **context:DATAMODEL → Derived tables (Chain)**.

Full procedure: [references/WORKFLOWS.md](references/WORKFLOWS.md).

---

## Sorted indexes (filters and range scans)

For equality, range, and sort-heavy OLAP — not full-text or vector (see **`hotdata-search`**):

```bash
hotdata indexes create --connection-id <id> --schema <schema> --table <table> \
  --name idx_orders_created --columns created_at --type sorted [--async]
```

List and delete use the same `hotdata indexes` commands as in the search skill; only **`--type sorted`** is the analytics focus here.

---

## Sandboxes and chains

Sandbox datasets use **`datasets.<sandbox_id>.<table>`**, not `datasets.main`. Run queries with active sandbox config or `hotdata sandbox <id> run hotdata query "..."`. See **`hotdata`** skill **Sandboxes**.
