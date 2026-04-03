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
