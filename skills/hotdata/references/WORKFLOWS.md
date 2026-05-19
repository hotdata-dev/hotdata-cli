# Hotdata CLI workflows

**Notation:** **`context:<STEM>`** (e.g. **`context:DATAMODEL`**) means the workspace document stored via the **context API**—CLI uses bare stems: `hotdata context show DATAMODEL`.

---

## Which skill?

Load **`hotdata`** first for auth and workspace setup. Add a sub-skill only when the task needs it.

| User goal | Skill | Key commands |
|-----------|--------|----------------|
| Login, workspaces, connections, tables, context, sandboxes | **`hotdata`** | `auth`, `workspaces`, `connections`, `tables`, `context`, `sandbox` |
| Upload CSV/JSON/URL or SQL-derived tables | **`hotdata`** | `datasets create`, `databases …` (see below) |
| SQL analytics, aggregations, history, Chain | **`hotdata-analytics`** | `query`, `queries`, `results`, `datasets create --sql` |
| BM25 / vector search, retrieval indexes | **`hotdata-search`** | `search`, `indexes create`, `embedding-providers` |
| Geospatial / PostGIS-style SQL | **`hotdata-geospatial`** | `query` with `ST_*`, WKB columns |

| Concept | Where documented |
|--------|------------------|
| **Model** | This file — [Model](#model) |
| **Upload path (datasets vs databases)** | This file — [Datasets vs managed databases](#datasets-vs-managed-databases) |
| **Sandboxes** | This file — [Sandboxes and datasets](#sandboxes-and-datasets) |
| **History / Chain** | **`hotdata-analytics`** — [WORKFLOWS.md](../../hotdata-analytics/references/WORKFLOWS.md) |
| **Search indexes** | **`hotdata-search`** — [INDEXES.md](../../hotdata-search/references/INDEXES.md) |

---

## Datasets vs managed databases

Both land queryable tables in the workspace; the path depends on **format** and **how you want to name tables in SQL**.

| | **Datasets** | **Managed databases** |
|---|-------------|------------------------|
| **Best for** | CSV, JSON, URL import, stdin, SQL/query snapshot | Parquet files you own; catalog-style `name.schema.table` |
| **SQL prefix** | `datasets.<schema>.<table>` (often `datasets.main.*`) | `<database>.<schema>.<table>` (database = connection name) |
| **CLI** | `hotdata datasets create` | `hotdata databases create` + `databases tables load` |
| **Declare schema up front** | No | Yes — `--table` on create (required before load on current API) |
| **Parquet** | Yes (`--file`, `--url`, `--upload-id`) | **Only** parquet on `tables load` |
| **Refresh upstream** | `datasets refresh` (URL/query sources) | Replace via `tables load` again |

**Rule of thumb:** CSV/JSON or “upload a file from a URL” → **datasets**. Parquet catalog you control as **`mydb.public.orders`** → **databases**.

### Workflow: dataset upload and query

1. Authenticate and set workspace (`hotdata auth`, `hotdata workspaces set` if needed).
2. Create the dataset (one source):

   ```bash
   hotdata datasets create --label "Orders" --file ./orders.csv
   # or: --url "https://example.com/orders.parquet"
   # or: --sql "SELECT ..."   # materialize from a query
   ```

3. Note the printed **`full_name`** (e.g. `datasets.main.orders`) — do not assume `datasets.main`.
4. Inspect if needed: `hotdata datasets list`, `hotdata datasets <dataset_id>`.
5. Query:

   ```bash
   hotdata query "SELECT count(*) FROM datasets.main.orders"
   ```

### Workflow: managed database (parquet)

1. Create the database and **declare tables** up front:

   ```bash
   hotdata databases create --name sales --table orders --table customers
   ```

2. Load parquet per table:

   ```bash
   hotdata databases tables load sales orders --file ./orders.parquet
   ```

   If load fails with *not declared*, add `--table` at create time. There is no `--url` on load — download parquet locally first.

3. Confirm and query:

   ```bash
   hotdata databases tables list sales
   hotdata query "SELECT count(*) FROM sales.public.orders"
   ```

For **Chain** materializations into datasets or databases, see **`hotdata-analytics`**.

---

## Model

**Goal:** A markdown map of entities, keys, grain, and how connections relate—stored as **context:DATAMODEL** on top of the live **catalog** from Hotdata.

### Initialize

1. Use [DATA_MODEL.template.md](DATA_MODEL.template.md) as the **structure** for **context:DATAMODEL**.
2. Run **`hotdata context list`**. **Only if** `DATAMODEL` appears, use `show` or `pull`. If absent, start from the template—**do not** run `show` (exits 1).
3. Edit `./DATAMODEL.md` in the project directory, then **`hotdata context push DATAMODEL`**.

### Deep model pass (optional)

Follow **[MODEL_BUILD.md](MODEL_BUILD.md)** for connector enrichment, per-table detail, and index/search notes in the data model.

### Refresh catalog facts

When metadata may be **stale**, run `connections refresh` before `tables list`. After **`databases tables load`**, refresh is not required for the new table—use `databases tables list` or `tables list`.

```bash
hotdata workspaces list
hotdata connections list
hotdata connections refresh <connection_id>   # after DDL / stale remote metadata
hotdata tables list
hotdata tables list --connection-id <connection_id>
hotdata datasets list
hotdata datasets <dataset_id>
hotdata databases list
```

Use `hotdata tables list` for discovery; do not query `information_schema` for that.

---

## Sandboxes and datasets

Use this when work is isolated in a **sandbox** (exploratory runs, ephemeral datasets).

**Active sandbox vs `sandbox run`:** After `sandbox new` or `sandbox set`, run **`datasets create`**, **`query`**, etc. **directly**. **`sandbox run <cmd>`** (no id before `run`) **always creates a new sandbox**.

**Qualified names:** Workspace datasets → **`datasets.main.<table>`**. Sandbox datasets → **`datasets.<sandbox_id>.<table>`**. Use **`full_name`** from create or **FULL NAME** from `datasets list`.

**Access:** Sandbox-only tables need active sandbox config or **`hotdata sandbox <id> run …`**.

**SQL:** Quote mixed-case columns with double quotes.

**Listing:** `datasets list` returns all workspace datasets; use **FULL NAME** to spot sandbox vs `main` rows.

---

## Cross-cutting

- **Workspace:** Active workspace or `--workspace-id`. **`hotdata queries`** uses the active workspace only (no `--workspace-id`).
- **Jobs:** `hotdata jobs list` / `jobs <id>` for async refreshes, dataset refresh, and index builds.
- **Discovery:** `hotdata tables list` — not `query` on `information_schema`.
