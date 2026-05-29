use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    /// Authenticate or manage auth settings
    Auth {
        #[command(subcommand)]
        command: Option<AuthCommands>,
    },

    /// Derived views — virtual SQL tables built from queries over your data
    Datasets {
        /// Dataset ID to show details
        id: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Output format (used with dataset ID)
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,

        #[command(subcommand)]
        command: Option<DatasetsCommands>,
    },

    /// Execute a SQL query, or check status of a running query
    Query {
        /// SQL query string (omit when using a subcommand)
        sql: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w')]
        workspace_id: Option<String>,

        /// Scope query to a specific connection
        #[arg(long)]
        connection: Option<String>,

        /// Run query against a specific managed database (overrides the current database set via `databases set`)
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "csv"])]
        output: String,

        #[command(subcommand)]
        command: Option<QueryCommands>,
    },

    /// Manage workspaces
    Workspaces {
        #[command(subcommand)]
        command: WorkspaceCommands,
    },

    /// Manage workspace connections
    Connections {
        /// Connection ID to show details
        id: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Output format (used with connection ID)
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,

        #[command(subcommand)]
        command: Option<ConnectionsCommands>,
    },

    /// Managed databases you create and populate with tables (parquet uploads)
    Databases {
        /// Database id or name (omit to use a subcommand)
        name_or_id: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,

        #[command(subcommand)]
        command: Option<DatabasesCommands>,
    },

    /// Manage tables in a workspace
    Tables {
        #[command(subcommand)]
        command: TablesCommands,
    },

    /// Manage the hotdata agent skill
    Skills {
        #[command(subcommand)]
        command: SkillCommands,
    },

    /// Retrieve a stored query result by ID, or list recent results
    Results {
        /// Result ID (omit to use a subcommand)
        result_id: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "csv"])]
        output: String,

        #[command(subcommand)]
        command: Option<ResultsCommands>,
    },

    /// Manage background jobs
    Jobs {
        /// Job ID (omit to use a subcommand)
        id: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Output format (used with job ID)
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,

        #[command(subcommand)]
        command: Option<JobsCommands>,
    },

    /// Manage indexes on a table
    Indexes {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        #[command(subcommand)]
        command: IndexesCommands,
    },

    /// Manage embedding providers (OpenAI, local, etc.) used by vector indexes
    #[command(name = "embedding-providers")]
    EmbeddingProviders {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        #[command(subcommand)]
        command: EmbeddingProvidersCommands,
    },

    /// Full-text or vector search across a table column
    Search {
        /// Search query text — required for both --type bm25 and --type vector
        query: String,

        /// Search type (`bm25` or `vector`). Inferred automatically when the table has exactly
        /// one search index — required only when multiple indexes exist.
        ///
        /// `vector` runs server-side `vector_distance(col, 'text')` — the server resolves the
        /// embedding column, model, and metric from the index metadata.
        ///
        /// `bm25` runs server-side `bm25_search(table, col, 'text')` and requires a BM25 index
        /// on the column.
        #[arg(long, value_parser = ["vector", "bm25"])]
        r#type: Option<String>,

        /// Table to search (`connection.table` or `connection.schema.table`).
        /// Schema defaults to `public` when omitted.
        #[arg(long)]
        table: String,

        /// Column to search. Inferred automatically when the table has exactly one search index
        /// of the resolved type — required only when multiple indexed columns exist.
        /// For `--type vector`, name the source text column — the server resolves the embedding
        /// column from the index metadata.
        #[arg(long)]
        column: Option<String>,

        /// Columns to display (comma-separated, defaults to all)
        #[arg(long)]
        select: Option<String>,

        /// Maximum number of results
        #[arg(long, default_value = "10")]
        limit: u32,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w')]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "csv"])]
        output: String,
    },

    /// Inspect query run history
    Queries {
        /// Query run ID to show details
        id: Option<String>,

        /// Output format (used with query run ID)
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,

        #[command(subcommand)]
        command: Option<QueriesCommands>,
    },

    /// Manage sandboxes
    Sandbox {
        /// Sandbox ID to show details
        id: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,

        #[command(subcommand)]
        command: Option<SandboxCommands>,
    },

    /// Sync database context with local Markdown (`./<NAME>.md` in the current directory)
    Context {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Database ID (defaults to active database set via 'hotdata databases set')
        #[arg(long, short = 'd', global = true)]
        database_id: Option<String>,

        #[command(subcommand)]
        command: ContextCommands,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: ShellChoice,
    },

    /// Update the hotdata CLI to the latest release
    Update,
}

#[derive(Clone, clap::ValueEnum)]
pub enum ShellChoice {
    Bash,
    Zsh,
    Fish,
}

