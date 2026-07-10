use crate::commands::auth::AuthCommands;
use crate::commands::context::ContextCommands;
use crate::commands::databases::DatabasesCommands;
use crate::commands::embedding_providers::EmbeddingProvidersCommands;
use crate::commands::indexes::IndexesCommands;
use crate::commands::ingest::IngestCommands;
use crate::commands::jobs::JobsCommands;
use crate::commands::queries::QueriesCommands;
use crate::commands::query::QueryCommands;
use crate::commands::results::ResultsCommands;
use crate::commands::skill::SkillCommands;
use crate::commands::tables::TablesCommands;
use crate::commands::workspace::WorkspaceCommands;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    /// Authenticate or manage auth settings
    Auth {
        #[command(subcommand)]
        command: Option<AuthCommands>,
    },

    /// Execute a SQL query, or check status of a running query
    Query {
        /// SQL query string (omit when using a subcommand)
        sql: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w')]
        workspace_id: Option<String>,

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

        /// Managed database to scope to (defaults to the current database set via `databases set`)
        #[arg(long, short = 'd', global = true)]
        database: Option<String>,

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

    /// Pull data from external sources (databases, APIs, buckets, Iceberg)
    /// into managed databases: datasources + imports
    Ingest {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"], global = true)]
        output: String,

        #[command(subcommand)]
        command: IngestCommands,
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

        /// Managed database to scope to (defaults to the current database set via `databases set`)
        #[arg(long, short = 'd', global = true)]
        database: Option<String>,

        /// Output format (used with query run ID)
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,

        #[command(subcommand)]
        command: Option<QueriesCommands>,
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

    /// Show workspace usage: queries, bytes scanned, and stored bytes
    Usage {
        /// Only count usage since this RFC 3339 timestamp (e.g. 2026-06-01T00:00:00Z); defaults to the current billing window
        #[arg(long)]
        since: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: ShellChoice,
    },

    /// Upgrade the hotdata CLI to the latest release
    Upgrade,
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
