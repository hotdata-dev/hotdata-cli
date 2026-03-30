---
name: hotdata-cli
description: Use this skill when the user wants to run hotdata CLI commands, query the Hotdata API, list workspaces, list connections, create connections, list tables, manage datasets, execute SQL queries, manage saved queries, search tables, manage indexes, or interact with the hotdata service. Activate when the user says "run hotdata", "query hotdata", "list workspaces", "list connections", "create a connection", "list tables", "list datasets", "create a dataset", "upload a dataset", "execute a query", "search a table", "list indexes", "create an index", "list saved queries", "run a saved query", or asks you to use the hotdata CLI.
version: 0.1.6
---

# Hotdata CLI Skill

Use the `hotdata` CLI to interact with the Hotdata service. In this project, run it as:

```
hotdata <command> [args]
```

Or if installed on PATH: `hotdata <command> [args]`

## Authentication

Run `hotdata auth` to authenticate via browser login. Config is stored in `~/.hotdata/config.yml`.

API key resolution (lowest to highest priority):
1. Config file (saved by `hotdata auth`)
2. `HOTDATA_API_KEY` environment variable (or `.env` file)
3. `--api-key <key>` flag (works on any command)

API URL defaults to `https://api.hotdata.dev/v1` or overridden via `HOTDATA_API_URL`.

## Workspace ID

All commands that accept `--workspace-id` are optional. If omitted, the active workspace is used. Use `hotdata workspaces set` to switch the active workspace interactively, or pass a workspace ID directly: `hotdata workspaces set <workspace_id>`. The active workspace is shown with a `*` marker in `hotdata workspaces list`. **Omit `--workspace-id` unless you need to target a specific workspace.**

## Available Commands

### List Workspaces
```
hotdata workspaces list [--format table|json|yaml]
```
Returns workspaces with `public_id`, `name`, `active`, `favorite`, `provision_status`.

### List Connections
```
hotdata connections list [-w <workspace_id>] [-o table|json|yaml]
hotdata connections <connection_id> [-w <workspace_id>] [-o table|json|yaml]
```
- `list` returns `id`, `name`, `source_type` for each connection.
- Pass a connection ID to view details (id, name, source type, table counts).

### Create a Connection

#### Step 1 — Discover available connection types
```
hotdata connections create list [--workspace-id <workspace_id>] [--format table|json|yaml]
```
Returns all available connection types with `name` and `label`.

#### Step 2 — Inspect the schema for a specific type
```
hotdata connections create list <name> [--workspace-id <workspace_id>] [--format json]
```
Returns `config` and `auth` JSON Schema objects describing all required and optional fields for that connection type. Use `--format json` to get the full schema detail.

- `config` — connection configuration fields (host, port, database, etc.). May be `null` for services that need no configuration.
- `auth` — authentication fields (password, token, credentials, etc.). May be `null` for services that need no authentication. May be a `oneOf` with multiple authentication method options.

#### Step 3 — Create the connection
```
hotdata connections create \
  --name "my-connection" \
  --type <source_type> \
  --config '<json object>' \
  [--workspace-id <workspace_id>]
```

The `--config` JSON object must contain all **required** fields from `config` plus the **auth fields** merged in at the top level. Auth fields are not nested — they sit alongside config fields in the same object.

Example for PostgreSQL (required: `host`, `port`, `user`, `database` + auth field `password`):
```
hotdata connections create \
  --name "my-postgres" \
  --type postgres \
  --config '{"host":"db.example.com","port":5432,"user":"myuser","database":"mydb","password":"..."}'
```

**Security: never expose credentials in plain text.** Passwords, tokens, API keys, and any field with `"format": "password"` in the schema must never be hardcoded as literal strings in CLI commands. Always use one of these safe approaches:

- Read from an environment variable:
  ```
  --config "{\"host\":\"db.example.com\",\"port\":5432,\"user\":\"myuser\",\"database\":\"mydb\",\"password\":\"$DB_PASSWORD\"}"
  ```
- Read a credential from a file and inject it:
  ```
  --config "{\"token\":\"$(cat ~/.secrets/my-token)\"}"
  ```

