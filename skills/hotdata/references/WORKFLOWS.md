# Hotdata CLI workflows

**Notation:** **`context:<STEM>`** (e.g. **`context:DATAMODEL`**) means the database-scoped document stored via the **context API** (active database; `-d`/`--database-id` to target another)—CLI uses bare stems: `hotdata context show DATAMODEL`.

---

## Which skill?

The `hotdata` skill is always loaded first (auth and workspace setup). The three specialized guides are **bundled inside it** under `subskills/` — `Read` one only when the task needs it.

| User goal | Skill | Key commands |
|-----------|--------|----------------|
| Login, workspaces, datasources, tables, context | **`hotdata`** | `auth`, `workspaces`, `datasource`, `ingest`, `tables`, `context` |
| Load parquet files into a managed database | **`hotdata`** | `databases create` + `databases load` |
| SQL analytics, aggregations, history, Chain | **`hotdata-analytics`** (`subskills/analytics/SKILL.md`) | `query`, `queries`, `results` |
| BM25 / vector search, retrieval indexes | **`hotdata-search`** (`subskills/search/SKILL.md`) | `search`, `indexes create`, `embedding-providers` |
| Geospatial / PostGIS-style SQL | **`hotdata-geospatial`** (`subskills/geospatial/SKILL.md`) | `query` with `ST_*`, WKB columns |

