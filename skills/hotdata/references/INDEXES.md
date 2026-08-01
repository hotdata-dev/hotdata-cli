# Index workflow (BM25 and vector)

**Goal:** Find full-text and vector access patterns that lack indexes, then create **bm25** or **vector** indexes when the benefit is clear.

## 1. Gather workload and schema

- **Query-run history** — recurring predicates or search-style SQL (`bm25_search`, `vector_distance`, or planned `hotdata search`):

  ```bash
  hotdata queries list
  hotdata queries <query_run_id>
  ```

- **Columns** — confirm types:

  ```bash
  hotdata tables list --connection-id <connection_id>
  ```

High-cardinality **text** (`title`, `body`, …) → **bm25**. **Embedding** / float list columns → **vector** (+ `--metric`).

## 2. Compare to existing indexes

```bash
hotdata indexes list [--connection-id <id>] [--schema <schema>] [--table <table>]
```

With no `--connection-id`, this is a whole-workspace scan that **includes managed-database indexes** (shown under the internal `__db_<id>.<schema>.<table>` label). Skip duplicates (same table, column, and purpose).

## 3. Create indexes

For managed databases (catalog alias — auto-selects the active database connection):

```bash
hotdata indexes create --catalog <alias> --schema <schema> --table <table> \
  --column body --type bm25

hotdata indexes create --catalog <alias> --schema <schema> --table <table> \
  --column embedding --type vector --metric cosine
```

For a regular connection, pass its name or ID to `--catalog`:

```bash
hotdata indexes create --catalog <connection-name-or-id> --schema <schema> --table <table> \
  --name idx_posts_body_bm25 --column body --type bm25

hotdata indexes create --catalog <connection-name-or-id> --schema <schema> --table <table> \
  --name idx_chunks_embedding --column embedding --type vector --metric cosine
```

Large builds: `--async`, then `hotdata jobs list` / `hotdata jobs <job_id>`.

## 4. Verify

Re-run `hotdata search` or representative SQL. Update **context:DATAMODEL → Search & index summary** via `hotdata context push DATAMODEL` (core skill).

## Guardrails

- Prefer evidence (repeated search workloads) over speculative indexes.
- Get approval before production `indexes create` when cost/impact is uncertain.
- Align connection/schema/table with `hotdata tables list` output.
