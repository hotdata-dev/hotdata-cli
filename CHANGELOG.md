## [0.2.2] - 2026-05-04

### 🚀 Features

- *(wizard)* Render schema description, examples, defaults (#75)
## [0.2.1] - 2026-04-30

### 🐛 Bug Fixes

- *(changelog)* Keep prior release sections identical to main

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
- *(deps)* Bump rustls-webpki to 0.103.13
- Validate CHANGELOG sections match base branch on PRs

### 🚜 Refactor

- *(skills)* Always auto-update skills when eligible (remove env opt-out)

### 🎨 Styling

- *(datasets)* Drop redundant Stylize import in update path
## [0.1.14] - 2026-04-28

### 🚀 Features

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
