---
name: hotdata-search
description: Use this skill when the user wants full-text search, BM25 keyword search, vector similarity search, semantic search, embeddings, or retrieval indexes in Hotdata. Activate for "hotdata search", "BM25", "full-text", "vector search", "semantic search", "similarity", "embedding", "embedding provider", "create an index" (bm25 or vector), "list indexes" for search, or SQL using bm25_search or vector_distance. Do not load for general SQL analytics (aggregations, GROUP BY) or geospatial work — use hotdata-analytics or hotdata-geospatial instead. Requires the core hotdata skill for auth and workspace basics.
version: 0.15.0
---

# Hotdata Search Skill

Retrieval workloads in Hotdata: **BM25 full-text**, **vector similarity**, and the **indexes** and **embedding providers** that power them.

**Prerequisites:** Authenticate, set a workspace, and set an active database (`hotdata databases set <id>`) — see the **`hotdata`** skill. Use fully qualified table names: `<catalog>.<schema>.<table>`.

**Related skills:** **`hotdata-analytics`** (OLAP SQL, query history, materialized chains), **`hotdata-geospatial`** (PostGIS-style functions).

---

## Search CLI

Both run server-side. `--type` and `--column` are **optional** when the table has exactly one search index — they are inferred automatically. Specify them when multiple indexes exist.

```bash
# BM25 (requires a BM25 index on the column)
hotdata search "<query>" --table <catalog.schema.table> [--type bm25] [--column <column>] \
  [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]

# Vector (requires a vector index; server auto-embeds the query text)
hotdata search "<query>" --table <catalog.schema.table> [--type vector] [--column <source_text_column>] \
  [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]

# With active database: schema.table is enough (catalog resolved from active DB)
hotdata search "<query>" --table <schema.table> [--type bm25|vector] [--column <col>]
```

| Type | Behavior |
|------|----------|
| **`bm25`** | Server generates `bm25_search(table, col, 'text')`. Results sort by score (descending). |
| **`vector`** | Pass plain-text query; name the **source text column** (e.g. `title`). Server embeds using the same provider/metric/dimensions as the index. SQL uses `vector_distance(col, 'text')`. Results sort by distance (ascending). |

- **Inference:** when `--type` or `--column` are omitted, the CLI fetches the table's indexes and selects the only BM25/vector index. If multiple exist, you must specify both flags.
- **Custom embedding model, raw query vector, or no vector index?** Use `hotdata query` directly (e.g. `cosine_distance(col, [<vec>])`) — `search` only auto-embeds the query text via the index's own provider.
- **Before search:** create the right index (`indexes create --type bm25` or `--type vector`). See [references/INDEXES.md](references/INDEXES.md).
- Default `--limit` is 10.
- **Active database:** with `hotdata databases set <db>`, you can pass `schema.table` directly (e.g. `--table public.articles`) — the active database's catalog is resolved automatically. Or use the full `catalog.schema.table` form. Do **not** use the internal `__db_<id>` label or raw connection ID prefix — `bm25_search`/`vector_distance` resolve a catalog attached to the active database, so an `__db_…` or `conn…` prefix errors with *catalog … is not attached*.

---

## Indexes (BM25 and vector)

Create attaches to a table via its `--catalog` alias (a managed-database catalog or a connection name). `list` narrows to the **active database** when one is set; without one it scans the whole workspace. Filter further with `--schema` / `--table`. `delete` **requires all of** `--connection-id` (short `-c`) + `--schema` + `--table` + `--name`.

```bash
# List — active-database scope when a DB is set, else whole-workspace scan
hotdata indexes list [--schema <schema>] [--table <table>] [--workspace-id <ws>] [--output table|json|yaml]

# Create — by catalog alias (resolves a managed-database catalog or a connection name)
hotdata indexes create --catalog <alias> --schema <schema> --table <table> \
  --column <col> --type bm25|vector \
  [--name <name>] [--metric l2|cosine|dot] [--async] \
  [--embedding-provider-id <id>] [--dimensions <n>] [--output-column <name>] [--description <text>]

# Delete — requires --connection-id + --schema + --table + --name
hotdata indexes delete --connection-id <id> --schema <schema> --table <table> --name <name>
```

- **`--type` is required** on create: `bm25` (one or more text columns, comma-separated in `--column`) or `vector` (exactly one column; often embeddings or auto-embedded text). (`sorted` is also a valid `--type`, covered in **`hotdata-analytics`**.)
- **`sorted`** indexes (range/equality for OLAP filters) are documented in **`hotdata-analytics`** — this skill focuses on retrieval types.
- **`--async`:** poll with `hotdata jobs <job_id>` (see **`hotdata`** skill **Jobs**).
- **Auto-embedding:** `--type vector` on a **text** column generates embeddings server-side. Optional `--embedding-provider-id`; default output column `{column}_embedding` (override with `--output-column`).

Full workflow (gather workload → compare existing → create → verify): [references/INDEXES.md](references/INDEXES.md).

---

## Embedding providers

```bash
hotdata embedding-providers list [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata embedding-providers get <id> [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata embedding-providers create --name <name> --provider-type service|local \
  [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata embedding-providers update <id> [--name <name>] [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata embedding-providers delete <id> [--workspace-id <workspace_id>]
```

- System providers (e.g. `sys_emb_openai`) are pre-configured; use `list` for IDs to pass to `--embedding-provider-id`.
- `--provider-api-key` is the **embedding service** key (not Hotdata `--api-key`). `--secret-name` references an existing secret.

---

## Quick workflow

1. `hotdata databases set <id>` — set an active database, then `hotdata tables list` to confirm column types.
2. `hotdata indexes list` — avoid duplicate indexes (scoped to active DB automatically).
3. `hotdata indexes create --catalog <alias> --table <table> --column <col> --type bm25|vector` (add `--async` if large).
4. `hotdata search "..." --table <schema.table>` — `--type` and `--column` are inferred when there is one search index.
5. Record what exists in **context:DATAMODEL** (core skill) when the workspace should remember index choices.
