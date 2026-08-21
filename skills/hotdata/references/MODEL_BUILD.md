# Building a database data model (advanced)

Optional **deep pass** for a single authoritative markdown document stored as **`context:DATAMODEL`** (database-scoped **context API** — the active database). For a short checklist only, use the **Model** section in [WORKFLOWS.md](WORKFLOWS.md) and [DATA_MODEL.template.md](DATA_MODEL.template.md).

**Notation:** **`context:DATAMODEL`** is the live server document; **not** the same phrase as “building a data model” for a one-off analysis. **CLI** uses the bare stem: `hotdata databases context show DATAMODEL`.

**Output:** After **`hotdata databases context list`** confirms `DATAMODEL` exists, read **context:DATAMODEL** with `hotdata databases context show DATAMODEL`; edit `./DATAMODEL.md` in the **project directory** where you run `hotdata`, then **`hotdata databases context push DATAMODEL`**. Do not use `docs/`, `DATA_MODEL.md`, or other repo-only paths as the system of record. Never store database-specific model text inside agent skill folders.

---

## 1. Discover catalogs and tables

List the catalogs you can query — managed databases you own and any attached catalogs — and the tables they expose:

```bash
hotdata databases list           # managed databases (catalogs you own)
hotdata databases tables list    # every workspace table, as <catalog>.<schema>.<table>
```

For each catalog, record its name and the tables it exposes. (Pulling *new* external data into a managed database is a separate step — see the `ingest sources` and `ingest` commands in the core skill.)

---

## 2. Enumerate tables and columns

A datasource's schema is discovered when it is added. If the source schema may have changed (recent DDL, new tables), re-check the currently discovered tables/columns with **`hotdata ingest sources show <datasource_id>`** **before** relying on `databases tables list`.

**Workspace tables** (list all, narrow with filters):

```bash
hotdata databases tables list --schema <schema> --table <table>
```

**Managed databases:**

```bash
hotdata databases list
hotdata databases tables list
```

Capture schema for each managed-database table (columns, types) from the table listing.

You can also re-check a datasource's discovered schema after enumeration if you suspect drift:

```bash
hotdata ingest sources show <datasource_id>
```

---

## 3. Enrich beyond column names (optional but valuable)

Use **connector and tooling docs** when `source_type` (or table shapes) match:

- **Vendor / ELT docs** — Your loader or integration vendor’s published schemas for canonical tables, PKs/FKs, and field semantics (link what you use so a human can verify).
- **dlt** — [verified sources](https://dlthub.com/docs/dlt-ecosystem/verified-sources) for normalized layouts.
- **dlt-loaded data** — If you see `_dlt_id`, `_dlt_load_id`, `_dlt_parent_id`: treat as pipeline metadata; `_dlt_parent_id` often links flattened child rows to parents when no explicit FK exists. Exclude these from **grain** statements unless the question is specifically about loads.
- **Vectors** — Columns typed as lists of floats (e.g. embedding columns) are candidates for vector search; note them.
- **Well-known SaaS shapes** — Apply general patterns (e.g. Stripe charges/customers, HubSpot contacts/deals) only when naming and structure fit; **link** the doc you used so a human can verify.

Do **not** invent facts: if **context:DATAMODEL** (or needed facts) is missing, say so and suggest a small sample query:

```bash
hotdata query "SELECT * FROM <catalog>.<schema>.<table> LIMIT 5"
```

---

## 4. Infer relationships

For each table, capture where reasonable:

1. **Grain** — One row = one `…` (required per table; if unknown, say unknown).
2. **Primary keys** — `id`, `<entity>_id`, or composite patterns from names + types.
3. **Foreign keys** — `_id` / `_fk` / name matches to other tables; confirm with connector docs when possible.
4. **Parent–child** — Flattened API/JSON tables (often nested names) and dlt parent keys.
5. **Cross-catalog** — Same logical entity in two catalogs (keys, type mismatches, caveats).

For **small** schemas (e.g. ≤5 tables in a domain), a short **ASCII diagram** helps. For larger ones, group by domain in prose (e.g. billing, identity, product).

---

## 5. Search and index awareness

Inventory indexes (whole workspace or filtered):

```bash
hotdata search list [-w <workspace_id>]
hotdata search list [--schema <schema>] [--table <table>] [-w <workspace_id>]
```

Per table when you only need one:

```bash
hotdata search list --schema <schema> --table <table> [-w <workspace_id>]
```

Managed-database indexes are included in the no-flag whole-workspace `search list` (shown under the internal `__db_<id>.<schema>.<table>` label); narrow to one with `--schema` / `--table` as above.

Note:

- **Vector**-friendly columns (embeddings) vs **BM25**-friendly text (`title`, `body`, `description`, …).
- **Time** columns — event grain vs slowly changing dimensions.
- **Facts vs dimensions** — for analytics-oriented workspaces.

When suggesting a new index, use the same catalog/schema/table/column names as in `databases tables list` and **`hotdata-search`** / **`hotdata-analytics`** `search create` examples (text/vector vs sorted).

---

## 6. Document structure

This Markdown body is what you store as **context:DATAMODEL** (`hotdata databases context push DATAMODEL`). Start from [DATA_MODEL.template.md](DATA_MODEL.template.md) and extend as needed:

- **Overview** — Domains and what the workspace is for.
- **Per catalog** — Optional subsection per source; for **deep** models, **repeat** one block per `catalog.schema.table` (grain, column table with name/type/nullable/PK-FK/notes, relationships, queryability, caveats)—the template’s single `####` heading is a pattern to copy for each table.
- **Managed databases** — Same treatment as catalog tables where relevant.
- **Cross-catalog joins** — Keys, semantics, type caveats.
- **Search / index summary** — Table, column, index status, intended use.

If the workspace has **many** tables (e.g. 50+), add a **table of contents** after the overview (catalog → table counts).

---

## Error handling

- If a CLI command fails, record the error in the doc and **continue** when possible.
- Unreachable catalogs or empty table lists: note in the catalogs table (e.g. unreachable / no tables).
- Do not abort the whole model for one bad catalog.

---

## Rules (keep quality high)

- Every table gets an explicit **grain** (or “unknown”).
- Prefer **documented** connector semantics over guesswork; **link** external docs when you use them.
- Flag **test/dev** tables (`test`, `tmp`, `dev`, `staging` in names) as non-production when applicable.
- Note **Utf8-stored numbers** and cast requirements where relevant.
- Do not leave column **Notes** empty when domain knowledge or docs apply; “—” is weak unless the column is opaque/internal.
- Align table names with **`hotdata databases tables list`** output (`catalog.schema.table`).
