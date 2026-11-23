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

For instant databases (`--from` catalog alias — auto-selects the current database catalog):

```bash
hotdata search create <table>_body --type text \
  --from <alias>.<schema>.<table> --column body

hotdata search create <table>_embedding_vec --type vector \
  --from <alias>.<schema>.<table> --column embedding --metric cosine

# Table too large for an in-memory hnsw index: cluster it with ivf instead
hotdata search create <table>_embedding_ivf --type vector \
  --from <alias>.<schema>.<table> --column embedding --metric cosine \
  --algorithm ivf [--nlist <n>] [--probe-fraction <f>] [--vector-precision int8|float32]

# ivf over a text column: the provider generates the embeddings
hotdata search create <table>_body_ivf --type vector \
  --from <alias>.<schema>.<table> --column body --provider <provider-id> \
  --metric cosine --algorithm ivf
```

Vector indexes default to `--algorithm hnsw`. Choose `ivf` when the table is too large to serve from memory; it works on columns that already hold vectors and on text columns with `--provider`, and supports `l2`/`cosine` (not `dot`). An `ivf` index with a provider is searched with `hotdata search "<text>" --index <name>`; one over a vector column is queried with SQL and a query vector. See the search skill for `--nlist`, `--probe-fraction`, and `--vector-precision`.

Indexes are created on **instant databases** only, and on the database's own tables: an index is a write, and an attached database is read-only. To index a table that lives in another database, build the index there, or load a copy into this one — attaching it does not make it indexable here.

Large builds: `--async`, then `hotdata jobs list` / `hotdata jobs <job_id>`.

### Hybrid search (BM25 + vector)

An auto-embedding vector index (`--type vector` on a text column, with or without an explicit `--provider`) **must be the only index on its table**. The server refuses it when the table already has a BM25 or sorted index, and refuses any other index once it exists, with `Embedding-backed vector indexes cannot coexist with other indexes on the same table`. Creating the indexes in the other order does not help. A vector index over a column that already holds vectors is not affected and can share a table with BM25.

Two patterns give BM25 and vector retrieval over the same rows:

1. **Companion table** (auto-embedding). Keep the BM25 index on the original table. Load a second table in the same instant database with the key and the text column, and put only the auto-embedding vector index on it:

   ```bash
   hotdata search create <table>_body_bm25 --type text \
     --from <alias>.<schema>.<table> --column body
   # <table>_vec holds (id, body), copied from <table>
   hotdata search create <table>_vec_body --type vector \
     --from <alias>.<schema>.<table>_vec --column body --provider <provider-id>
   ```

   Search each index (`hotdata search "<text>" --index <name>`) and fuse the two ranked lists on `id`, e.g. reciprocal rank fusion (`score = Σ 1 / (60 + rank)`). Reload the companion table when the source text changes.

2. **Precomputed embeddings** (one table). Compute embeddings outside Hotdata and load them into a float-list column (e.g. `embedding`). Create the vector index on that column without `--provider`, and the BM25 index on the text column; both coexist. Query the vector side with SQL and a query vector embedded by the same model.

## 4. Verify

Re-run `hotdata search "..." --index <name>` or representative SQL. Update **context:DATAMODEL → Search & index summary** via `hotdata databases context push DATAMODEL` (core skill).

## Guardrails

- Prefer evidence (repeated search workloads) over speculative indexes.
- Get approval before production `search create` when cost/impact is uncertain.
- Align catalog/schema/table with `hotdata databases tables list` output.
