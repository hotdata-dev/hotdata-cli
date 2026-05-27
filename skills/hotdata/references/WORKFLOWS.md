# Hotdata CLI workflows

**Notation:** **`context:<STEM>`** (e.g. **`context:DATAMODEL`**) means the workspace document stored via the **context API**—CLI uses bare stems: `hotdata context show DATAMODEL`.

---

## Which skill?

Load **`hotdata`** first for auth and workspace setup. Add a sub-skill only when the task needs it.

| User goal | Skill | Key commands |
|-----------|--------|----------------|
| Login, workspaces, connections, tables, context, sandboxes | **`hotdata`** | `auth`, `workspaces`, `connections`, `tables`, `context`, `sandbox` |
| Upload SQL-derived tables | **`hotdata`** | `views create`, `databases …` (see below) |
| SQL analytics, aggregations, history, Chain | **`hotdata-analytics`** | `query`, `queries`, `results`, `views create --sql` |
| BM25 / vector search, retrieval indexes | **`hotdata-search`** | `search`, `indexes create`, `embedding-providers` |
| Geospatial / PostGIS-style SQL | **`hotdata-geospatial`** | `query` with `ST_*`, WKB columns |

| Concept | Where documented |
|--------|------------------|
| **Model** | This file — [Model](#model) |
| **Upload path (views vs databases)** | This file — [Views vs managed databases](#views-vs-managed-databases) |
| **Sandboxes** | This file — [Sandboxes and views](#sandboxes-and-views) |
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

**Next:** create views ([Views vs managed databases](#views-vs-managed-databases)) or run analytics (**Chain** below).

### Chain (materialize then query)

**Skill:** **`hotdata-analytics`** (catalog via **`hotdata`**)

1. [ ] Run base SQL: `hotdata query "SELECT …"` — poll `hotdata query status <id>` if async
2. [ ] Materialize one way:
   - [ ] **View:** `hotdata views create --name "…" --sql "SELECT …"`
   - [ ] **Managed DB:** `hotdata databases create --name … --table …` then `hotdata databases tables load … --file ./….parquet`
3. [ ] Copy **`full_name`** from create output (or `views list` **FULL NAME**)
4. [ ] Chain: `hotdata query "SELECT … FROM <full_name> WHERE …"`
5. [ ] (Sandbox) Use `views.<sandbox_id>.<table>` and active sandbox or `hotdata sandbox <id> run …`
6. [ ] Record stable chains in **context:DATAMODEL** when they should outlive the session

**Detail:** [hotdata-analytics WORKFLOWS — Chain](../../hotdata-analytics/references/WORKFLOWS.md#chain)

### Retrieval (index then search)

**Skill:** **`hotdata-search`** (schema via **`hotdata`**)

1. [ ] `hotdata tables list --connection-id <id>` — pick text column (BM25) or embedding/text column (vector)
2. [ ] `hotdata indexes list` — avoid duplicate bm25/vector indexes on the same column
3. [ ] Create index:
   - [ ] **Keyword:** `hotdata indexes create … --type bm25 --columns <text_col>`
   - [ ] **Semantic:** `hotdata indexes create … --type vector --columns <col> [--metric cosine|l2|dot]`
   - [ ] Large build: add `--async`, then `hotdata jobs <job_id>`
4. [ ] Search:
   - [ ] `hotdata search "…" --type bm25 --table <connection.schema.table> --column <col>`
   - [ ] `hotdata search "…" --type vector --table … --column <source_text_col>`
5. [ ] (Optional) Note indexes in **context:DATAMODEL → Search & index summary**

**Detail:** [hotdata-search INDEXES.md](../../hotdata-search/references/INDEXES.md)

---

## Views vs managed databases

Both land queryable tables in the workspace; the path depends on **source** and **how you want to name tables in SQL**.

| | **Views** | **Managed databases** |
|---|-----------|------------------------|
| **Best for** | SQL/query snapshot | Parquet files you own; catalog-style `name.schema.table` |
| **SQL prefix** | `views.<schema>.<table>` (often `views.main.*`) | `<database>.<schema>.<table>` (database = connection name) |
| **CLI** | `hotdata views create` | `hotdata databases create` + `databases tables load` |
| **Declare schema up front** | No | Yes — `--table` on create (required before load on current API) |
| **Parquet** | No | **Only** parquet on `tables load` |
| **Refresh upstream** | `views refresh` (query sources) | Replace via `tables load` again |

**Rule of thumb:** SQL-query snapshot → **views**. Parquet catalog you control as **`mydb.public.orders`** → **databases**.

### Workflow: view creation and query

1. Authenticate and set workspace (`hotdata auth`, `hotdata workspaces set` if needed).
2. Create the view:

   ```bash
   hotdata views create --name orders --sql “SELECT ...”
   # or: --query-id <saved_query_id>  # materialize from a saved query
   ```

3. Note the printed **`full_name`** (e.g. `views.main.orders`) — do not assume `views.main`.
4. Inspect if needed: `hotdata views list`, `hotdata views <view_id>`.
5. Query:

   ```bash
   hotdata query “SELECT count(*) FROM views.main.orders”
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

For **Chain** materializations into views or databases, see **`hotdata-analytics`**.

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
hotdata views list
hotdata views <view_id>
hotdata databases list
```

Use `hotdata tables list` for discovery; do not query `information_schema` for that.

---

## Sandboxes and views

Use this when work is isolated in a **sandbox** (exploratory runs, ephemeral views).

**Active sandbox vs `sandbox run`:** After `sandbox new` or `sandbox set`, run **`views create`**, **`query`**, etc. **directly**. **`sandbox run <cmd>`** (no id before `run`) **always creates a new sandbox**.

**Qualified names:** Workspace views → **`views.main.<table>`**. Sandbox views → **`views.<sandbox_id>.<table>`**. Use **`full_name`** from create or **FULL NAME** from `views list`.

**Access:** Sandbox-only tables need active sandbox config or **`hotdata sandbox <id> run …`**.

**SQL:** Quote mixed-case columns with double quotes.

**Listing:** `views list` returns all workspace views; use **FULL NAME** to spot sandbox vs `main` rows.

---

## Cross-cutting

- **Workspace:** Active workspace or `--workspace-id`. **`hotdata queries`** uses the active workspace only (no `--workspace-id`).
- **Jobs:** `hotdata jobs list` / `jobs <id>` for async refreshes, view refresh, and index builds.
- **Discovery:** `hotdata tables list` — not `query` on `information_schema`.
