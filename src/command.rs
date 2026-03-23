use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize HotData CLI configuration file with default settings
    Init,

    /// Show HotData service information
    Info,

    /// Manage authentication and profiles
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },

    /// Manage datasets
    Datasets {
        /// Dataset ID to show details
        id: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, global = true)]
        workspace_id: Option<String>,

        /// Output format (used with dataset ID)
        #[arg(long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,

        #[command(subcommand)]
        command: Option<DatasetsCommands>,
    },

    /// Execute a SQL query
    Query {
        /// SQL query string
        sql: String,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Scope query to a specific connection
        #[arg(long)]
        connection: Option<String>,

        /// Output format
        #[arg(long, default_value = "table", value_parser = ["table", "json", "csv"])]
        format: String,
    },

    /// Manage configuration profiles
    Profile {
        #[command(subcommand)]
        command: ProfileCommands,
    },

    /// Manage workspaces
    Workspaces {
        #[command(subcommand)]
        command: WorkspaceCommands,
    },

    /// Manage workspace connections
    Connections {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, global = true)]
        workspace_id: Option<String>,

        #[command(subcommand)]
        command: ConnectionsCommands,
    },

    /// Manage tables in a workspace
    Tables {
        #[command(subcommand)]
        command: TablesCommands,
    },

    /// Manage the hotdata-cli agent skill
    Skill {
        #[command(subcommand)]
        command: SkillCommands,
    },

    /// Retrieve a stored query result by ID
    Results {
        /// Result ID
        result_id: String,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long, default_value = "table", value_parser = ["table", "json", "csv"])]
        format: String,
    },
}

#[derive(Subcommand)]
pub enum AuthCommands {
    /// Log in to HotData via browser
    Login,

    /// Remove authentication for a profile
    Logout {
        /// Configuration profile name
        #[arg(long, default_value = "default")]
        profile: String,
    },

    /// Show authentication status
    Status {
        /// Configuration profile name
        #[arg(long, default_value = "default")]
        profile: String,
    },

    /// Update authentication configuration
    Config {
        /// API endpoint URL
        #[arg(long)]
        endpoint: Option<String>,

        /// Configuration profile name
        #[arg(long, default_value = "default")]
        profile: String,
    },

    /// Validate the API key for the active profile
    Validate,

    /// Manage API keys
    Keys {
        #[command(subcommand)]
        command: AuthKeysCommands,
    },
}

#[derive(Subcommand)]
pub enum AuthKeysCommands {
    /// Create a new API key for an organization
    Create {
        /// Organization ID
        #[arg(long)]
        org_id: String,

        /// API key (if not provided, a new one will be generated)
        #[arg(long)]
        key: Option<String>,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Delete an API key from an organization
    Delete {
        /// API key to delete
        api_key: String,

        /// Organization ID
        #[arg(long)]
        org_id: String,
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
        #[arg(long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
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
        #[arg(long, conflicts_with_all = ["file", "upload_id", "query_id"])]
        sql: Option<String>,

        /// Saved query ID to create the dataset from
        #[arg(long, conflicts_with_all = ["file", "upload_id", "sql"])]
        query_id: Option<String>,
    },
}


#[derive(Subcommand)]
pub enum ProfileCommands {
    /// List available configuration profiles
    List,

    /// Show the current active configuration profile
    Current,

    /// Set the active configuration profile for all future commands
    Set {
        /// Profile name to activate
        profile: String,
    },

    /// Remove a configuration profile
    Remove {
        /// Profile name to remove
        profile: String,
    },
}

#[derive(Subcommand)]
pub enum WorkspaceCommands {
    /// List all workspaces
    List {
        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Get details for a workspace
    Get {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Create a new workspace
    Create {
        /// Workspace name
        #[arg(long)]
        name: String,

        /// Workspace description
        #[arg(long, default_value = "")]
        description: String,

        /// Organization ID for the workspace
        #[arg(long)]
        organization_id: String,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Update an existing workspace
    Update {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// New workspace name
        #[arg(long)]
        name: Option<String>,

        /// New workspace description
        #[arg(long)]
        description: Option<String>,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },
}

#[derive(Subcommand)]
pub enum ConnectionsCreateCommands {
    /// List available connection types, or get details for a specific type
    List {
        /// Connection type name (e.g. postgres, mysql); omit to list all
        name: Option<String>,

        /// Output format
        #[arg(long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
    },
}

#[derive(Subcommand)]
pub enum ConnectionsCommands {
    /// Interactively create a new connection
    New,

    /// List all connections for a workspace
    List {
        /// Output format
        #[arg(long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Get details for a specific connection
    Get {
        /// Connection ID
        connection_id: String,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
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
        #[arg(long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Update a connection in a workspace
    Update {
        /// Connection ID
        connection_id: String,

        /// New connection name
        #[arg(long)]
        name: Option<String>,

        /// New connection type
        #[arg(long = "type")]
        conn_type: Option<String>,

        /// New connection config as JSON string
        #[arg(long)]
        config: Option<String>,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Delete a connection from a workspace
    Delete {
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
pub enum TablesCommands {
    /// List all tables in a workspace
    List {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Filter by connection ID (also enables column output)
        #[arg(long)]
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
        #[arg(long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
    },
}
