# Hotdata CLI workflows

Procedures for **Model**, **History**, **Chain**, **Indexes**, and **sandboxes with datasets** (see **Sandboxes and datasets**). These compose existing `hotdata` commands; they are not separate subcommands.

**Notation:** **`context:<STEM>`** (e.g. **`context:DATAMODEL`**, **`context:GLOSSARY`**) means the **workspace document** stored under that stem via the **context API**—not generic “data model” language and not local files except as `pull`/`push` transport. **CLI** still uses bare stems: `hotdata context show DATAMODEL`.

## Where things live

| Concept | Location |
|--------|----------|
| **Model** | **`context:DATAMODEL`** — workspace context API (`hotdata context list` then `show` / `pull` / `push` with `./DATAMODEL.md` in the project cwd only as the CLI file surface; **list before `show`** so missing `DATAMODEL` does not error). Never store workspace-specific model text inside agent skill directories. |
| **History** | `hotdata queries list` / `queries <query_run_id>` for query runs (execution history); `hotdata results list` / `results <id>` for row data. |
| **Chain** | Intermediate tables in **`datasets.<schema>.<table>`** — usually **`datasets.main.*`** for workspace-wide materializations; **sandbox uploads** use **`datasets.<sandbox_id>.*`** (see **Sandboxes and datasets** below). Document stable chains in **context:DATAMODEL** under **Derived tables (Chain)**. |
| **Indexes** | Recommendations and live objects in Hotdata (`indexes list` / `indexes create`). Record rationale in **context:DATAMODEL** (e.g. Search & index summary) or a dedicated **context:** stem if you split concerns. |

---

## Model

**Goal:** A markdown map of entities, keys, grain, and how connections relate—stored as **context:DATAMODEL** on top of the live **catalog** from Hotdata.

### Initialize

1. Use [DATA_MODEL.template.md](DATA_MODEL.template.md) in this skill bundle as the **structure** for what you store as **context:DATAMODEL**.
2. Run **`hotdata context list`**. **Only if** `DATAMODEL` appears, you may use `hotdata context show DATAMODEL` or `pull` to hydrate `./DATAMODEL.md`. If it does **not** appear, start from the template only—**do not** run `show` (it exits 1). In the **project directory** where you run `hotdata`, create or refresh `./DATAMODEL.md`, fill workspace-specific sections as you discover schema, then **`hotdata context push DATAMODEL`** so the server owns **context:DATAMODEL**.
3. Agents that skip local files: **`context list`** first; **`context show DATAMODEL` only when listed** to read **context:DATAMODEL**; when updating, write `./DATAMODEL.md` then `hotdata context push DATAMODEL`.

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

`datasets list` returns **every** dataset in the workspace (no sandbox-only filter). Use the **`FULL NAME`** column (`datasets.<schema>.<table>`): **`main`** in the middle segment is the usual workspace catalog; a value like **`s_…`** is the **sandbox id** for sandbox-scoped datasets.

Use output to update **Connections**, **Tables**, **Columns**, and **Datasets** in **context:DATAMODEL** (edit via `./DATAMODEL.md` + `hotdata context push DATAMODEL`, or your editor workflow). Optional: small exploratory queries once names are known:

```bash
hotdata query "SELECT * FROM <connection>.<schema>.<table> LIMIT 5"
```

**Rule:** Use `hotdata tables list` for discovery; do not use `query` against `information_schema` for that (see main skill).

---

## Sandboxes and datasets

Use this when work is isolated in a **sandbox** (exploratory runs, ephemeral datasets).

**Active sandbox vs `sandbox run`:** After `hotdata sandbox new` or `hotdata sandbox set <sandbox_id>`, run **`hotdata datasets create`**, **`hotdata query`**, etc. **directly** — the CLI attaches the sandbox from saved config. **`hotdata sandbox run <cmd>`** (no sandbox id before `run`) **always creates a new sandbox**; it does **not** reuse the active one. To wrap a command in an **existing** sandbox, use **`hotdata sandbox <sandbox_id> run <cmd> [args…]`**.

**Qualified table names:** Workspace-wide dataset tables are typically **`datasets.main.<table_name>`**. Datasets created **inside** a sandbox use **`datasets.<sandbox_id>.<table_name>`**, not `main`. After **`datasets create`**, use the printed **`full_name`**; after **`datasets list`**, use the **`FULL NAME`** column — do not assume `datasets.main` for sandbox data.

**Access:** Queries against sandbox-only tables need sandbox context: **active sandbox in config** (`sandbox set`) **or** commands run under **`hotdata sandbox <sandbox_id> run …`**. Otherwise you may see **access denied**.