**Field-building rules from the schema:**

- Include all fields listed in `config.required` — these are mandatory.
- Include optional config fields only if the user provides values for them.
- For `auth` with a single method (no `oneOf`): include all `auth.required` fields in the config object.
- For `auth` with `oneOf`: pick one authentication method and include only its required fields.
- Fields with `"format": "password"` are credentials — apply the security rules above.
- Fields with `"type": "integer"` must be JSON numbers, not strings (e.g. `"port": 5432` not `"port": "5432"`).
- Fields with `"type": "boolean"` must be JSON booleans (e.g. `"use_tls": true`).
- Fields with `"type": "array"` must be JSON arrays (e.g. `"spreadsheet_ids": ["abc", "def"]`).
- Nested `oneOf` fields must be a JSON object including a `"type"` discriminator field matching the chosen variant's `const` value.

### List Tables and Columns
```
hotdata tables list [--workspace-id <workspace_id>] [--connection-id <connection_id>] [--schema <pattern>] [--table <pattern>] [--limit <int>] [--cursor <cursor>] [--format table|json|yaml]
```
- Default format is `table`.
- **Always use this command to inspect available tables and columns.** Do NOT use the `query` command to query `information_schema` for this purpose.
- Without `--connection-id`: lists all tables with `table`, `synced`, `last_sync`. The `table` column is formatted as `<connection>.<schema>.<table>`.
- With `--connection-id`: includes column definitions. Lists each column as its own row with `table`, `column`, `data_type`, `nullable`. Use this to inspect the schema before writing queries.
- **Always use the full `<connection>.<schema>.<table>` name when referencing tables in SQL queries.**
- `--schema` and `--table` support SQL `%` wildcard patterns (e.g. `--table order%` matches `orders`, `order_items`, etc.).
- Results are paginated (default 100 per page). If more results are available, a `--cursor` token is printed — pass it to fetch the next page.

### Datasets

Datasets are managed files uploaded to Hotdata and queryable as tables.

#### List datasets
```
hotdata datasets list [--workspace-id <workspace_id>] [--limit <int>] [--offset <int>] [--format table|json|yaml]
```
- Default format is `table`.
- Returns `id`, `label`, `table_name`, `created_at`.
- Results are paginated (default 100). Use `--offset` to fetch further pages.

#### Get dataset details
```
hotdata datasets <dataset_id> [--workspace-id <workspace_id>] [--format table|json|yaml]
```
- Shows dataset metadata and a full column listing with `name`, `data_type`, `nullable`.
- Use this to inspect schema before querying.

#### Create a dataset
```
hotdata datasets create --label "My Dataset" --file data.csv [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --sql "SELECT * FROM ..." [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --query-id <saved_query_id> [--table-name my_dataset] [--workspace-id <workspace_id>]
hotdata datasets create --label "My Dataset" --url "https://example.com/data.parquet" [--table-name my_dataset] [--workspace-id <workspace_id>]
```
- `--file` uploads a local file. Omit to pipe data via stdin: `cat data.csv | hotdata datasets create --label "My Dataset"`
- `--sql` creates a dataset from a SQL query result.
- `--query-id` creates a dataset from a previously saved query.
- `--url` imports data directly from a URL (supports csv, json, parquet).
- `--file`, `--sql`, `--query-id`, and `--url` are mutually exclusive.
- Format is auto-detected from file extension (`.csv`, `.json`, `.parquet`) or file content.
- `--label` is optional when `--file` is provided — defaults to the filename without extension. Required for `--sql` and `--query-id`.
- `--table-name` is optional — derived from the label if omitted.

#### Querying datasets

Datasets are queryable using the catalog `datasets` and schema `main`. Always reference dataset tables as:
```
datasets.main.<table_name>
```
For example:
```
hotdata query "SELECT * FROM datasets.main.my_dataset LIMIT 10"
```
Use `hotdata datasets <dataset_id>` to look up the `table_name` before writing queries.

