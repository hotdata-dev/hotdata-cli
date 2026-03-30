use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    /// Authenticate or manage auth settings
    Auth {
        #[command(subcommand)]
        command: Option<AuthCommands>,
    },

    /// Manage datasets
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

    /// Manage tables in a workspace
    Tables {
        #[command(subcommand)]
        command: TablesCommands,
    },

    /// Manage the hotdata-cli agent skill
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

    /// Full-text or vector search across a table column
    Search {
        /// Search query text (omit to read a vector from stdin for vector search)
        query: Option<String>,

        /// Table to search (connection.schema.table)
        #[arg(long)]
        table: String,

        /// Column to search
        #[arg(long)]
        column: String,

        /// Columns to display (comma-separated, defaults to all)
        #[arg(long)]
        select: Option<String>,

        /// Maximum number of results
        #[arg(long, default_value = "10")]
        limit: u32,

        /// Embedding model to generate a vector from the query text (e.g. text-embedding-3-small)
        #[arg(long, value_parser = ["text-embedding-3-small", "text-embedding-3-large"])]
        model: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w')]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "csv"])]
        output: String,
    },

    /// Manage saved queries
    Queries {
        /// Query ID to show details
        id: Option<String>,

        /// Output format (used with query ID)
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,

        #[command(subcommand)]
        command: Option<QueriesCommands>,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: ShellChoice,
    },
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
    /// Remove authentication for a profile
    Logout,

    /// Show authentication status
    Status,
}

#[derive(Subcommand)]
pub enum IndexesCommands {
    /// List indexes on a table
    List {
        /// Connection ID
        #[arg(long, short = 'c')]
        connection_id: String,

        /// Schema name
        #[arg(long)]
        schema: String,

        /// Table name
        #[arg(long)]
        table: String,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Create an index on a table
    Create {
        /// Connection ID
        #[arg(long, short = 'c')]
        connection_id: String,

        /// Schema name
        #[arg(long)]
        schema: String,

        /// Table name
        #[arg(long)]
        table: String,

        /// Index name
        #[arg(long)]
        name: String,

        /// Columns to index (comma-separated)
        #[arg(long)]
        columns: String,

        /// Index type
        #[arg(long, default_value = "sorted", value_parser = ["sorted", "bm25", "vector"])]
        r#type: String,

        /// Distance metric for vector indexes
        #[arg(long, value_parser = ["l2", "cosine", "dot"])]
        metric: Option<String>,

        /// Create as a background job
        #[arg(long)]
        r#async: bool,
    },
}

#[derive(Subcommand)]
pub enum JobsCommands {
    /// List background jobs (shows active jobs by default)
    List {
        /// Filter by job type
        #[arg(long, value_parser = ["data_refresh_table", "data_refresh_connection", "create_index"])]
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

    /// Create a new dataset from a file, piped stdin, upload ID, or SQL query
    Create {
        /// Dataset label (derived from filename if omitted)
        #[arg(long)]
        label: Option<String>,

        /// Table name (derived from label if omitted)
        #[arg(long)]
        table_name: Option<String>,

        /// Path to a file to upload (omit to read from stdin)
        #[arg(long, conflicts_with_all = ["upload_id", "sql"])]
        file: Option<String>,

        /// Skip upload and use a pre-existing upload ID directly
        #[arg(long, conflicts_with_all = ["file", "sql"])]
        upload_id: Option<String>,

        /// Source format when using --upload-id (csv, json, parquet)
        #[arg(long, default_value = "csv", value_parser = ["csv", "json", "parquet"], requires = "upload_id")]
        format: String,

        /// SQL query to create the dataset from
        #[arg(long, conflicts_with_all = ["file", "upload_id", "query_id", "url"])]
        sql: Option<String>,

        /// Saved query ID to create the dataset from
        #[arg(long, conflicts_with_all = ["file", "upload_id", "sql", "url"])]
        query_id: Option<String>,

        /// URL to import data from
        #[arg(long, conflicts_with_all = ["file", "upload_id", "sql", "query_id"])]
        url: Option<String>,
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

    /// Refresh a connection's schema
    Refresh {
        /// Connection ID
        connection_id: String,
    },
}

#[derive(Subcommand)]
pub enum SkillCommands {
    /// Install or update the hotdata-cli skill into agent directories
    Install {
        /// Install into the current project directory instead of globally
        #[arg(long)]
        project: bool,
    },
    /// Show the installation status of the hotdata-cli skill
    Status,
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
    /// List saved queries
    List {
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,

        /// Pagination offset
        #[arg(long)]
        offset: Option<u32>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Create a new saved query
    Create {
        /// Query name
        #[arg(long)]
        name: String,

        /// SQL query string
        #[arg(long)]
        sql: String,

        /// Query description
        #[arg(long)]
        description: Option<String>,

        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Execute a saved query
    Run {
        /// Saved query ID
        id: String,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "csv"])]
        output: String,
    },

    /// Update a saved query
    Update {
        /// Saved query ID
        id: String,

        /// New query name
        #[arg(long)]
        name: Option<String>,

        /// New SQL query string
        #[arg(long)]
        sql: Option<String>,

        /// New description
        #[arg(long)]
        description: Option<String>,

        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,

        /// Override the auto-detected category (pass empty string to clear)
        #[arg(long)]
        category: Option<String>,

        /// User annotation for table size (pass empty string to clear)
        #[arg(long)]
        table_size: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
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
