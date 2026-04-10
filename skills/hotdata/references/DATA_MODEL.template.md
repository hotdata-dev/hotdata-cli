# Data model — `<project name>`

> Copy this file to your **project** directory (e.g. `./DATA_MODEL.md`, `./data_model.md`, or `./docs/DATA_MODEL.md`).  
> Do not commit workspace-specific content into agent skill folders.  
> For a **full** build (per-table detail, connector enrichment, index summary), follow [MODEL_BUILD.md](MODEL_BUILD.md) from the installed skill’s `references/` (or this repo’s `skills/hotdata/references/`). Relative links to `MODEL_BUILD.md` below work only while this file lives next to those references; in your project, open that path separately if the link 404s.

**Workspace (Hotdata):** `<workspace name or id>`  
**Last catalog refresh:** `<YYYY-MM-DD>`

## Overview

What data exists, which business domains it covers, and who owns this document.  
_(Large workspaces: add a **table of contents** here—per connection, table counts.)_

## Purpose

Short description of what this workspace is for and how the model should be used for queries.

## Connections & sources

| Connection ID | Name | Type | Role / domain |
|---------------|------|------|---------------|
| | | | |

### Per-table detail (optional — use for deep models)

_Use for important tables only, or expand all via [MODEL_BUILD.md](MODEL_BUILD.md). **Duplicate** this whole block (from the heading through the horizontal rule) for each table._

#### `<connection>.<schema>.<table>`

**Grain:** one row = one `…`  
**Description:**  

| Column | Type | Nullable | PK/FK | Notes |
|--------|------|----------|-------|-------|

**Relationships:** (PK, FKs, parent–child)  
**Queryability:** (filters, joins, caveats)

---

## Entities and grain (summary view)

For each business entity:

- **Entity:**  
- **Grain:** one row per …  
- **Primary tables:** `connection.schema.table`  
- **Key columns:**  

## Cross-connection joins

Document safe join paths and caveats (fan-out, timing, different refresh cadence, type mismatches).

## Search & index summary (optional)

| Table | Column | Kind (vector / text / …) | Index status | Notes |
|-------|--------|--------------------------|--------------|-------|
| | | | | |

_Use `hotdata indexes list -c <connection_id> --schema <schema> --table <table>` per table as needed._

## Datasets (uploaded)

Catalog from `hotdata datasets list` / `hotdata datasets <id>`:

| Label | Table name (`datasets.main.…`) | Grain | Notes |
|-------|-------------------------------|-------|-------|
| | | | |

## Derived tables (Chain)

Stable `datasets.main.*` tables built for **Chain** workflows (not necessarily uploaded file datasets):

| Table name | Built from | Purpose | Owner / TTL |
|------------|------------|---------|-------------|
| | | | |

## Saved query index (Library)

Link business questions to saved queries (ids/names from `hotdata queries list`):

| Question / report | Saved query name | ID (optional) |
|-------------------|------------------|---------------|
| | | |

## Notes

Assumptions, known gaps, and refresh checklist.