### Execute SQL Query
```
hotdata query "<sql>" [--workspace-id <workspace_id>] [--connection <connection_id>] [--format table|json|csv]
```
- Default format is `table`, which prints results with row count and execution time.
- Use `--connection` to scope the query to a specific connection.
- Use `hotdata tables list` to discover tables and columns — do not query `information_schema` directly.
- **Always use PostgreSQL dialect SQL.**

### Get Query Result
```
hotdata results <result_id> [--workspace-id <workspace_id>] [--format table|json|csv]
```
- Retrieves a previously executed query result by its result ID.
- Query results include a `result-id` in the footer (e.g. `[result-id: rslt...]`).
- **Always use this command to retrieve past query results rather than re-running the same query.** Re-running queries wastes resources and may return different results.

### Saved Queries
```
hotdata queries list [--limit <int>] [--offset <int>] [--format table|json|yaml]
hotdata queries <query_id> [--format table|json|yaml]
hotdata queries create --name "My Query" --sql "SELECT ..." [--description "..."] [--tags "tag1,tag2"] [--format table|json|yaml]
hotdata queries update <query_id> [--name "New Name"] [--sql "SELECT ..."] [--description "..."] [--tags "tag1,tag2"] [--format table|json|yaml]
hotdata queries run <query_id> [--format table|json|csv]
```
- `list` shows saved queries with name, description, tags, and version.
- View a query by ID to see its formatted and syntax-highlighted SQL.
- `create` requires `--name` and `--sql`. Tags are comma-separated.
- `update` accepts any combination of fields to change.
- `run` executes a saved query and displays results like the `query` command.
- **Use `queries run` instead of re-typing SQL when a saved query exists.**

### Search
```
hotdata search "<query>" --table <connection.schema.table> --column <column> [--select <columns>] [--limit <n>] [--format table|json|csv]
```
- Full-text search using BM25 across a table column.
- Requires a BM25 index on the target column (see `indexes create`).
- Results are ordered by relevance score (descending).
- `--select` specifies which columns to return (comma-separated, defaults to all). The `score` column is automatically appended when `--select` is used.
- Default limit is 10.

### Indexes
```
hotdata indexes list --connection-id <id> --schema <schema> --table <table> [--workspace-id <workspace_id>] [--format table|json|yaml]
hotdata indexes create --connection-id <id> --schema <schema> --table <table> --name <name> --columns <cols> [--type sorted|bm25|vector] [--metric l2|cosine|dot] [--async]
```
- `list` shows indexes on a table with name, type, columns, status, and creation date.
- `create` creates an index. Use `--type bm25` for full-text search, `--type vector` for vector search (requires `--metric`).
- `--async` submits index creation as a background job. Use `hotdata jobs <job_id>` to check status.
- **Before using `hotdata search`, create a BM25 index on the target column.**

### Jobs
```
hotdata jobs list [--workspace-id <workspace_id>] [--job-type <type>] [--status <status>] [--all] [--format table|json|yaml]
hotdata jobs <job_id> [--workspace-id <workspace_id>] [--format table|json|yaml]
```
- `list` shows only active jobs (`pending`, `running`) by default. Use `--all` to see all jobs.
- `--job-type`: `data_refresh_table`, `data_refresh_connection`, `create_index`.
- `--status`: `pending`, `running`, `succeeded`, `partially_succeeded`, `failed`.
- Use `hotdata jobs <job_id>` to inspect a specific job's status, error, and result.

### Auth
```
hotdata auth                # Browser-based login
hotdata auth status         # Check current auth status
```

## Workflow: Running a Query

1. List connections:
   ```
   hotdata connections list
   ```
2. Inspect available tables:
   ```
   hotdata tables list
   ```
3. Inspect columns for a specific connection:
   ```
   hotdata tables list --connection-id <connection_id>
   ```
4. Run SQL:
   ```
   hotdata query "SELECT 1"
   ```

## Workflow: Creating a Connection

1. List available connection types:
   ```
   hotdata connections create list
   ```
2. Inspect the schema for the desired type:
   ```
   hotdata connections create list <type_name> --format json
   ```
3. Collect required config and auth field values from the user or environment. **Never hardcode credentials — use env vars or files.**
4. Create the connection:
   ```
   hotdata connections create --name "my-connection" --type <type_name> --config '<json>'
   ```
