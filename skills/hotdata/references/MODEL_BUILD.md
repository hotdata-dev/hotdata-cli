# Building a workspace data model (advanced)

Optional **deep pass** for a single authoritative markdown document stored as **`context:DATAMODEL`** (workspace **context API**). For a short checklist only, use the **Model** section in [WORKFLOWS.md](WORKFLOWS.md) and [DATA_MODEL.template.md](DATA_MODEL.template.md).

**Notation:** **`context:DATAMODEL`** is the live server document; **not** the same phrase as “building a data model” for a one-off analysis. **CLI** uses the bare stem: `hotdata context show DATAMODEL`.

**Output:** After **`hotdata context list`** confirms `DATAMODEL` exists, read **context:DATAMODEL** with `hotdata context show DATAMODEL`; edit `./DATAMODEL.md` in the **project directory** where you run `hotdata`, then **`hotdata context push DATAMODEL`**. Do not use `docs/`, `DATA_MODEL.md`, or other repo-only paths as the system of record. Never store workspace-specific model text inside agent skill folders.

---

## 1. Discover connections

```bash
hotdata connections list
```

For each connection, record `id`, `name`, and `source_type`.

---

## 2. Enumerate tables, columns, and datasets

If the catalog may be **stale** (recent DDL, new tables missing), run **`hotdata connections refresh <connection_id>`** for affected connections **before** relying on `tables list`.

**Per connection:**

```bash
hotdata tables list --connection-id <connection_id>
```

**Uploaded datasets:**

```bash
hotdata datasets list
hotdata datasets <dataset_id>
```

Capture schema for each dataset (columns, types) from the detail view.

You can also refresh after enumeration if you discover drift:

```bash
hotdata connections refresh <connection_id>
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
hotdata query "SELECT * FROM <connection>.<schema>.<table> LIMIT 5"
```

---

## 4. Infer relationships

For each table, capture where reasonable:

1. **Grain** — One row = one `…` (required per table; if unknown, say unknown).
2. **Primary keys** — `id`, `<entity>_id`, or composite patterns from names + types.
3. **Foreign keys** — `_id` / `_fk` / name matches to other tables; confirm with connector docs when possible.
4. **Parent–child** — Flattened API/JSON tables (often nested names) and dlt parent keys.
5. **Cross-connection** — Same logical entity in two connections (keys, type mismatches, caveats).

For **small** schemas (e.g. ≤5 tables in a domain), a short **ASCII diagram** helps. For larger ones, group by domain in prose (e.g. billing, identity, product).

---

## 5. Search and index awareness

Inventory indexes on connection tables (whole workspace or filtered):

```bash
hotdata indexes list [-w <workspace_id>]
hotdata indexes list -c <connection_id> [--schema <schema>] [--table <table>] [-w <workspace_id>]
```

Per table when you only need one:

```bash
hotdata indexes list -c <connection_id> --schema <schema> --table <table> [-w <workspace_id>]
```

For dataset-backed indexes: `hotdata indexes list --dataset-id <dataset_id>` (not merged into the workspace-wide connection-table list).

Note:

- **Vector**-friendly columns (embeddings) vs **BM25**-friendly text (`title`, `body`, `description`, …).
- **Time** columns — event grain vs slowly changing dimensions.
- **Facts vs dimensions** — for analytics-oriented workspaces.

When suggesting a new index, use the same connection/schema/table/column names as in `tables list` and the main skill’s `indexes create` examples.

---

## 6. Document structure

This Markdown body is what you store as **context:DATAMODEL** (`hotdata context push DATAMODEL`). Start from [DATA_MODEL.template.md](DATA_MODEL.template.md) and extend as needed:

- **Overview** — Domains and what the workspace is for.
- **Per connection** — Optional subsection per source; for **deep** models, **repeat** one block per `connection.schema.table` (grain, column table with name/type/nullable/PK-FK/notes, relationships, queryability, caveats)—the template’s single `####` heading is a pattern to copy for each table.
- **Datasets** — Same treatment as connection tables where relevant.
- **Cross-connection joins** — Keys, semantics, type caveats.
- **Search / index summary** — Table, column, index status, intended use.

If the workspace has **many** tables (e.g. 50+), add a **table of contents** after the overview (connection → table counts).

---

## Error handling

- If a CLI command fails, record the error in the doc and **continue** when possible.
- Unreachable connections or empty table lists: note in the connections table (e.g. unreachable / no tables).
- Do not abort the whole model for one bad connection.

---

## Rules (keep quality high)

- Every table gets an explicit **grain** (or “unknown”).
- Prefer **documented** connector semantics over guesswork; **link** external docs when you use them.
- Flag **test/dev** tables (`test`, `tmp`, `dev`, `staging` in names) as non-production when applicable.
- Note **Utf8-stored numbers** and cast requirements where relevant.
- Do not leave column **Notes** empty when domain knowledge or docs apply; “—” is weak unless the column is opaque/internal.
- Align table names with **`hotdata tables list`** output (`connection.schema.table`).
