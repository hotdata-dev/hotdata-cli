# Analytics workflows

OLAP-style SQL, **History** (query runs and stored results), and **Chain** (materialized follow-ups). Requires **`hotdata`** for auth, workspaces, and catalog commands.

**Related:** **`hotdata-search`** for BM25/vector indexes and `hotdata search`; **`hotdata`** [WORKFLOWS.md](../../hotdata/references/WORKFLOWS.md) for views vs managed databases.

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

**Views** (SQL snapshot → `views.<schema>.<table>`):

```bash
hotdata views create --name chain_revenue_slice --description "chain revenue slice" --sql "SELECT ..."
hotdata views create --name chain_from_saved --description "from saved" --query-id <query_id>
```

**Managed database** (parquet → `<database>.<schema>.<table>`):

```bash
hotdata databases create --name chain_db --table revenue_slice
hotdata databases tables load chain_db revenue_slice --file ./revenue_slice.parquet
```

Note the printed **`full_name`** (e.g. `views.main.chain_revenue_slice` or `chain_db.public.revenue_slice`). For views, **`FULL NAME`** from `views list` is authoritative.

### 3. Chain query

Query using that name — do not hardcode `views.main` if the schema segment is a sandbox id:

```bash
hotdata views list
hotdata query "SELECT * FROM views.main.chain_revenue_slice WHERE ..."
# Sandbox example (use actual full_name from create or list):
# hotdata query "SELECT * FROM views.s_ufmblmvq.chain_revenue_slice WHERE ..."
# Managed database:
# hotdata query "SELECT * FROM chain_db.public.revenue_slice WHERE ..."
```

### Sandbox context

For **sandbox-scoped** chain tables:

- Qualified name is **`views.<sandbox_id>.<table>`**, not `views.main`.
- Run queries with **active sandbox** in config (`hotdata sandbox set`) **or** inside **`hotdata sandbox <sandbox_id> run hotdata query "…"`**.
- Without sandbox context, you may get **access denied** on sandbox-only tables.

### Naming and documentation

- Prefer predictable `--table-name` values: `chain_<topic>_<YYYYMMDD>`.
- Record long-lived chains in **context:DATAMODEL → Derived tables (Chain)** with the **full** SQL name you use (`views.…` or `database.schema.table`).
- Promote join/grain findings to **context:DATAMODEL** when they should outlive the sandbox (**`hotdata`** skill).

### Guardrails

- Materialize when the base scan is large and the follow-up runs many times.
- Keep Chain tables focused; avoid wide `SELECT *` materializations when a narrow projection suffices.
- For source format choice (views vs databases), see **`hotdata`** WORKFLOWS — [Views vs managed databases](../../hotdata/references/WORKFLOWS.md#views-vs-managed-databases).