impl From<ShellChoice> for clap_complete::Shell {
    fn from(s: ShellChoice) -> Self {
        match s {
            ShellChoice::Bash => clap_complete::Shell::Bash,
            ShellChoice::Zsh => clap_complete::Shell::Zsh,
            ShellChoice::Fish => clap_complete::Shell::Fish,
        }
    }
}

#[derive(Subcommand)]
pub enum QueryCommands {
    /// Check the status of a running query and retrieve results.
    /// Exit codes: 0 = succeeded, 1 = failed, 2 = still running (poll again)
    Status {
        /// Query run ID
        id: String,
    },
}

#[derive(Subcommand)]
pub enum AuthCommands {
    /// Log in via browser (same as `hotdata auth` with no subcommand)
    Login,

    /// Create a new account via browser (defaults to GitHub OAuth)
    Register {
        /// Sign up with email and password instead of GitHub
        #[arg(long)]
        email: bool,
    },

    /// Remove authentication for a profile
    Logout,

    /// Show authentication status
    Status,
}

#[derive(Subcommand)]
pub enum IndexesCommands {
    /// List indexes (defaults to the whole workspace; narrow with filters or pass --dataset-id)
    List {
        /// Filter by connection ID
        #[arg(long, short = 'c', conflicts_with = "dataset_id")]
        connection_id: Option<String>,

        /// Filter by schema name
        #[arg(long, conflicts_with = "dataset_id")]
        schema: Option<String>,

        /// Filter by table name
        #[arg(long, conflicts_with = "dataset_id")]
        table: Option<String>,

        /// List indexes for a specific dataset (alternative scope to --connection-id)
        #[arg(long)]
        dataset_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Create an index on a table or dataset.
    ///
    /// For connection-scoped indexes, pass the table and columns using bracket notation:
    ///   `connection.table[col1,col2]` or `connection.schema.table[col1,col2]`
    ///   (schema defaults to `public` when omitted)
    ///
    /// For dataset-scoped indexes, use `--dataset-id` with `--columns`.
    Create {
        /// Table and columns to index: `connection.table[col1,col2]`
        /// or `connection.schema.table[col1,col2]`. Schema defaults to `public`.
        ///
        /// Quote the argument to prevent shell glob expansion:
        /// `hotdata indexes create 'airbnb.listings[description]' --type bm25`
        #[arg(conflicts_with = "dataset_id")]
        target: Option<String>,

        /// Dataset ID (alternative scope to the positional target)
        #[arg(long, conflicts_with = "target")]
        dataset_id: Option<String>,

        /// Columns to index (comma-separated). Required with --dataset-id;
        /// for connection scope use bracket notation in the target instead.
        #[arg(long)]
        columns: Option<String>,

        /// Index name (derived from table, columns, and type if omitted)
        #[arg(long)]
        name: Option<String>,

        /// Index type — required (no default; choose deliberately)
        #[arg(long, value_parser = ["sorted", "bm25", "vector"])]
        r#type: String,

        /// Distance metric for vector indexes
        #[arg(long, value_parser = ["l2", "cosine", "dot"])]
        metric: Option<String>,

        /// Create as a background job
        #[arg(long)]
        r#async: bool,

        /// Embedding provider ID — when set on a vector index over a text column,
        /// embeddings are generated automatically. Defaults to first system provider if omitted.
        #[arg(long = "embedding-provider-id")]
        embedding_provider_id: Option<String>,

        /// Override embedding output dimensions (vector indexes with auto-embedding only)
        #[arg(long)]
        dimensions: Option<u32>,

        /// Custom name for the generated embedding column (defaults to `{column}_embedding`)
        #[arg(long = "output-column")]
        output_column: Option<String>,

        /// Human-readable description of the embedding (e.g. "product titles")
        #[arg(long)]
        description: Option<String>,
    },

    /// Delete an index from a table or dataset
    ///
    /// Pass either connection scope (--connection-id + --schema + --table) OR
    /// dataset scope (--dataset-id), not both.
    Delete {
        /// Connection ID (use with --schema and --table)
        #[arg(long, short = 'c', conflicts_with = "dataset_id", requires_all = ["schema", "table"])]
        connection_id: Option<String>,

        /// Schema name (requires --connection-id)
        #[arg(long, requires = "connection_id")]
        schema: Option<String>,

        /// Table name (requires --connection-id)
        #[arg(long, requires = "connection_id")]
        table: Option<String>,

        /// Dataset ID (alternative scope to --connection-id)
        #[arg(long, conflicts_with_all = ["connection_id", "schema", "table"])]
        dataset_id: Option<String>,

        /// Index name
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
pub enum JobsCommands {
    /// List background jobs (shows active jobs by default)
    List {
        /// Filter by job type
        #[arg(long, value_parser = ["data_refresh_table", "data_refresh_connection", "dataset_refresh", "create_index", "create_dataset_index"])]
        job_type: Option<String>,

        /// Filter by status
        #[arg(long, value_parser = ["pending", "running", "succeeded", "partially_succeeded", "failed"])]
        status: Option<String>,

        /// Show all jobs, not just active ones
        #[arg(long)]
        all: bool,

        /// Maximum number of results (default: 50)
        #[arg(long)]
        limit: Option<u32>,

        /// Pagination offset
        #[arg(long)]
        offset: Option<u32>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },
}

#[derive(Subcommand)]
pub enum DatasetsCommands {
    /// List all datasets in a workspace
    List {
        /// Maximum number of results (default: 100, max: 1000)
        #[arg(long)]
        limit: Option<u32>,

        /// Pagination offset
        #[arg(long)]
        offset: Option<u32>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Create a derived view from a SQL query or saved query
    Create {
        /// SQL table name the dataset is addressable as (e.g. my_view)
        #[arg(long)]
        name: String,

        /// Human-readable display label
        #[arg(long)]
        description: Option<String>,

        /// SQL query to create the dataset from
        #[arg(long, conflicts_with = "query_id", required_unless_present = "query_id")]
        sql: Option<String>,

        /// Saved query ID to create the dataset from
        #[arg(long, conflicts_with = "sql", required_unless_present = "sql")]
        query_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Update a dataset's description and/or name
    Update {
        /// Dataset ID
        id: String,

        /// New display label
        #[arg(long)]
        description: Option<String>,

        /// New SQL table name (must be a valid identifier)
        #[arg(long)]
        name: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Refresh a dataset by re-running its source (URL fetch or saved query) and creating a new version
    Refresh {
        /// Dataset ID
        id: String,

        /// Submit as a background job
        #[arg(long)]
        r#async: bool,
    },
}

#[derive(Subcommand)]
pub enum WorkspaceCommands {
    /// List all workspaces
    List {
        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Set the default workspace
    Set {
        /// Workspace ID to set as default (omit for interactive selection)
        workspace_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ConnectionsCreateCommands {
    /// List available connection types, or get details for a specific type
    List {
        /// Connection type name (e.g. postgres, mysql); omit to list all
        name: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },
}

#[derive(Subcommand)]
pub enum DatabasesCommands {
    /// List managed databases in the workspace
    List {
        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Show details for a specific managed database
    Show {
        /// Database name or ID
        name_or_id: String,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Create a new managed database
    Create {
        /// SQL catalog alias — becomes the catalog name in queries:
        /// SELECT … FROM <name>.public.<table>.
        /// Must be [a-z_][a-z0-9_]*, globally unique. When provided the
        /// database defaults to no expiry; omit for an anonymous 24h sandbox.
        #[arg(long)]
        name: Option<String>,

        /// Default schema for bare `--table` entries (default: public).
        /// Use dot notation in `--table` to target a different schema directly,
        /// e.g. `--table raw.raw_orders` always goes into the "raw" schema.
        #[arg(long, default_value = "public")]
        schema: String,

        /// Table to declare up front (repeatable). Accepts bare names or
        /// `schema.table` dot notation to span multiple schemas in one command:
        ///   --table orders --table raw.raw_orders --table raw.raw_customers
        #[arg(long = "table")]
        tables: Vec<String>,

        /// When the database expires. Accepts a relative duration (e.g. 24h, 7d, 90m)
        /// or an RFC 3339 timestamp. Omitting with --name means no expiry; omitting
        /// without --name defaults to 24h.
        #[arg(long)]
        expires_at: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Set the current database (used by default when no database is specified)
    Set {
        /// Database id
        id: String,
    },

    /// Delete a managed database and its tables
    Delete {
        /// Database name or connection ID
        name_or_id: String,
    },

    /// Load a parquet file into a table using dot notation: `database.table` or `database.schema.table`
    Load {
        /// Table to load into: `database.table` or `database.schema.table`.
        /// Schema defaults to `public` when omitted.
        target: String,

        /// Path to a local parquet file to upload and load
        #[arg(long, conflicts_with_all = ["upload_id", "url"])]
        file: Option<String>,

        /// URL of a remote parquet file to download and load
        #[arg(long, conflicts_with_all = ["file", "upload_id"])]
        url: Option<String>,

        /// Use a previously staged upload ID from `POST /v1/files` instead of uploading
        #[arg(long, conflicts_with_all = ["file", "url"])]
        upload_id: Option<String>,
    },

    /// Manage tables inside a managed database
    Tables {
        /// Database id or name — shorthand for `tables list` when no subcommand is given
        database: Option<String>,

        #[command(subcommand)]
        command: Option<DatabaseTablesCommands>,
    },

    /// Run a command with a database-scoped token. Creates a new database unless --database is given.
    Run {
        /// Existing database id to scope the token to (omit to auto-create a database)
        #[arg(long)]
        database: Option<String>,

        /// Description for the auto-created database (only used when --database is omitted)
        #[arg(long)]
        description: Option<String>,

        /// Schema for tables declared in the auto-created database (default: public)
        #[arg(long, default_value = "public")]
        schema: String,

        /// Table to declare in the auto-created database (repeatable)
        #[arg(long = "table")]
        tables: Vec<String>,

        /// When the auto-created database expires. Accepts a relative duration
        /// (e.g. 24h, 7d, 90m) or an RFC 3339 timestamp. Defaults to 24h when omitted.
        #[arg(long)]
        expires_at: Option<String>,

        /// Command to execute (everything after `--`)
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum DatabaseTablesCommands {
    /// List tables in a managed database
    List {
        /// Database id or name (defaults to current database)
        #[arg(long)]
        database: Option<String>,

        /// Filter by schema name
        #[arg(long)]
        schema: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Load a parquet file into a table (creates or replaces the table)
    Load {
        /// Database id or name (defaults to current database)
        #[arg(long)]
        database: Option<String>,

        /// Table name
        table: String,

        /// Schema name (default: public)
        #[arg(long, default_value = "public")]
        schema: String,

        /// Path to a local parquet file to upload and load
        #[arg(long, conflicts_with_all = ["upload_id", "url"])]
        file: Option<String>,

        /// URL of a remote parquet file to download and load
        #[arg(long, conflicts_with_all = ["file", "upload_id"])]
        url: Option<String>,

        /// Use a previously staged upload ID from `POST /v1/files` instead of uploading
        #[arg(long, conflicts_with_all = ["file", "url"])]
        upload_id: Option<String>,
    },

    /// Delete a table from a managed database
    Delete {
        /// Database id or name (defaults to current database)
        #[arg(long)]
        database: Option<String>,

        /// Table name
        table: String,

        /// Schema name (default: public)
        #[arg(long, default_value = "public")]
        schema: String,
    },
}

#[derive(Subcommand)]
pub enum ConnectionsCommands {
    /// Interactively create a new connection
    New,

    /// List all connections for a workspace
    List {
        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Create a new connection, or list/inspect available connection types
    Create {
        #[command(subcommand)]
        command: Option<ConnectionsCreateCommands>,

        /// Connection name
        #[arg(long)]
        name: Option<String>,

        /// Connection source type (e.g. postgres, mysql, snowflake)
        #[arg(long = "type")]
        source_type: Option<String>,

        /// Connection config as a JSON object
        #[arg(long)]
        config: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Refresh a connection's schema or data
    Refresh {
        /// Connection ID
        connection_id: String,

        /// Refresh data instead of schema metadata
        #[arg(long)]
        data: bool,

        /// Narrow refresh to a specific schema (requires --table for data refresh)
        #[arg(long)]
        schema: Option<String>,

        /// Narrow refresh to a specific table (requires --schema)
        #[arg(long)]
        table: Option<String>,

        /// Submit as a background job (only valid with --data)
        #[arg(long)]
        r#async: bool,

        /// Include uncached tables in connection-wide data refresh (only with --data, no --table)
        #[arg(long = "include-uncached")]
        include_uncached: bool,
    },
}

#[derive(Subcommand)]
pub enum SkillCommands {
    /// Install or update the hotdata skill into agent directories
    Install {
        /// Install into the current project directory instead of globally
        #[arg(long)]
        project: bool,
    },
    /// Show the installation status of the hotdata skill
    Status,
    /// List installed skills and their versions (alias for status)
    List,
}

#[derive(Subcommand)]
pub enum ResultsCommands {
    /// List stored query results
    List {
        /// Maximum number of results (default: 100, max: 1000)
        #[arg(long)]
        limit: Option<u32>,

        /// Pagination offset
        #[arg(long)]
        offset: Option<u32>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },
}

#[derive(Subcommand)]
pub enum QueriesCommands {
    /// List query runs
    List {
        /// Maximum number of results
        #[arg(long, default_value_t = 20)]
        limit: u32,

        /// Pagination cursor from a previous response
        #[arg(long)]
        cursor: Option<String>,

        /// Filter by status (comma-separated, e.g. running,failed)
        #[arg(long)]
        status: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },
}

#[derive(Subcommand)]
pub enum SandboxCommands {
    /// List all sandboxes in a workspace
    List {
        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Create a new sandbox and set it as active
    New {
        /// Sandbox name
        #[arg(long)]
        name: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Update a sandbox's markdown or name
    Update {
        /// Sandbox ID (defaults to active sandbox)
        id: Option<String>,

        /// New sandbox name
        #[arg(long)]
        name: Option<String>,

        /// Markdown content
        #[arg(long)]
        markdown: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Print the markdown content of the current sandbox
    Read,

    /// Set the active sandbox (omit ID to clear)
    Set {
        /// Sandbox ID to set as active (omit to clear)
        id: Option<String>,
    },

    /// Run a command inside a hotdata sandbox. Creates a new sandbox unless an ID was provided.
    /// Example: hotdata sandbox run claude
    /// Example: hotdata sandbox <id> run claude
    Run {
        /// Sandbox name (only used when creating a new sandbox)
        #[arg(long)]
        name: Option<String>,

        /// Command and arguments to execute
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },

    /// Delete a sandbox permanently
    Delete {
        /// Sandbox ID to delete
        id: String,
    },
}

#[derive(Subcommand)]
pub enum ContextCommands {
    /// List named contexts in the workspace
    List {
        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,

        /// Only include names starting with this prefix (case-sensitive)
        #[arg(long)]
        prefix: Option<String>,
    },

    /// Print context content to stdout
    Show {
        /// Context name (same rules as a SQL table identifier; local file is <NAME>.md). A trailing `.md` is ignored (e.g. `USER.md` → `USER`).
        name: String,
    },

    /// Download context from the database to ./<NAME>.md
    Pull {
        /// Context name (trailing `.md` ignored, e.g. `USER.md` → `USER`)
        name: String,

        /// Overwrite ./<NAME>.md if it already exists
        #[arg(long)]
        force: bool,

        /// Print the target path and size only; do not write a file
        #[arg(long)]
        dry_run: bool,
    },

    /// Upload ./<NAME>.md to the database as named context
    Push {
        /// Context name (trailing `.md` ignored, e.g. `USER.md` → `USER`; reads `./USER.md`)
        name: String,

        /// Print what would be sent; do not POST
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum TablesCommands {
    /// List all tables in a workspace
    List {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w')]
        workspace_id: Option<String>,

        /// Filter by connection ID (also enables column output)
        #[arg(long, short = 'c')]
        connection_id: Option<String>,

        /// Filter by schema name (supports % wildcards)
        #[arg(long)]
        schema: Option<String>,

        /// Filter by table name (supports % wildcards)
        #[arg(long)]
        table: Option<String>,

        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,

        /// Pagination cursor from a previous response
        #[arg(long)]
        cursor: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },
}

#[derive(Subcommand)]
pub enum EmbeddingProvidersCommands {
    /// List embedding providers
    List {
        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Show details for a specific embedding provider
    Get {
        /// Provider ID
        id: String,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Create a new embedding provider
    Create {
        /// Provider name (must be unique within the workspace)
        #[arg(long)]
        name: String,

        /// Provider type ("local" or "service")
        #[arg(long, value_parser = ["local", "service"])]
        provider_type: String,

        /// Provider-specific config as a JSON string (model, base_url, dimensions, etc.)
        #[arg(long)]
        config: Option<String>,

        /// The provider's own API key (e.g. an OpenAI sk-... key). Auto-creates a
        /// managed secret. Mutually exclusive with --secret-name. Named
        /// `--provider-api-key` to pair with `--provider-type` and to avoid colliding
        /// with the global `--api-key` (Hotdata auth) flag.
        #[arg(long = "provider-api-key", conflicts_with = "secret_name")]
        provider_api_key: Option<String>,

        /// Reference an existing secret by name (for service providers)
        #[arg(long)]
        secret_name: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Update an embedding provider's name, config, or secret
    Update {
        /// Provider ID
        id: String,

        /// New name
        #[arg(long)]
        name: Option<String>,

        /// New config as a JSON string
        #[arg(long)]
        config: Option<String>,

        /// New provider API key (replaces or creates the managed secret).
        /// See `embedding-providers create --provider-api-key` for naming rationale.
        #[arg(long = "provider-api-key", conflicts_with = "secret_name")]
        provider_api_key: Option<String>,

        /// New secret name to reference
        #[arg(long)]
        secret_name: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Delete an embedding provider
    Delete {
        /// Provider ID
        id: String,
    },
}