**Listing:** `datasets list` does not filter by sandbox; use **`FULL NAME`** to distinguish `…main…` from `…s_…` rows.

**SQL:** Column names from uploads that are not all-lowercase are **case-sensitive** in PostgreSQL; quote with double quotes (e.g. `"CustomerName"`).

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
hotdata results list [--workspace-id <workspace_id>] [--limit N] [--offset N]
hotdata results <result_id> [--workspace-id <workspace_id>]
```

Query footers include a `result-id` when applicable—record it for later, or pick it up from `queries <query_run_id>`. **Prefer `hotdata results <result_id>` over re-running identical heavy SQL.**

---

## Chain

**Goal:** Follow-up analysis on a **bounded** intermediate without rescanning huge base tables.

**Pattern:** materialize → query using the dataset’s **qualified name** (`datasets.<schema>.<table>`).

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

   Note the **`full_name`** line in the output (e.g. `datasets.main.chain_revenue_slice` or `datasets.s_….…` inside a sandbox).

3. **Chain** — query the dataset using that **`full_name`** (or **`FULL NAME`** from `datasets list`); do not hardcode `datasets.main` if the schema segment is a sandbox id:

   ```bash
   hotdata datasets list                    # FULL NAME column: datasets.<schema>.<table>
   hotdata query "SELECT * FROM datasets.main.<table_name> WHERE ..."   # workspace / no sandbox
   # Sandbox example (use the actual full_name from create or list):
   # hotdata query "SELECT * FROM datasets.s_ufmblmvq.<table_name> WHERE ..."
   ```

   For **sandbox-scoped** chain tables, ensure an **active sandbox** (`sandbox set`) or run the query inside **`hotdata sandbox <sandbox_id> run hotdata query "…"`**. Quote mixed-case columns: e.g. `"Revenue"`.

**Naming:** Prefer predictable `--table-name` values, e.g. `chain_<topic>_<YYYYMMDD>`, and list long-lived chains in **context:DATAMODEL → Derived tables (Chain)** (record the **full** `datasets.<schema>.<table>` you use in SQL).

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

Start broad, then narrow:

```bash
# All indexes on connection tables in the workspace (optional: -c / --schema / --table to filter)
hotdata indexes list [--workspace-id <workspace_id>]
```

For a single table, or to avoid scanning the whole workspace:

```bash
hotdata indexes list --connection-id <connection_id> --schema <schema> --table <table> [--workspace-id <workspace_id>]
```

Indexes on **uploaded datasets** are not included in that workspace scan — use `hotdata indexes list --dataset-id <dataset_id>` per dataset.

Skip creating a duplicate: same table + overlapping columns + same purpose (e.g. another bm25 on the same column).

### 3. Create indexes when justified

Use stable names (e.g. `idx_<table>_<columns>_<type>`). Examples:

```bash
# Sorted (default) — filters, joins, ordering on scalar columns
hotdata indexes create --connection-id <connection_id> --schema <schema> --table <table> \
  --name idx_orders_created --columns created_at --type sorted

# BM25 — full-text on one text column (required for bm25_search on that column)
hotdata indexes create --connection-id <connection_id> --schema <schema> --table <table> \
  --name idx_posts_body_bm25 --columns body --type bm25

# Vector — embeddings; requires --metric
hotdata indexes create --connection-id <connection_id> --schema <schema> --table <table> \
  --name idx_chunks_embedding --columns embedding --type vector --metric l2
```

Large builds: add `--async` and track with **`hotdata jobs list`** / **`hotdata jobs <job_id>`** (see main skill **Indexes** and **Jobs**).

### 4. Verify

Re-run representative **`hotdata query`** or **`hotdata search`** workloads. Update **context:DATAMODEL → Search & index summary** (`hotdata context push DATAMODEL` after editing `./DATAMODEL.md`) so future agents see what exists.

### Guardrails

- Prefer **evidence** (repeated predicates, slow queries, or planned search) over speculative indexes.
- **Production:** get explicit approval before `indexes create` when impact or cost is uncertain.
- Align **connection id**, **schema**, and **table** with `hotdata tables list` output.

---

## Cross-cutting

- **Workspace:** Use active workspace or `--workspace-id` when targeting a non-default workspace.
- **Sandboxes:** See **Sandboxes and datasets** above (`sandbox run` vs direct commands, `full_name`, access denied without context).
- **Jobs:** For async work (indexes, some refreshes), `hotdata jobs list` and `hotdata jobs <job_id>`.
