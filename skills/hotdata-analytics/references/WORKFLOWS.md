# Analytics workflows

OLAP-style SQL, **History** (query runs and stored results), and **Chain** (materialized follow-ups). Requires **`hotdata`** for auth, workspaces, and catalog commands.

**Related:** **`hotdata-search`** for BM25/vector indexes and `hotdata search`; **`hotdata`** [WORKFLOWS.md](../../hotdata/references/WORKFLOWS.md) for datasets vs managed databases.

---

## History

**Goal:** Find prior work: query runs (execution history) and stored result rows.

### Query runs

Uses the **active workspace only** — no `--workspace-id` on `queries`. Set default workspace with `hotdata workspaces set` first.

```bash
hotdata queries list [--limit N] [--cursor <token>] [--status <csv>]
hotdata queries <query_run_id>
```

- `list` — status, creation time, duration, row count, truncated SQL preview (default limit 20).
- `--status` — filter comma-separated values, e.g. `--status running,failed`.
- `<query_run_id>` — full metadata (timings, `result_id`, snapshot, hashes) and formatted SQL.
- If a run has a `result_id`, fetch rows with `hotdata results <result_id>` below.

Use history to spot recurring `WHERE`, `JOIN`, `GROUP BY`, or search-style SQL before adding indexes (**`hotdata-search`**) or new Chain tables.

### Stored results

```bash
hotdata results list [--workspace-id <workspace_id>] [--limit N] [--offset N]
hotdata results <result_id> [--workspace-id <workspace_id>] [--output table|json|csv]
```

- Query footers may include `[result-id: rslt...]` — record it for later.
- Pick up `result_id` from `queries <query_run_id>` when present.
- **Prefer `hotdata results <result_id>` over re-running identical heavy SQL.** Re-runs waste resources and may return different data.

Results are paginated; the CLI hints the next `--offset` when more rows exist.

---

## Chain

**Goal:** Follow-up analysis on a **bounded** intermediate without rescanning huge base tables.

**Pattern:** run SQL → materialize → query the materialized **qualified name**.

### 1. Base query

```bash
hotdata query "SELECT ..."
```

- Quote mixed-case columns with double quotes (PostgreSQL dialect).
- If the CLI returns a `query_run_id`, poll instead of re-running:

  ```bash
  hotdata query status <query_run_id>
  ```

  Exit codes: `0` succeeded, `1` failed, `2` still running.

### 2. Materialize

Land a smaller table — pick one:

**Datasets** (CSV/JSON/URL/SQL snapshot → `datasets.<schema>.<table>`):

```bash
hotdata datasets create --label "chain revenue slice" --sql "SELECT ..." [--table-name chain_revenue_slice]
hotdata datasets create --label "from saved" --query-id <query_id> [--table-name ...]
```

**Managed database** (parquet → `<database>.<schema>.<table>`):

```bash
hotdata databases create --catalog chain_db
hotdata databases load --catalog chain_db --table revenue_slice --file ./revenue_slice.parquet
```

Note the printed **`full_name`** (e.g. `datasets.main.chain_revenue_slice` or `chain_db.public.revenue_slice`). For datasets, **`FULL NAME`** from `datasets list` is authoritative.

### 3. Chain query

Query using that name — do not hardcode `datasets.main` if the schema segment is a sandbox id:

```bash
hotdata datasets list
hotdata query "SELECT * FROM datasets.main.chain_revenue_slice WHERE ..."
# Sandbox example (use actual full_name from create or list):
# hotdata query "SELECT * FROM datasets.s_ufmblmvq.chain_revenue_slice WHERE ..."
# Managed database:
# hotdata query "SELECT * FROM chain_db.public.revenue_slice WHERE ..."
```

### Sandbox context

For **sandbox-scoped** chain tables:

- Qualified name is **`datasets.<sandbox_id>.<table>`**, not `datasets.main`.
- Run queries with **active sandbox** in config (`hotdata sandbox set`) **or** inside **`hotdata sandbox <sandbox_id> run hotdata query "…"`**.
- Without sandbox context, you may get **access denied** on sandbox-only tables.

### Naming and documentation

- Prefer predictable `--table-name` values: `chain_<topic>_<YYYYMMDD>`.
- Record long-lived chains in **context:DATAMODEL → Derived tables (Chain)** with the **full** SQL name you use (`datasets.…` or `database.schema.table`).
- Promote join/grain findings to **context:DATAMODEL** when they should outlive the sandbox (**`hotdata`** skill).

### Guardrails

- Materialize when the base scan is large and the follow-up runs many times.
- Keep Chain tables focused; avoid wide `SELECT *` materializations when a narrow projection suffices.
- For upload format choice (datasets vs databases), see **`hotdata`** WORKFLOWS — [Datasets vs managed databases](../../hotdata/references/WORKFLOWS.md#datasets-vs-managed-databases).
