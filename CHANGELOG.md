## [0.31.0] - 2026-09-04

### 🚀 Features

- *(databases)* [**breaking**] Lineage walks the whole fork family (#288)

### 🐛 Bug Fixes

- *(query)* Render fetched results with arrow-json (#285)

### 💼 Other

- *(deps)* Arrow 55 -> 59, hotdata 0.16.0 (#286)

### 📚 Documentation

- *(skills)* Document databases lineage, forked_from, and retired job types (#287)
## [0.30.0] - 2026-09-03

### 🐛 Bug Fixes

- *(query)* Render named-timezone timestamps (#283)
## [0.29.0] - 2026-09-03

### 🚀 Features

- *(databases)* Lineage command and forked_from provenance (#281): new `hotdata databases lineage [database]` renders a database's fork tree (ancestor chain and direct forks, deleted generations marked, `--forks-limit` paging), and `databases show`/`fork` display a fork's `forked_from` record — source id, its label at fork time, the copied snapshot, and when — in the table views and as a nested object in `-o json/yaml` (omitted for non-forks, whose output is unchanged).

### 🗑️ Removed

- `jobs list --type` no longer accepts `data_refresh_table` / `data_refresh_connection` — those job types were retired server-side with the connections removal (the engine is push-only), and the SDK (now 0.15.0) dropped them.
## [0.28.0] - 2026-08-25

### 🚀 Features

- *(databases)* Support append mode for table loads (#276)

### 📚 Documentation

- Rename the user-facing term "managed database" to "instant database" across help text, README, and skill docs. Command names, flags, and server wire-format values (such as the `managed_load` job type) are intentionally unchanged.

## [0.27.1] - 2026-08-22

### 🐛 Bug Fixes

- *(errors)* Never mask a 4xx server message with the re-auth hint (#273)
## [0.27.0] - 2026-08-21

### ⚠️ Breaking — command surface reorganized (migration guide)

The top level went from 19 groups to 8 (`auth`, `workspaces`, `databases`, `query`, `jobs`, `ingest`, `search`, `manage`). **Old command spellings are removed with no aliases** — update scripts and muscle memory. All behavior and results are unchanged; only the command paths moved.

**Re-parented groups**

| Old | New |
| :-- | :-- |
| `hotdata tables …` | `hotdata databases tables …` |
| `hotdata queries …` | `hotdata databases queries …` |
| `hotdata results …` | `hotdata databases results …` |
| `hotdata context …` | `hotdata databases context …` |
| `hotdata datasource …` | `hotdata ingest sources …` |
| `hotdata indexes …` | `hotdata search …` (`create` / `list` / `remove`) |
| `hotdata embedding-providers …` | `hotdata search embeddings …` |
| `hotdata usage` / `completions` / `upgrade` / `skills …` | `hotdata manage …` |
| `hotdata connections …` | **removed** — use `hotdata ingest sources` (external sources) + `hotdata databases attach` (join) |

**Renamed verbs**

| Old | New |
| :-- | :-- |
| `databases set` / `workspaces set` | `… use` |
| `databases delete` | `databases remove` |
| `ingest cancel` | `ingest pause` |
| `ingest runs` | `ingest logs` |
| `ingest delete` | `ingest remove` |
| `datasource validate` | `ingest sources test` |
| `datasource create` | `ingest sources add` |
| `datasource delete` | `ingest sources remove` |
| `results show` | `databases results get` |
| `embedding-providers get` / `create` / `delete` | `search embeddings show` / `add` / `remove` |

**Reshaped flags**

- Search runs by **index name**: `search "<text>" --index <name>` (create the index first with `search create`). The old `search --table … --column … --type …` form is gone.
- `indexes create --catalog … --schema … --table … --name <n>` → `search create <n> --from <catalog.schema.table>`; `--embedding-provider-id` → `--provider`.
- Context targeting: `--database-id` → `--database` / `-d`.

**New in this release**

- `hotdata query --dialect duckdb|postgres|snowflake` — write SQL in another dialect; the server transpiles it to HotSQL (read-only).
- `hotdata ingest create --stream` — shorthand for a continuous ingest.

### 🚀 Features

- *(query)* Add --dialect to transpile DuckDB/Postgres/Snowflake SQL (#266)

### 💼 Other

- Remove dead code and clear all clippy warnings (#268)

### 🚜 Refactor

- *(cli)* [**breaking**] Reorganize command surface (#263)

### 📚 Documentation

- Shorten README and drop the removed connections command (#267)
- *(skills)* Correct the --database claim for databases tables show (#269)
## [0.26.0] - 2026-08-19

### 🚀 Features

- *(auth)* [**breaking**] Stop minting jwts from api tokens
- *(sdk)* [**breaking**] Adopt hotdata 0.13.0 and restore the per-request bearer hook
- *(databases)* [**breaking**] Remove the run command

### 🐛 Bug Fixes

- *(databases)* Stop tables load misrouting into wrong database
- *(sdk)* Re-resolve bearer per call for CLI-owned request paths
- *(sdk)* Diagnose expired-session upload failures, fix blocking probe
- *(sdk)* Scope expired-session upload hint to real sessions
- *(databases)* Treat a 403 existence check as unverifiable in set

### 🚜 Refactor

- *(auth)* Share the /workspaces fetch helper
- *(databases)* [**breaking**] Drop guessed database-token detection

### 📚 Documentation

- *(ingest)* Create starts no run — correct the causality (#260)
- *(jwt)* Correct stale JWT-exchange doc comments
- *(credentials)* Correct stale empty-ids semantics

### 🧪 Testing

- *(sdk)* Prove the bearer re-resolves per request, not once
- *(sdk)* Pin which mint answers which upload refresh

### ⚠️ Breaking: `databases run` is gone, and API tokens are no longer exchanged for JWTs

`databases run` is removed with no replacement. It was the only command that
minted a database session token, so the client side of that credential goes
with it — the `HOTDATA_DATABASE_TOKEN` auth path no longer exists.
`HOTDATA_DATABASE` and the `hd_…` database API token are untouched.

An `hd_…` API token is now sent verbatim as the bearer instead of being
exchanged for a JWT, and the credential is resolved once per request rather
than once per client. That fixes uploads large enough to outlive the credential
they started with, which used to 401 at finalize (#120).

Three consequences of losing the JWT claims the CLI used to read:

- `auth status` no longer shows an "API Key Source" row.
- `tables load` no longer routes a database-scoped API token to the
  database-scoped endpoints — the claim it keyed off is gone and there is no
  replacement signal, so those tokens now surface the server's `ACCESS_DENIED`
  403. `databases set` still works with them: its existence check is advisory,
  so a 403 there means "can't verify, proceed".
- Workspace scope for an API key comes from a live `GET /workspaces` instead of
  a claim. An unrestricted key with no saved default used to have no
  discoverable scope and forced `--workspace-id`; it now resolves to whichever
  workspace the API lists first.
## [0.25.0] - 2026-08-14

### 🚀 Features

- *(ingest)* [**breaking**] Split ingest into datasource, ingest, and run (#256)

### ⚠️ Breaking: every `ingest` command moved

`ingest` had grown to mean four things — a stored connection, a one-time
import, a recurring sync, and the job you asked about — so `ingest status <id>`
took an id that could be any of them and answered differently depending which.
They are now three nouns with three command groups.

| Was | Now |
|---|---|
| `ingest new-datasource` | `datasource create` |
| `ingest list-datasources` / `show-datasource` / `delete-datasource` | `datasource list` / `show` / `delete` |
| `ingest new-import` | `ingest create --type one-time` |
| `ingest list-imports` | `ingest list` |
| `ingest status <id>` | `ingest run <run_id>` / `ingest runs <ingest_id>` |
| `ingest raw-sql "…"` | `ingest create --raw-sql "…"` |
| `ingest trigger-import` | **removed, no replacement** |

Every retired verb still runs and tells you what to use instead, so the change
is discoverable rather than an unrecognised-subcommand error.

`trigger-import` is gone because a recurring load resumes from its last
committed position — re-running by hand is not the recovery it looks like. To
bring the next scheduled run forward: `ingest schedule <id> --next now`. For a
one-off, create another one-time ingest.

A run is addressed under its ingest — `ingest runs <ingest_id>` lists them,
`ingest run <run_id>` shows one. There is no top-level `run` command: that word
already belongs to `databases run`, `jobs`, and `queries`.

Two smaller changes worth knowing:

- `--sql "SELECT … FROM a.b"` now reads `a.b` as `schema.table`. The datasource
  is named by `--source` (a display name or an id) or `--datasource-id`.
- Loads no longer start the moment you create them; the scheduler dispatches
  them. `--wait` polls rather than driving, so it watches instead of making
  anything happen sooner.

**Requires the matching service release.** These commands call endpoints that
do not exist in earlier versions.
## [0.24.0] - 2026-08-12

### 🚀 Features

- *(databases)* Count databases in the workspace
## [0.23.0] - 2026-08-09

### 🚀 Features

- *(ingest)* --record-shape flag for new-datasource (#251)
## [0.22.0] - 2026-08-08

### 🚀 Features

- *(ingest)* --continuous flag on new-datasource (buckets) (#249)

### 🚜 Refactor

- *(skills)* Bundle search/analytics/geospatial under hotdata + fix CLI drift (#244)
- Replace user-facing "connection" with "catalog" (keep wire contract) (#245)

### 🧪 Testing

- Retire stale connections scenario tests (command removed in #221) (#246)
## [0.21.0] - 2026-07-31

### 🚀 Features

- *(databases)* Drain cursor for paginated databases list (#239)
- *(ingest)* Add `hotdata ingest raw-sql` verb

### 🐛 Bug Fixes

- *(deps)* Bump open and quinn-proto for advisories
- *(deps)* Drop number_prefix, bump vuln crates

### 💼 Other

- Disable persisted credentials on CI checkouts
## [0.20.0] - 2026-07-20

### 🚀 Features

- *(queries)* Expose bytes_scanned and rows_scanned
- *(query)* Surface query_run_id and serialize result struct directly

### 🐛 Bug Fixes

- *(query)* Preserve query_run_id when following a truncated inline result

### 💼 Other

- Add default CODEOWNERS

### 🚜 Refactor

- *(util)* Make human_bytes take u64 and clamp at call sites
## [0.19.1] - 2026-07-20

### 🐛 Bug Fixes

- *(errors)* Never print a blank error for an empty response body (#232)
- *(auth)* Use the timeout-bounded HTTP client for workspace fetch and auth probe (#233)
## [0.19.0] - 2026-07-20

### 🐛 Bug Fixes

- *(auth)* Serialize token refresh and make config/session writes atomic (#230)

### 📚 Documentation

- *(skills)* Document fork re-attach behavior and add fork-before-risky-changes workflow (#229)
## [0.18.0] - 2026-07-15

### 🚀 Features

- *(databases)* Add fork subcommand (#227)
## [0.17.1] - 2026-07-14

### 🐛 Bug Fixes

- *(ingest)* cancel command exits 0 on success (#225)
## [0.17.0] - 2026-07-14

### 🚀 Features

- *(ingest)* Add cancel subcommand and handle cancelled status (#223)
## [0.16.0] - 2026-07-10

### 🚀 Features

- *(tables)* Scope `tables list` to active database; honor `--table`, `--limit`, `--cursor` within that scope (#221)
- *(tables)* New `tables show <schema.table>` subcommand; accepts `schema.table` (active DB) or `catalog.schema.table` (#221)
- *(results)* New `results show <id>` subcommand; downloads and prints stored result (#221)
- *(indexes)* Scope `indexes list` to active database's connection; remove `--connection-id` flag (#221)
- *(indexes)* `indexes delete` now uses `--catalog` instead of `--connection-id` (#221)
- *(search)* `--table` now accepts `schema.table` (active DB) or `catalog.schema.table` (#221)

### 🔧 Breaking Changes

- *(connections)* `hotdata connections` command removed; use `hotdata databases` for managed databases (#221)
- *(indexes)* `indexes list --connection-id` removed; scopes automatically to active database (#221)
- *(indexes)* `indexes delete --connection-id` removed; use `--catalog` instead (#221)

## [0.15.0] - 2026-07-10

### 🚀 Features

- *(databases)* Show created_at in list and show commands (#217)
- *(ingest)* Rename the catalog verb connectors -> datasources (#216)
- *(cli)* Loading indicators for indexes list and usage (#215)
- *(databases)* Title the active-database star column DEFAULT (#214)
## [0.14.0] - 2026-07-09

### 🚀 Features

- *(ingest)* [**breaking**] Datasources rename + stop leaking service internals (#211)
## [0.13.1] - 2026-07-09

### 🚜 Refactor

- *(ingest)* [**breaking**] Remove the pre-rename verb aliases (#209)
## [0.13.0] - 2026-07-09

### 🚀 Features

- *(databases)* Load a query result via --result-id (#203)
- *(ingest)* Add `hotdata ingest` command group
- *(ingest)* Add-connection discovers schema only, loads no data
- *(ingest)* Show live stage progress while polling
- *(ingest)* Mark active connectors in `connectors`
- *(ingest)* Connection/import command surface, API-backed listings, true re-run
- *(ingest)* Imports return immediately; add `status` for tracking
- *(ingest)* Color the STATUS column in list-imports/list-connections
- *(ingest)* IMPORT ID is the leftmost list-imports column
- *(ingest)* Listing tables read oldest-to-newest
- *(ingest)* Show-connection and delete-connection
- *(ingest)* Product-noun connection type labels — SQL, buckets, API

### 🐛 Bug Fixes

- *(ingest)* Redact source secrets from --debug request logging
- *(ingest)* Actionable hint on enqueue transport failures
- *(workspace)* Report the workspace commands actually target
- *(ingest)* Wizard iceberg catalog key is uri, not catalog_uri
- *(ingest)* Send connector_type for iceberg so imports can resolve it
- *(ingest)* Send connector_type for filesystem so imports can resolve it

### 💼 Other

- *(release)* Drop README version-badge replacement

### 🚜 Refactor

- *(ingest)* Mirror the `connections` UX — new/list/create/import/refresh
- *(ingest)* Drop display-side connector dedup (fixed at the source)
- *(ingest)* Consolidate add-connection under `new`; drop `create`
- *(ingest)* Rename `refresh` to `update`
- *(ingest)* Quality-review pass — bugs, reuse, DRY, docs

### 📚 Documentation

- *(ingest)* Terse one-line command summaries in help
- *(skills)* Teach the hotdata agent skill the ingest surface
- *(skills)* Lead the new-connection example with @file config
- Buckets and iceberg connectors in the skill and README
- *(readme)* Rework for users — quickstart first, tasks over flags (#207)

### 🎨 Styling

- Rustfmt
## [0.12.0] - 2026-07-07

### 🚀 Features

- *(queries,results)* Scope to database for SDK 0.8.0

### 🚜 Refactor

- *(seam)* Consolidate database scoping into scoped_to_database_opt
## [0.11.1] - 2026-07-03

### 🐛 Bug Fixes

- *(indexes)* Accept --catalog on 'indexes delete'
- *(indexes)* Validate delete scope via clap; default schema to public

### 🚜 Refactor

- *(modules)* Move to module layout for sub-command structure
## [0.11.0] - 2026-06-30

### 🚀 Features

- *(upload)* Large or slow `databases load` uploads now ride out a flaky connection instead of failing partway through: a single dropped or stalled part is retried on its own — with a fresh upload link and a timeout so it can't hang indefinitely — while parts that already finished are kept, so one part's bad moment no longer aborts the whole transfer

### 🐛 Bug Fixes

- *(jobs)* Accept managed_load as --job-type filter (#160) (#195)

### 💼 Other

- *(deps)* Bump hotdata SDK to 0.7.0
## [0.10.0] - 2026-06-29

### 🚀 Features

- *(upload)* Very large or slow `databases load` uploads no longer fail partway through — each part's upload link is now fetched just in time and renewed automatically if it expires

### 💼 Other

- *(auth)* `hotdata auth` now shows help instead of auto-launching a browser login — run `hotdata auth login` to sign in; adds `auth profiles` to scaffold profiles.yml (#182)

### 📚 Documentation

- *(skills)* Update bundled skill docs to match current commands and auth

## [0.9.0] - 2026-06-27

### 🚀 Features

- *(databases)* Add auth and command limitations for database api token flag
- *(upload)* Speed up large-file uploads with concurrent, direct-to-storage transfer

### 🐛 Bug Fixes

- *(upload)* Clean up --url temp file before exit on failure

### 💼 Other

- *(deps)* Update hotdata SDK to 0.5.0

### 📚 Documentation

- *(deps)* Update reqwest blocking comment for presigned --url path
## [0.8.1] - 2026-06-25

### 🐛 Bug Fixes

- *(query)* Carry execution_time_ms through async poll path
## [0.8.0] - 2026-06-24

### 🚀 Features

- *(upgrade)* Gate commands on a new release; rename update→upgrade (#178)

### 🐛 Bug Fixes

- *(databases)* Suppress current-database footer for non-TTY stdout (#180)
## [0.7.0] - 2026-06-22

### 🚀 Features

- *(databases)* Attach/detach connection catalogs

### 🐛 Bug Fixes

- *(databases)* Tolerate a bad --attach spec in create
- *(query)* Show cross-source hint on poll failure
- *(databases)* Keep attach failures auth-aware like detach

### 📚 Documentation

- Document cross-source query via catalog attach
## [0.6.0] - 2026-06-20

### 🚀 Features

- *(cli)* Show "waking up worker" hint on KEDA cold starts (#167)
- *(datasets)* [**breaking**] Remove datasets commands and dataset feature surface (#166)
- *(usage)* Add `hotdata usage` command (#174)

### 🐛 Bug Fixes

- *(search)* Infer embedding source column for vector indexes (#163)
- *(indexes)* List indexes for a database-scoped connection (#164)
- *(query)* Fail loud on incomplete result previews
- *(indexes)* Show managed-database indexes in unscoped `indexes list` (#170)
- *(databases)* Surface the id change when `load` recreates a managed database (#173)

### 💼 Other

- *(deps)* Upgrade hotdata SDK 0.3.1 -> 0.4.0 (#171)

### 📚 Documentation

- *(skills)* Improve accuracy, structure, and consistency across CLI skills (#172)

### 🧪 Testing

- *(query)* Cover table footer rendering and ApiError::message
## [0.5.0] - 2026-06-16

### 🚀 Features

- Follow truncated inline query results to full set
- Auto-retry queries shed under load (HTTP 429 `OVERLOADED`), honoring `Retry-After`

### 🐛 Bug Fixes

- Preserve inline warning and timing when following truncation
- Stop using deprecated `QueryResponse.row_count`

### 💼 Other

- Bump hotdata SDK to 0.3.1

## [0.4.2] - 2026-06-15

### 📚 Documentation

- *(skills)* Fix stale datasets create flags and add --no-input (#153)
## [0.4.1] - 2026-06-13

### 🚀 Features

- *(sdk)* Add sync wrapper and CliTokenProvider
- *(query)* Remove dead --connection flag

### 🐛 Bug Fixes

- *(sdk)* Avoid double /v1 and scope by database
- *(sdk)* Restore sandbox scope, guard, timeout
- *(sdk)* Guard negative numeric casts
- *(sdk)* Set hotdata-cli user-agent header
- *(sdk)* Drop dead X-Sandbox-Id header
- *(ci)* Skip scenario-parity for Dependabot PRs
- *(release)* Prepend unreleased changelog instead of full regen

### 💼 Other

- Pin third-party github actions to commit SHAs
- *(deps)* Add hotdata sdk, tokio, async-trait
- *(deps)* Pin hotdata sdk to merged rev
- *(deps)* Consolidate CLI on reqwest 0.13
- Add cargo fmt check job
- *(deps)* Pin sdk-rust to upload_stream content_length rev
- Remove sandbox cli commands
- *(deps)* Use published hotdata 0.1.1 from crates.io
- *(ci)* Add Dependabot to track published hotdata SDK

### 🚜 Refactor

- *(http)* Add slim raw-http helper
- *(workspace)* Use sdk workspaces handle
- *(jobs)* Use sdk jobs handle
- *(tables)* Use sdk information_schema
- *(queries)* Use sdk query_runs handle
- *(results)* Use sdk results handle
- *(embeddings)* Use sdk providers handle
- *(context)* Use sdk database_context
- *(datasets)* Use sdk datasets handle
- *(connections)* Migrate connections_new
- *(connections)* Use sdk connections handle
- *(sandbox)* Use sdk sandboxes handle
- *(indexes)* Use sdk indexes handle
- *(query)* Poll+arrow via sdk handles
- *(databases)* Use sdk databases handle
- *(api)* Remove legacy ApiClient
- *(query)* Submit via sdk submit_query
- *(sdk)* Extract apply_seam_headers helper
- *(sdk)* Drop stale dead_code allows
- *(query)* Decode results via SDK get_result_arrow (arrow v55)
- Migrate raw HTTP to typed SDK (#131)
- *(databases)* Clarify output DTOs, use From trait
- Drop dead Deserialize derives on output DTOs
- *(databases)* Stream /files upload via SDK upload_stream
- *(databases)* Drop redundant upload content-type param

### 📚 Documentation

- *(sdk)* Drop migration-history notes from comments
- Describe current behavior, not change history, in comments

### 🎨 Styling

- Clear clippy lints in migrated modules
- Apply cargo fmt to codebase
- Apply rustfmt

### 🧪 Testing

- *(sdk)* Cover generic HTTP status preservation in from_arrow
- *(cli)* Add env-gated scenario integration tests
## [0.4.0] - 2026-06-04

### 🚀 Features

- *(databases)* Add --catalog flag to databases create (#125)
- *(queries)* Show result_id in queries list table (#126)
- Managed database demo flow — explicit flags, catalog resolution, BM25 search (#127)

## [0.3.4] - 2026-06-04

### 🚀 Features

- *(databases)* Add databases run command for and isolated database CLI (#118)

### 🐛 Bug Fixes

- Handle pre-existing draft release in host job (#116)
- *(api)* Add API timeout relaxation and refresh token retry ability
- *(databases)* Rename --description to --name in databases run (#122)
- *(skills)* Update --description to --name in databases commands (#123)

### 💼 Other

- Allow dirty ci in dist-workspace config
- *(ci)* Bump Node 20 actions to Node 24 runtime

## [0.3.3] - 2026-05-28

### 🐛 Bug Fixes

- *(databases)* Use name not description for API alignment (#112)

## [0.3.2] - 2026-05-27

### 🐛 Bug Fixes

- *(datasets)* Add missing `-o`/`--output` flag to `datasets create`; move success banner to stderr so `-o json` stdout is `jq`-parseable (#110)
- *(sandbox)* Move "Sandbox created" and "Sandbox updated" banners to stderr for clean `-o json` output (#110)
- *(sandbox)* Fix missing trailing newline in `sandbox read` output (#110)
- *(sandbox)* Add `sandbox delete <id>` subcommand; clears the active session automatically when the deleted sandbox was the active one (#110)
- *(workspaces)* Fix incorrect lock check in `workspaces set` — was checking `HOTDATA_WORKSPACE` (always set in sandbox runs), now correctly checks `HOTDATA_SANDBOX` (#110)
- *(context)* Surface a friendly hint when `context push` is blocked inside an active sandbox, pointing users to `hotdata sandbox set` (no args) to clear it (#110)

## [0.3.1] - 2026-05-25

### 🐛 Bug Fixes

- *(skills)* Bump skill file versions to 0.3.1 so `hotdata skills install` correctly detects and installs the latest skills for CLI v0.3.x

## [0.3.0] - 2026-05-23

### 🚀 Features

- *(query)* Fetch results as Arrow IPC instead of JSON; reduces transfer size and preserves native types (#103)
- *(query)* Add `--database` / `-d` flag to scope a query to a managed database without changing the active database (#102)
- *(databases)* Add `databases show <id>` as an explicit subcommand alias (#103)
- *(databases)* `databases tables <id>` now lists tables without requiring the `list` subcommand (#103)
- *(skills)* Add `skills list` as an alias for `skills status` (#103)
- *(update)* Background update check with post-command notice; never blocks command output (#104)
- *(update)* Auto-install and update skills to match the new binary version during `hotdata update` (#105)
- *(update)* Execute `brew upgrade` directly for Homebrew installs instead of printing manual instructions (#106)

### 🐛 Bug Fixes

- *(query)* Async polling loop exits with code 2 on unexpected statuses instead of spinning forever (#103)
- *(query)* Failed async queries now surface the real server error message (#103)
- *(query)* `results get <id>` now fetches Arrow IPC like the rest of the query path (#103)
- *(query)* Polling loop polls first before checking the deadline, eliminating a mandatory 500ms delay (#106)
- *(skills)* Add 120-second HTTP timeout to the skills tarball download during `hotdata update` (#106)

## [0.2.9] - 2026-05-22

### 📚 Documentation

- *(skills)* Update skills to reflect recent API changes: database-scoped context, `databases set`, `--expires-at`, corrected flag names for `databases create` / `datasets create` / `datasets update` (#100)

## [0.2.8] - 2026-05-22

### 🚀 Features

- *(context)* Scope context commands to active database; `hotdata context` now calls `/databases/{id}/context` and requires `--database-id` or an active database set via `databases set` (#98)
- *(databases)* Add `--expires-at` flag to `databases create`; accepts relative durations (`24h`, `7d`) or RFC 3339 timestamps (#97)
- *(datasets)* Remove upload/URL/file create paths; `datasets create` now requires exactly one of `--sql` or `--query-id` (#95)
- *(databases)* Migrate CLI to dedicated `/databases` API; `databases set` saves active database; `X-Database-Id` header sent automatically on all requests (#94)

### 🐛 Bug Fixes

- *(datasets)* Add missing `type` discriminator to dataset source payloads sent to API
- *(context)* Correct `--database-id` flag name in error message

## [0.2.7] - 2026-05-20

### 🚀 Features

- *(indexes)* Dot-bracket notation for `indexes create`: `airbnb.listings[col1,col2]` replaces `--connection-id/--schema/--table/--columns` (#92)
- *(databases)* Add `databases load <db.table>` shorthand replacing `databases tables load` (#92)
- *(indexes)* Make `--name` optional on `indexes create`; auto-derived from table, columns, and type (#92)

### 🐛 Bug Fixes

- *(databases)* Remove `load:` hint from `databases create` success output (#92)

## [0.2.6] - 2026-05-19

### 🚀 Features

- *(search)* Infer `--type` and `--column` from table indexes; schema defaults to `public` (#90)

### 🐛 Bug Fixes

- *(search)* Explicit error when a search index has no columns (#90)

## [0.2.5] - 2026-05-19

### 🚀 Features

- *(databases)* Add `--url` flag to `tables load` for remote parquet files (#88)
## [0.2.4] - 2026-05-19

### 🚀 Features

- *(auth)* Add `hotdata auth register` command (#85, #86)
- *(auth)* Default register to GitHub; add `--email` flag
- *(update)* Add `hotdata update` command
- *(skills)* Split bundled skills into `hotdata-search` and `hotdata-analytics` (#84)

### 🐛 Bug Fixes

- *(auth)* Align CLI callback page colors with web app theme

### 🚜 Refactor

- *(auth)* Extract `run_browser_auth` helper; add tests for `exchange_cli_register_code`

### 📚 Documentation

- *(skill)* Epic flow checklists, datasets vs databases workflows, tag-only release finish (#84)
## [0.2.3] - 2026-05-19

### 🚀 Features

- *(databases)* Add managed databases CLI for parquet table loads (#82)
- *(sandbox)* Add sandbox JWT support
- *(tty)* Add no-input flag and tty checks for interactive commands

### 🐛 Bug Fixes

- *(deps)* Bump openssl to 0.10.79 for CVE fixes (#77)

### 💼 Other

- Ignore macOS metadata files (#81)

### 📚 Documentation

- *(skill)* Document managed databases commands
## [0.2.2] - 2026-05-04

### 🚀 Features

- *(wizard)* Render schema description, examples, defaults (#75)

## [0.2.1] - 2026-04-30

### 📚 Documentation

- *(skill)* Align hotdata skill with CLI behavior

## [0.2.0] - 2026-04-29

### 🚀 Features

- *(datasets)* Add update subcommand to rename label or table_name
- Data/dataset refresh + indexes auto-embedding + embedding providers (#67)
- *(skills)* Add optional geospatial agent skill
- *(skills)* Auto-update bundled agent skills after CLI upgrade

### 🐛 Bug Fixes

- *(datasets)* Match runtimedb response shape on update
- *(datasets)* Drop synthetic schema_name on update output
- *(datasets)* Restore eprintln for "Dataset updated" status line
- *(skills)* Complete partial installs and improve status output
- *(skills)* Show Installed: No when no skill store exists
- *(skills)* Stop repeat auto-downloads (parse SKILL.md, stale tarball guard)

### 💼 Other

- *(release)* Bump geospatial skill version on release

### 🚜 Refactor

- *(skills)* Always auto-update skills when eligible (remove env opt-out)

### 🎨 Styling

- *(datasets)* Drop redundant Stylize import in update path
## [0.1.14] - 2026-04-28

### 🚀 Features

- *(auth)* Add CLI auth session support (JWT access tokens, refresh, PKCE login)
- *(indexes)* Workspace-wide list with filters and parallel fetch

### 💼 Other

- *(codecov)* Treat patch coverage as informational

### 🧪 Testing

- Raise coverage for indexes list and get_none_if_not_found
## [0.1.13] - 2026-04-24

### 🚀 Features

- *(auth)* Add login subcommand mirroring bare auth

### 🐛 Bug Fixes

- *(context)* Strip .md suffix using correct byte length
- *(context)* Avoid UTF-8 panic when probing .md suffix

### 💼 Other

- *(release)* Pass --no-confirm to cargo release

### 📚 Documentation

- *(skill)* List before show; avoid blind context show DATAMODEL
- *(skill)* Context:<STEM> notation and analysis vs DATAMODEL
## [0.1.12] - 2026-04-24

### 🚀 Features

- *(context)* Add context list/show/pull/push commands

### 🐛 Bug Fixes

- *(context)* Fail-fast pull when target exists; expand stem tests

### 💼 Other

- *(release)* Regenerate changelog with git-cliff

### 🚜 Refactor

- *(context)* Clearer fetch_context exhaustiveness; drop no-op mkdir

### 📚 Documentation

- *(cli)* Clarify datasets command as upload and query Parquet/CSV
- *(skill)* Prefer workspace context API for data model and agents
- *(skill)* Context API only for data model and workspace docs
- *(readme)* Document workspace context commands and API-first model
- *(skill)* Align Hotdata SKILL with current CLI flags
- *(skill)* Sandbox datasets, long flags, and WORKFLOWS
- *(skill)* Unify dataset SQL as datasets.<schema>.<table>
## [0.1.11] - 2026-04-20

### 🚀 Features

- *(sandbox)* Align CLI with updated sandbox API
## [0.1.10] - 2026-04-17

### 🐛 Bug Fixes

- *(connections)* Add health check to connection flow
## [0.1.9] - 2026-04-10

### 🚀 Features

- *(sessions)* Add session command feature

### 🐛 Bug Fixes

- *(security)* Update tar from 0.4.44 to 0.4.45
- *(ci)* Update workflow actions for Node.js 24 compatibility
- *(ci)* Revert release.yml (cargo-dist managed)
- *(ci)* SHA-pin codecov-action for consistency
- *(ci)* Keep create-github-app-token at v1 to avoid breaking changes

### 📚 Documentation

- *(skill)* Document results list, connections refresh, queries update flags
- *(skill)* Workflows, references, and SKILL alignment
## [0.1.8] - 2026-04-03

### 🐛 Bug Fixes

- *(auth)* Fix config initialization and url upload api parameter
## [0.1.7] - 2026-03-30

### 🚀 Features

- *(search)* Add basic vector search (l2_distance)
- *(query)* Async query execution by returning early from long running queries
- *(connections)* Add connection information command (#28)
## [0.1.6] - 2026-03-27

### 🚀 Features

- *(completions)* Add completions command and include in brew formula
- *(search)* Add indexes and text search commands
- *(queries)* Add upload url for datasets and new queries commands (#23)
## [0.1.5] - 2026-03-24

### 🚀 Features

- *(datasets)* Add --sql and --query-id to datasets create
- *(workspaces)* Add set command to switch default workspace
- *(results)* Add results list command
- *(connections)* Add connections refresh endpoint
- *(jobs)* Add jobs commands

### 🐛 Bug Fixes

- *(cli)* Add table formatting and fix wrapping issues
- *(cli)* Skills/Auth CLI command quality of life improvements
## [0.1.4] - 2026-03-17

### 🚀 Features

- *(skills)* Add skill install command for CLI
- *(datasets)* Add datasets commands to create, list, and view datasets in CLI
- *(datasets)* Add upload id functionality to datasets create command
- *(connections)* Add connection command to list/view/create connections via the CLI
## [0.1.3] - 2026-03-11

### 🚀 Features

- *(results)* Add query result retrieval via results command
- *(workspaces)* Default workspace support for CLI commands
- *(tables)* Add search and pagination to tables list command

### 🎨 Styling

- Format table list output with full table path <connection>.<schema>.<name>
## [0.1.2] - 2026-03-10

### 🚀 Features

- *(tables)* Add table list command and better table rendering
## [0.1.1] - 2026-03-09

### 🚀 Features

- *(workspace)* Add workspace list command
- *(connection)* Add connections list command
- *(query)* Add query execution command
## [0.1.0] - 2026-03-06
