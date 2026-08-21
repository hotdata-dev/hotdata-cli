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

With no filters, this is a whole-workspace scan that **includes managed-database indexes** (shown under the internal `__db_<id>.<schema>.<table>` label). Skip duplicates (same table, column, and purpose).

## 3. Create indexes

For managed databases (`--from` catalog alias — auto-selects the active database catalog):

```bash
hotdata search create <table>_body --type text \
  --from <alias>.<schema>.<table> --column body

hotdata search create <table>_embedding_vec --type vector \
  --from <alias>.<schema>.<table> --column embedding --metric cosine
```

Indexes are created on **managed databases** only. To index a table that lives in an external catalog, attach the catalog to a managed database first (`hotdata databases attach <catalog>`), then create the index with the managed database's catalog in `--from` — a bare connection/catalog is rejected.

Large builds: `--async`, then `hotdata jobs list` / `hotdata jobs <job_id>`.

## 4. Verify

Re-run `hotdata search "..." --index <name>` or representative SQL. Update **context:DATAMODEL → Search & index summary** via `hotdata databases context push DATAMODEL` (core skill).

## Guardrails

- Prefer evidence (repeated search workloads) over speculative indexes.
- Get approval before production `search create` when cost/impact is uncertain.
- Align catalog/schema/table with `hotdata databases tables list` output.
