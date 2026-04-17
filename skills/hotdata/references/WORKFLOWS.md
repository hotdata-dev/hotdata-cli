# Hotdata CLI workflows

Procedures for **Model**, **History**, **Chain**, and **Indexes**. These compose existing `hotdata` commands; they are not separate subcommands.

## Where files live

| Concept | Location |
|--------|----------|
| **Model** | Your **project** root or `docs/` (e.g. `DATA_MODEL.md` / `data_model.md`). Never store workspace-specific model text inside agent skill directories. |
| **History** | `hotdata queries list` / `queries <query_run_id>` for query runs (execution history); `hotdata results list` / `results <id>` for row data. |
| **Chain** | Intermediate tables in **`datasets.main.*`**; document stable ones in the Model file under **Derived tables (Chain)**. |
| **Indexes** | Recommendations and decisions live in Hotdata (`indexes list` / `indexes create`). Optional project log (e.g. `INDEXES.md`) if you track rationale outside the catalog. |

---

## Model

**Goal:** A markdown map of entities, keys, grain, and how connections relate—on top of the live **catalog** from Hotdata.

### Initialize

1. Copy `references/DATA_MODEL.template.md` from this skill bundle to your project as `DATA_MODEL.md` or `docs/DATA_MODEL.md`.
2. Fill workspace-specific sections as you discover schema.

### Deep model pass (optional)

For a **full** catalog-style document—datasets, enrichment from connector or loader docs (e.g. dlt), relationships, search/index notes, and stricter documentation rules—follow **[MODEL_BUILD.md](MODEL_BUILD.md)**. Use it when the light template is not enough; skip it for small or fast-moving workspaces.

### Refresh catalog facts (run from project root)

When metadata may be **stale**, run `connections refresh` for affected connections **before** relying on `tables list` (same order as below).

```bash
hotdata workspaces list
hotdata connections list
# For each connection you care about:
hotdata connections refresh <connection_id>   # after DDL / stale metadata
hotdata tables list
hotdata tables list --connection-id <connection_id>
hotdata datasets list
hotdata datasets <dataset_id>                # schema detail per dataset
```

Use output to update **Connections**, **Tables**, **Columns**, and **Datasets** in the model. Optional: small exploratory queries once names are known:

```bash
hotdata query "SELECT * FROM <connection>.<schema>.<table> LIMIT 5"
```

**Rule:** Use `hotdata tables list` for discovery; do not use `query` against `information_schema` for that (see main skill).

---

## History

**Goal:** Find prior work: query runs (execution history) and stored result rows.

### Query runs

```bash
hotdata queries list [--limit N] [--cursor <token>] [--status <csv>]
hotdata queries <query_run_id>
```

`queries list` returns recent executions with status, duration, row count, and a SQL preview (default limit 20). Filter with `--status` (e.g. `--status failed`). The detail view shows full timings, the `result_id` (if any), and the formatted SQL.

### Results

```bash
hotdata results list [-w <workspace_id>] [--limit N] [--offset N]
hotdata results <result_id> [-w <workspace_id>]
```

Query footers include a `result-id` when applicable—record it for later, or pick it up from `queries <query_run_id>`. **Prefer `hotdata results <result_id>` over re-running identical heavy SQL.**

---

## Chain

**Goal:** Follow-up analysis on a **bounded** intermediate without rescanning huge base tables.

**Pattern:** materialize → query `datasets.main.*`.

1. **Base** — run SQL:

   ```bash
   hotdata query "SELECT ..."
   ```

   If the CLI returns a `query_run_id`, poll:

   ```bash
   hotdata query status <query_run_id>
   ```

2. **Materialize** — land a table in datasets (pick one):

   ```bash
   hotdata datasets create --label "chain revenue slice" --sql "SELECT ..." [--table-name chain_revenue_slice]
   hotdata datasets create --label "from saved" --query-id <query_id> [--table-name ...]
   ```

3. **Chain** — query the dataset:

   ```bash
   hotdata datasets list                    # find table_name if needed
   hotdata query "SELECT * FROM datasets.main.<table_name> WHERE ..."
   ```

**Naming:** Prefer predictable `--table-name` values, e.g. `chain_<topic>_<YYYYMMDD>`, and list long-lived chains in **Model → Derived tables (Chain)**.

---

## Indexes

**Goal:** Find filters, joins, sorts, full-text, and vector access patterns that are **missing** indexes, then **create** them when the benefit is clear.

### 1. Gather workload and schema

- **Query-run history** — Inspect recent runs for recurring `WHERE`, `JOIN`, `GROUP BY`, `ORDER BY`, and any use of full-text or vector access (e.g. SQL that calls `bm25_search`, or workloads you run via **`hotdata search`** — see main skill **Search**).

  ```bash
  hotdata queries list
  hotdata queries <query_run_id>
  ```

- **Table/column types** — Confirm columns exist and types fit the index you plan:

  ```bash
  hotdata tables list --connection-id <connection_id>
  ```

High-cardinality **text** columns (`title`, `body`, `description`, …) may warrant **bm25** if you use or plan text search. **Embedding** / list-of-float columns may warrant **vector** (+ `--metric`). Equality/range/sort on discrete fields often map to **sorted** (default index type)—confirm fit with your workload and product limits when in doubt.

### 2. Compare to existing indexes

For each `connection.schema.table` you care about:

```bash
hotdata indexes list -c <connection_id> --schema <schema> --table <table> [-w <workspace_id>]
```

Skip creating a duplicate: same table + overlapping columns + same purpose (e.g. another bm25 on the same column).

### 3. Create indexes when justified

Use stable names (e.g. `idx_<table>_<columns>_<type>`). Examples:

```bash
# Sorted (default) — filters, joins, ordering on scalar columns
hotdata indexes create -c <connection_id> --schema <schema> --table <table> \
  --name idx_orders_created --columns created_at --type sorted

# BM25 — full-text on one text column (required for bm25_search on that column)
hotdata indexes create -c <connection_id> --schema <schema> --table <table> \
  --name idx_posts_body_bm25 --columns body --type bm25

# Vector — embeddings; requires --metric
hotdata indexes create -c <connection_id> --schema <schema> --table <table> \
  --name idx_chunks_embedding --columns embedding --type vector --metric l2
```

Large builds: add `--async` and track with **`hotdata jobs list`** / **`hotdata jobs <job_id>`** (see main skill **Indexes** and **Jobs**).

### 4. Verify

Re-run representative **`hotdata query`** or **`hotdata search`** workloads. Update **Model → Search & index summary** (if you maintain a data model doc) so future agents know what exists.

### Guardrails

- Prefer **evidence** (repeated predicates, slow queries, or planned search) over speculative indexes.
- **Production:** get explicit approval before `indexes create` when impact or cost is uncertain.
- Align **connection id**, **schema**, and **table** with `hotdata tables list` output.

---

## Cross-cutting

- **Workspace:** Use active workspace or `-w` / `--workspace-id` when targeting a non-default workspace.
- **Jobs:** For async work (indexes, some refreshes), `hotdata jobs list` and `hotdata jobs <job_id>`.
