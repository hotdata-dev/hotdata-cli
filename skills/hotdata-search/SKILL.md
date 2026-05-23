---
name: hotdata-search
description: Use this skill when the user wants full-text search, BM25 keyword search, vector similarity search, semantic search, embeddings, or retrieval indexes in Hotdata. Activate for "hotdata search", "BM25", "full-text", "vector search", "semantic search", "similarity", "embedding", "embedding provider", "create an index" (bm25 or vector), "list indexes" for search, or SQL using bm25_search or vector_distance. Do not load for general SQL analytics (aggregations, GROUP BY) or geospatial work — use hotdata-analytics or hotdata-geospatial instead. Requires the core hotdata skill for auth and workspace basics.
version: 0.2.9
---

# Hotdata Search Skill

Retrieval workloads in Hotdata: **BM25 full-text**, **vector similarity**, and the **indexes** and **embedding providers** that power them.

**Prerequisites:** Authenticate and select a workspace (see the **`hotdata`** skill). Use fully qualified table names: `<connection>.<schema>.<table>`.

**Related skills:** **`hotdata-analytics`** (OLAP SQL, query history, materialized chains), **`hotdata-geospatial`** (PostGIS-style functions).

---

## Search CLI

`--type` is **required**: `bm25` or `vector`. Both run server-side.

```bash
# BM25 (requires a BM25 index on the column)
hotdata search "<query>" --type bm25 --table <connection.schema.table> --column <column> \
  [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]

# Vector (requires a vector index; server auto-embeds the query text)
hotdata search "<query>" --type vector --table <connection.schema.table> --column <source_text_column> \
  [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]
```

| Type | Behavior |
|------|----------|
| **`bm25`** | Server generates `bm25_search(table, col, 'text')`. Results sort by score (descending). |
| **`vector`** | Pass plain-text query; name the **source text column** (e.g. `title`). Server embeds using the same provider/metric/dimensions as the index. SQL uses `vector_distance(col, 'text')`. Results sort by distance (ascending). |

- **No vector index, or custom embedding model?** Use raw SQL via `hotdata query` (e.g. `cosine_distance(col, [<vec>])`). The removed `--model` / stdin-vector paths hardcoded `l2_distance` and are not supported.
- **Before search:** create the right index (`indexes create --type bm25` or `--type vector`). See [references/INDEXES.md](references/INDEXES.md).
- Default `--limit` is 10.

---

## Indexes (BM25 and vector)

Indexes attach to a **connection table** (`--connection-id` + `--schema` + `--table`) or a **dataset** (`--dataset-id`). Scopes are mutually exclusive for create/delete.

```bash
# List — workspace scan on connection tables (filter with -c / --schema / --table)
hotdata indexes list [--connection-id <id>] [--schema <schema>] [--table <table>] [--workspace-id <ws>] [--output table|json|yaml]
hotdata indexes list --dataset-id <dataset_id> [--workspace-id <ws>] [--output table|json|yaml]

# Connection table
hotdata indexes create --connection-id <id> --schema <schema> --table <table> \
  --name <name> --columns <cols> --type bm25|vector \
  [--metric l2|cosine|dot] [--async] \
  [--embedding-provider-id <id>] [--dimensions <n>] [--output-column <name>] [--description <text>]
hotdata indexes delete --connection-id <id> --schema <schema> --table <table> --name <name>

# Dataset
hotdata indexes create --dataset-id <dataset_id> --name <name> --columns <cols> --type bm25|vector ...
hotdata indexes delete --dataset-id <dataset_id> --name <name>
```

- **`--type` is required** on create: `bm25` (one text column) or `vector` (exactly one column; often embeddings or auto-embedded text).
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
  [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>]
hotdata embedding-providers update <id> [--name <name>] [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata embedding-providers delete <id> [--workspace-id <workspace_id>]
```

- System providers (e.g. `sys_emb_openai`) are pre-configured; use `list` for IDs to pass to `--embedding-provider-id`.
- `--provider-api-key` is the **embedding service** key (not Hotdata `--api-key`). `--secret-name` references an existing secret.

---

## Quick workflow

1. `hotdata tables list --connection-id <id>` — confirm column types.
2. `hotdata indexes list` — avoid duplicate indexes.
3. `hotdata indexes create ... --type bm25|vector` (add `--async` if large).
4. `hotdata search "..." --type bm25|vector --table ... --column ...`
5. Record what exists in **context:DATAMODEL** (core skill) when the workspace should remember index choices.
