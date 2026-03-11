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
        #[command(subcommand)]
        command: DatasetsCommands,
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
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Get details for a specific dataset
    Get {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Dataset ID
        dataset_id: String,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Create a new dataset in a workspace
    Create {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Dataset name
        #[arg(long)]
        name: String,

        /// SQL query for the dataset
        #[arg(long)]
        sql: Option<String>,

        /// Connection ID for the dataset
        #[arg(long)]
        connection_id: Option<String>,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Update a dataset in a workspace
    Update {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Dataset ID
        dataset_id: String,

        /// New dataset name
        #[arg(long)]
        name: Option<String>,

        /// New SQL query for the dataset
        #[arg(long)]
        query: Option<String>,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Delete a dataset from a workspace
    Delete {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Dataset ID
        dataset_id: String,
    },

    /// Update the SQL query for a dataset
    UpdateSql {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Dataset ID
        dataset_id: String,

        /// New SQL query for the dataset
        #[arg(long)]
        sql: String,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Execute a dataset
    Execute {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Dataset ID
        dataset_id: String,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
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
pub enum ConnectionsCommands {
    /// List all connections for a workspace
    List {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long, default_value = "table", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Get details for a specific connection
    Get {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Connection ID
        connection_id: String,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Create a new connection in a workspace
    Create {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

        /// Connection name
        #[arg(long)]
        name: String,

        /// Connection type
        #[arg(long = "type")]
        conn_type: String,

        /// Connection config as JSON string
        #[arg(long)]
        config: String,

        /// Output format
        #[arg(long, default_value = "yaml", value_parser = ["table", "json", "yaml"])]
        format: String,
    },

    /// Update a connection in a workspace
    Update {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

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
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long)]
        workspace_id: Option<String>,

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
