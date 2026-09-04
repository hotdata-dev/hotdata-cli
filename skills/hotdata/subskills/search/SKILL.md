---
name: hotdata-search
description: Use this skill when the user wants full-text search, BM25 keyword search, vector similarity search, semantic search, embeddings, or retrieval indexes in Hotdata. Activate for "hotdata search", "BM25", "full-text", "vector search", "semantic search", "similarity", "embedding", "embedding provider", "create an index" (bm25 or vector), "list indexes" for search, or SQL using bm25_search or vector_distance. Do not load for general SQL analytics (aggregations, GROUP BY) or geospatial work — use hotdata-analytics or hotdata-geospatial instead. Requires the core hotdata skill for auth and workspace basics.
version: 0.31.0
---

# Hotdata Search Skill

Retrieval workloads in Hotdata: **BM25 full-text**, **vector similarity**, and the **indexes** and **embedding providers** that power them.

**Prerequisites:** Authenticate, set a workspace, and set an active database (`hotdata databases use <id>`) — see the **`hotdata`** skill. Use fully qualified table names: `<catalog>.<schema>.<table>`.

**Related sub-skills** (bundled alongside this one — `Read` on demand): **`hotdata-analytics`** ([`../analytics/SKILL.md`](../analytics/SKILL.md) — OLAP SQL, query history, materialized chains), **`hotdata-geospatial`** ([`../geospatial/SKILL.md`](../geospatial/SKILL.md) — PostGIS-style functions).

---

## Search CLI

Both run server-side. The search action addresses an index **by name** (`--index`, alias `--in`); the index's type, column, and provider come from the index itself.

```bash
# BM25 / text (requires a text index; address it by name)
hotdata search "<query>" --index <name> \
  [--database <db-id>] [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]

# Vector (requires a vector index; server auto-embeds the query text)
hotdata search "<query>" --index <name> \
  [--database <db-id>] [--select <columns>] [--limit <n>] [--workspace-id <workspace_id>] [--output table|json|csv]

# --in is an accepted alias for --index
hotdata search "<query>" --in <name>
```

| Type | Behavior |
|------|----------|
| **`bm25`** | Server generates `bm25_search(table, col, 'text')`. Results sort by score (descending). |
| **`vector`** | Pass plain-text query; name the **source text column** (e.g. `title`). Server embeds using the same provider/metric/dimensions as the index. SQL uses `vector_distance(col, 'text')`. Results sort by distance (ascending). |

- **Index name:** the index carries its own type, column, and provider — you only name the index. Use `search list` to see available index names.
- **Custom embedding model, raw query vector, or no vector index?** Use `hotdata query` directly (e.g. `cosine_distance(col, [<vec>])`) — `search` only auto-embeds the query text via the index's own provider.
- **Before search:** create the right index (`search create <name> --type text` or `--type vector`). See [references/INDEXES.md](references/INDEXES.md).
- Default `--limit` is 10.
- **Database:** the search commands resolve the index in the **active** database (`hotdata databases use <id>`). Pass `-d/--database <id>` to target a different database explicitly — it is required when no active database is set. The same `-d/--database` works on `search show` and `search remove`.
- **Active database:** with `hotdata databases use <db>`, an index created with a `schema.table` `--from` resolves the active database's catalog automatically. Or create it with the full `catalog.schema.table` form. Do **not** use the internal `__db_<id>` label or raw catalog ID prefix — `bm25_search`/`vector_distance` resolve a catalog attached to the active database, so an `__db_…` or `conn…` prefix errors with *catalog … is not attached*.

---

## Indexes (text and vector)

Indexes are an **instant-database** concept. Create names the index (positional) and attaches to a table on an instant database via `--from` — `catalog.schema.table` (the instant database's catalog), or `schema.table` with an active database set. A plain connection catalog is rejected. `list` narrows to the **active database** when one is set; without one it scans the whole workspace. `show`/`remove` resolve the index by name in the active database (or `--database <id>`).

```bash
# List — active-database scope when a DB is set, else whole-workspace scan
hotdata search list [--workspace-id <ws>] [--output table|json|yaml]

# Create — index name is positional; --from is an instant database's table
hotdata search create <name> --type text|vector --from <catalog.schema.table> \
  --column <col> \
  [--metric l2|cosine|dot] [--async] \
  [--provider <id>] [--dimensions <n>] [--output-column <name>] [--description <text>]

# Show — by index name, in the active database (or -d/--database <id>)
hotdata search show <name> [-d <db-id>] [--output table|json|yaml]

# Remove — by index name, in the active database (or -d/--database <id>)
hotdata search remove <name> [-d <db-id>]
```

- **`--type` is required** on create: `text` (BM25; one or more text columns, comma-separated in `--column`) or `vector` (exactly one column; often embeddings or auto-embedded text). (`sorted` is also a valid `--type`, covered in **`hotdata-analytics`** — [`../analytics/SKILL.md`](../analytics/SKILL.md).)
- **`sorted`** indexes (range/equality for OLAP filters) are documented in **`hotdata-analytics`** ([`../analytics/SKILL.md`](../analytics/SKILL.md)) — this skill focuses on retrieval types.
- **`--async`:** poll with `hotdata jobs <job_id>` (see **`hotdata`** skill **Jobs**).
- **Auto-embedding:** `--type vector` on a **text** column generates embeddings server-side. Optional `--provider`; default output column `{column}_embedding` (override with `--output-column`).

Full workflow (gather workload → compare existing → create → verify): [references/INDEXES.md](references/INDEXES.md).

---

## Embedding providers

```bash
hotdata search embeddings list [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata search embeddings show <id> [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata search embeddings add --name <name> --provider-type service|local \
  [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata search embeddings update <id> [--name <name>] [--config '<json>'] [--provider-api-key <key> | --secret-name <name>] [--workspace-id <workspace_id>] [--output table|json|yaml]
hotdata search embeddings remove <id> [--workspace-id <workspace_id>]
```

- System providers (e.g. `sys_emb_openai`) are pre-configured; use `list` for IDs to pass to `--provider`.
- `--provider-api-key` is the **embedding service** key (not Hotdata `--api-key`). `--secret-name` references an existing secret.

---

## Quick workflow

1. `hotdata databases use <id>` — set an active database, then `hotdata databases tables list` to confirm column types.
2. `hotdata search list` — avoid duplicate indexes (scoped to active DB automatically).
3. `hotdata search create <name> --type text|vector --from <catalog.schema.table> --column <col>` (add `--async` if large).
4. `hotdata search "..." --index <name>` — address the index you created by name.
5. Record what exists in **context:DATAMODEL** (core skill) when the workspace should remember index choices.
