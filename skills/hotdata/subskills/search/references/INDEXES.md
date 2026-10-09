# Index workflow (BM25 and vector)

**Goal:** Find full-text and vector access patterns that lack indexes, then create **bm25** or **vector** indexes when the benefit is clear.

## 1. Gather workload and schema

- **Query-run history** — recurring predicates or search-style SQL (`bm25_search`, `vector_distance`, or planned `hotdata search`):

  ```bash
  hotdata databases queries list
  hotdata databases queries <query_run_id>
  ```

- **Columns** — confirm types:

  ```bash
  hotdata databases tables list --schema <schema> --table <table>
  ```

High-cardinality **text** (`title`, `body`, …) → **bm25**. **Embedding** / float list columns → **vector** (+ `--metric`).

## 2. Compare to existing indexes

```bash
hotdata search list
```

With no filters, this is a whole-workspace scan that **includes instant-database indexes** (shown as `<catalog>.<schema>.<table>`, the database's own catalog — the name you query and pass to `--from`). Skip duplicates (same table, column, and purpose).

## 3. Create indexes

For instant databases (`--from` catalog alias — auto-selects the active database catalog):

```bash
hotdata search create <table>_body --type text \
  --from <alias>.<schema>.<table> --column body

hotdata search create <table>_embedding_vec --type vector \
  --from <alias>.<schema>.<table> --column embedding --metric cosine

# Table too large for an in-memory hnsw index: cluster it with ivf instead
hotdata search create <table>_embedding_ivf --type vector \
  --from <alias>.<schema>.<table> --column embedding --metric cosine \
  --algorithm ivf [--nlist <n>] [--probe-fraction <f>] [--vector-precision int8|float32]
```

Vector indexes default to `--algorithm hnsw`. Choose `ivf` when the table is too large to serve from memory; it works only on columns that already hold vectors, and supports `l2`/`cosine` (not `dot`). See the search skill for `--nlist`, `--probe-fraction`, and `--vector-precision`.

Indexes are created on **instant databases** only, and on the database's own tables: an index is a write, and an attached database is read-only. To index a table that lives in another database, build the index there, or load a copy into this one — attaching it does not make it indexable here.

Large builds: `--async`, then `hotdata jobs list` / `hotdata jobs <job_id>`.

## 4. Verify

Re-run `hotdata search "..." --index <name>` or representative SQL. Update **context:DATAMODEL → Search & index summary** via `hotdata databases context push DATAMODEL` (core skill).

## Guardrails

- Prefer evidence (repeated search workloads) over speculative indexes.
- Get approval before production `search create` when cost/impact is uncertain.
- Align catalog/schema/table with `hotdata databases tables list` output.
