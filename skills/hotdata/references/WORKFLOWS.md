# Hotdata CLI workflows

**Notation:** **`context:<STEM>`** (e.g. **`context:DATAMODEL`**) means the workspace document stored via the **context API**—CLI uses bare stems: `hotdata context show DATAMODEL`.

---

## Which skill?

Load **`hotdata`** first for auth and workspace setup. Add a sub-skill only when the task needs it.

| User goal | Skill | Key commands |
|-----------|--------|----------------|
| Login, workspaces, connections, tables, context | **`hotdata`** | `auth`, `workspaces`, `connections`, `tables`, `context` |
| Upload CSV/JSON/URL or SQL-derived tables | **`hotdata`** | `datasets create`, `databases …` (see below) |
| SQL analytics, aggregations, history, Chain | **`hotdata-analytics`** | `query`, `queries`, `results`, `datasets create --sql` |
| BM25 / vector search, retrieval indexes | **`hotdata-search`** | `search`, `indexes create`, `embedding-providers` |
| Geospatial / PostGIS-style SQL | **`hotdata-geospatial`** | `query` with `ST_*`, WKB columns |

| Concept | Where documented |
|--------|------------------|
| **Model** | This file — [Model](#model) |
| **Upload path (datasets vs databases)** | This file — [Datasets vs managed databases](#datasets-vs-managed-databases) |
| **History / Chain** | **`hotdata-analytics`** — [WORKFLOWS.md](../../hotdata-analytics/references/WORKFLOWS.md) |
| **Search indexes** | **`hotdata-search`** — [INDEXES.md](../../hotdata-search/references/INDEXES.md) |
| **Epic flows** | This file — [Epic flows](#epic-flows) |

---

## Epic flows

End-to-end checklists. Use the linked sections for command detail and guardrails.

### Onboard a workspace

**Skill:** **`hotdata`** (optional **`hotdata-analytics`** for first queries)

1. [ ] `hotdata auth login` (or `hotdata auth`)
2. [ ] `hotdata workspaces list` → `hotdata workspaces set` if not on the right workspace
3. [ ] `hotdata connections list` — note connection ids and names
4. [ ] (Optional) `hotdata connections create …` — see **`hotdata`** skill **Create a Connection**
5. [ ] `hotdata connections refresh <connection_id>` if catalog may be stale
6. [ ] `hotdata tables list` and `hotdata tables list --connection-id <id>` for columns
7. [ ] (Optional) `hotdata context list` — if `DATAMODEL` is listed, `hotdata context show DATAMODEL`; else skip `show`
8. [ ] (Optional) Bootstrap **context:DATAMODEL** — [Model](#model), [DATA_MODEL.template.md](DATA_MODEL.template.md)

**Next:** upload data ([Datasets vs managed databases](#datasets-vs-managed-databases)) or run analytics (**Chain** below).

### Chain (materialize then query)

**Skill:** **`hotdata-analytics`** (catalog via **`hotdata`**)

1. [ ] Run base SQL: `hotdata query "SELECT …"` — poll `hotdata query status <id>` if async
2. [ ] Materialize one way:
   - [ ] **Dataset:** `hotdata datasets create --name <name> [--description "…"] --sql "SELECT …"`
   - [ ] **Managed DB:** `hotdata databases create --catalog <alias> --table <name>` then `hotdata databases load --catalog <alias> --table <name> --file ./….parquet`
3. [ ] Copy **`full_name`** from create output (or `datasets list` **FULL NAME**)
4. [ ] Chain: `hotdata query "SELECT … FROM <full_name> WHERE …"`
5. [ ] Record stable chains in **context:DATAMODEL** when they should outlive the session

**Detail:** [hotdata-analytics WORKFLOWS — Chain](../../hotdata-analytics/references/WORKFLOWS.md#chain)

### Retrieval (index then search)

**Skill:** **`hotdata-search`** (schema via **`hotdata`**)

1. [ ] `hotdata tables list --connection-id <id>` — pick text column (BM25) or embedding/text column (vector)
2. [ ] `hotdata indexes list` — avoid duplicate bm25/vector indexes on the same column
3. [ ] Create index:
   - [ ] **Managed DB:** `hotdata indexes create --catalog <alias> --table <tbl> --column <text_col> --type bm25|vector`
   - [ ] **Connection:** `hotdata indexes create --connection-id <id> --schema <s> --table <t> --column <col> --type bm25|vector [--metric cosine|l2|dot]`
   - [ ] Large build: add `--async`, then `hotdata jobs <job_id>`
4. [ ] Search (--type and --column inferred when one search index exists):
   - [ ] `hotdata search "…" --table <catalog.schema.table>` (auto-infer)
   - [ ] `hotdata search "…" --table … --type bm25 --column <col>` (explicit)
5. [ ] (Optional) Note indexes in **context:DATAMODEL → Search & index summary**

**Detail:** [hotdata-search INDEXES.md](../../hotdata-search/references/INDEXES.md)

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
2. Create the dataset — `--name` is the SQL table name (required); `--description` is the display label (optional):

   ```bash
   hotdata datasets create --name orders --sql "SELECT ..."
   # or: --query-id <saved_query_id>
   ```

   For parquet file uploads use **managed databases** instead (see below).

3. Note the printed **`full_name`** (e.g. `datasets.main.orders`) — do not assume `datasets.main`.
4. Inspect if needed: `hotdata datasets list`, `hotdata datasets <dataset_id>`.
5. Query:

   ```bash
   hotdata query "SELECT count(*) FROM datasets.main.orders"
   ```

### Workflow: managed database (parquet)

1. Create the database with a catalog alias:

   ```bash
   hotdata databases create --catalog sales
   ```

2. Load parquet per table (tables are auto-declared if needed):

   ```bash
   hotdata databases load --catalog sales --table orders --file ./orders.parquet
   hotdata databases load --catalog sales --table customers --url https://example.com/customers.parquet
   ```

3. Confirm and query:

   ```bash
   hotdata databases tables list
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

## Cross-cutting

- **Workspace:** Active workspace or `--workspace-id`. **`hotdata queries`** uses the active workspace only (no `--workspace-id`).
- **Jobs:** `hotdata jobs list` / `jobs <id>` for async refreshes, dataset refresh, and index builds.
- **Discovery:** `hotdata tables list` — not `query` on `information_schema`.