| Concept | Where documented |
|--------|------------------|
| **Model** | This file — [Model](#model) |
| **Upload path (managed databases)** | This file — [Managed databases](#managed-databases) |
| **History / Chain** | **`hotdata-analytics`** — [WORKFLOWS.md](../subskills/analytics/references/WORKFLOWS.md) |
| **Search indexes** | **`hotdata-search`** — [INDEXES.md](../subskills/search/references/INDEXES.md) |
| **Epic flows** | This file — [Epic flows](#epic-flows) |

---

## Epic flows

End-to-end checklists. Use the linked sections for command detail and guardrails.

### Onboard a workspace

**Skill:** **`hotdata`** (optional **`hotdata-analytics`** for first queries)

1. [ ] `hotdata auth login`
2. [ ] `hotdata workspaces list` → `hotdata workspaces set` if not on the right workspace
3. [ ] `hotdata databases list` and `hotdata tables list` — see the catalogs and tables you can query (tables print as `<catalog>.<schema>.<table>`)
4. [ ] `hotdata tables show <table>` for columns (add `--schema`/`--table` filters to `tables list` to narrow)
5. [ ] (Optional, to pull external data) `hotdata datasource list`; add one with `hotdata datasource create --family <f> --config @source.json`, then re-check its discovered schema with `hotdata datasource show <datasource_id>`. Load rows with `hotdata ingest create --datasource-id <id>` — see **`hotdata`** skill → **Ingest external data**
6. [ ] (Optional) `hotdata context list` — if `DATAMODEL` is listed, `hotdata context show DATAMODEL`; else skip `show`
7. [ ] (Optional) Bootstrap **context:DATAMODEL** — [Model](#model), [DATA_MODEL.template.md](DATA_MODEL.template.md)

**Next:** upload data ([Managed databases](#managed-databases)) or run analytics (**Chain** below).

### Chain (materialize then query)

**Skill:** **`hotdata-analytics`** (catalog via **`hotdata`**)

1. [ ] Run base SQL: `hotdata query "SELECT …"` — poll `hotdata query status <id>` if async
2. [ ] Materialize into a managed database: `hotdata databases create --catalog <alias> --table <name>` then `hotdata databases load --catalog <alias> --table <name> --file ./….parquet`
3. [ ] Query with the catalog-qualified name `<alias>.public.<name>`
4. [ ] Chain: `hotdata query "SELECT … FROM <alias>.public.<name> WHERE …"`
5. [ ] Record stable chains in **context:DATAMODEL** when they should outlive the session

**Detail:** [hotdata-analytics WORKFLOWS — Chain](../subskills/analytics/references/WORKFLOWS.md#chain)

### Retrieval (index then search)

**Skill:** **`hotdata-search`** (schema via **`hotdata`**)

1. [ ] `hotdata tables list` (filter with `--schema`/`--table`) — pick text column (BM25) or embedding/text column (vector)
2. [ ] `hotdata indexes list` — avoid duplicate bm25/vector indexes on the same column
3. [ ] Create index:
   - [ ] **Managed DB:** `hotdata indexes create --catalog <alias> --table <tbl> --column <text_col> --type bm25|vector`
   - [ ] **Catalog:** `hotdata indexes create --catalog <catalog-name-or-id> --schema <s> --table <t> --column <col> --type bm25|vector [--metric cosine|l2|dot]`
   - [ ] Large build: add `--async`, then `hotdata jobs <job_id>`
4. [ ] Search (--type and --column inferred when one search index exists):
   - [ ] `hotdata search "…" --table <catalog.schema.table>` (auto-infer)
   - [ ] `hotdata search "…" --table … --type bm25 --column <col>` (explicit)
5. [ ] (Optional) Note indexes in **context:DATAMODEL → Search & index summary**

**Detail:** [hotdata-search INDEXES.md](../subskills/search/references/INDEXES.md)

### Cross-source query (attach a catalog)

**Skill:** **`hotdata`**

A `hotdata query` runs inside **one** managed database; its scope sees that database's own catalog plus **attached** catalog catalogs only. To query a catalog's tables — or join a managed table against a live catalog table in one query — attach the catalog. (No managed database set → *"a database is required."*; an unattached catalog → *"table not found."*)

1. [ ] Pick/create the managed database that will be the query context (`hotdata databases set <id>` or `databases create --catalog <alias>`)
2. [ ] Attach the catalog(s) you need (live, sync intact): `hotdata databases attach <catalog> [--alias <a>]`
   - Or attach at creation: `hotdata databases create --catalog <alias> --attach <catalog>[=<alias>]`
3. [ ] Confirm scope: `hotdata databases <id>` lists attached catalogs
4. [ ] Query across sources: `hotdata query "SELECT … FROM <my_catalog>.public.<t> JOIN <catalog_or_alias>.<schema>.<table> ON …"`
5. [ ] (Optional) `hotdata databases detach <catalog|alias>` when finished; record required attachments in **context:DATAMODEL → Cross-catalog joins**

**Do not** export a catalog to parquet just to query it — attach is the live, sync-preserving path.

---

## Managed databases

**Managed databases** land queryable tables you own in the workspace, addressed in SQL as `<catalog>.<schema>.<table>` where the catalog is the `--catalog` alias.

| | **Managed databases** |
|---|------------------------|
| **Best for** | Parquet files you own; catalog-style `alias.schema.table` |
| **SQL prefix** | `<catalog>.<schema>.<table>` where catalog = `--catalog` alias |
| **CLI** | `hotdata databases create --catalog` + `databases load` |
| **Declare schema up front** | Yes — `--table` on create (auto-declared on first `databases load`) |
| **Parquet file uploads** | `databases load --file` / `--url` / `--upload-id` |
| **Refresh** | Replace via `databases load` again |

**Rule of thumb:** Parquet files you control as **`mydb.public.orders`** → **managed databases**.

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

   > Auto-declaring a *new* table recreates the database (no add-table API), which **changes its `id`** — the id returned by `databases create` goes stale after the next `load` of an undeclared table. Declare tables up front (`databases create --table orders --table customers`) to avoid the recreate, and don't cache ids across loads: re-read the current id from `databases list` at time of use. (Selection is still always by id — names and catalogs are not unique.)

3. Confirm and query:

   ```bash
   hotdata databases tables list
   hotdata query "SELECT count(*) FROM sales.public.orders"
   ```

For **Chain** materializations into managed databases, see **`hotdata-analytics`**.

### Workflow: fork before risky changes

Before destructive experimentation (bulk replaces, schema rework, testing a load pipeline), fork the database and experiment on the copy — the source stays untouched and the two diverge freely:

```bash
hotdata databases list                    # note the source database id (dbid...)
hotdata databases set <source_id>         # source to protect (`set` takes an id)
hotdata databases fork --expires-at 24h   # deep copy; becomes the active database — note the fork id it prints
hotdata databases load --catalog sales --table orders --file ./risky.parquet  # hits the fork
```

**Capture both ids.** After the fork, both databases answer to the same catalog alias (here `sales`), so ids are the only unambiguous way to refer to either one — the source id comes from `databases list` up front, the fork id from the `fork` output. The shared alias means experimental SQL runs unchanged against the fork. Attached catalogs are re-attached to the fork; indexes are not carried over. When done, keep the fork (`databases set <source_id>` to switch back to the source) or delete it (`databases delete <fork_id>`). Only DuckLake-backed databases can be forked — see `fork` in the main skill for details.

---

## Model

**Goal:** A markdown map of entities, keys, grain, and how catalogs relate—stored as **context:DATAMODEL** on top of the live **catalog** from Hotdata.

### Initialize

1. Use [DATA_MODEL.template.md](DATA_MODEL.template.md) as the **structure** for **context:DATAMODEL**.
2. Run **`hotdata context list`**. **Only if** `DATAMODEL` appears, use `show` or `pull`. If absent, start from the template—**do not** run `show` (exits 1).
3. Edit `./DATAMODEL.md` in the project directory, then **`hotdata context push DATAMODEL`**.

### Deep model pass (optional)

Follow **[MODEL_BUILD.md](MODEL_BUILD.md)** for connector enrichment, per-table detail, and index/search notes in the data model.

### Refresh catalog facts

A datasource's schema is discovered when it is added (`hotdata datasource create`); inspect the current discovered tables/columns with `hotdata datasource show <datasource_id>`. After **`databases tables load`**, no refresh is required for the new table—use `databases tables list` or `tables list`.

```bash
hotdata workspaces list
hotdata datasource list
hotdata datasource show <datasource_id>   # re-check discovered schema after source DDL
hotdata tables list
hotdata tables list --schema <schema> --table <table>   # narrow the workspace-wide listing
hotdata databases list
```

Use `hotdata tables list` for discovery; do not query `information_schema` for that.

---

## Cross-cutting

- **Workspace:** Active workspace or `--workspace-id`. **`hotdata queries`** uses the active workspace only (no `--workspace-id`).
- **Jobs:** `hotdata jobs list` / `jobs <id>` for async refreshes and index builds.
- **Discovery:** `hotdata tables list` — not `query` on `information_schema`.
