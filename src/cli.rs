use crate::commands::auth::AuthCommands;
use crate::commands::databases::DatabasesCommands;
use crate::commands::ingest::IngestCommands;
use crate::commands::jobs::JobsCommands;
use crate::commands::query::QueryCommands;
use crate::commands::search::SearchCommands;
use crate::commands::skill::SkillCommands;
use crate::commands::workspace::WorkspaceCommands;
use clap::Subcommand;

// Variant sizes differ, but a clap command tree is parsed once per invocation
// and boxing subcommand variants would complicate the derive and match arms.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
pub enum Commands {
    /// Authenticate or manage auth settings
    Auth {
        #[command(subcommand)]
        command: Option<AuthCommands>,
    },

    /// Manage workspaces
    Workspaces {
        #[command(subcommand)]
        command: WorkspaceCommands,
    },

    /// Instant databases, plus the tables, queries, results, and context inside them
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

    /// Execute a SQL query, or check a running query (shortcut for `databases query`)
    Query {
        /// SQL query string (omit when using a subcommand)
        sql: Option<String>,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w')]
        workspace_id: Option<String>,

        /// Run against a specific instant database (defaults to the current database set via `databases use`)
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// SQL dialect the query is written in — a non-`hotsql` dialect is
        /// transpiled to HotSQL server-side before it runs (read-only only)
        #[arg(long, default_value = "hotsql", value_parser = ["hotsql", "duckdb", "postgres", "snowflake"])]
        dialect: String,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "csv"])]
        output: String,

        #[command(subcommand)]
        command: Option<QueryCommands>,
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

    /// Saved load definitions and the external sources they read
    ///
    /// Add the source first (`hotdata ingest sources add`). Selector and
    /// destination are fixed at creation; `pause` stops the current run AND
    /// future ones; `resume` never runs anything immediately. There is no
    /// `run`/`run-now` verb — use `ingest schedule <id> --next now`.
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

    /// Full-text and vector search — `search "text" --index <name>`, plus index management
    #[command(args_conflicts_with_subcommands = true)]
    Search {
        /// Text to search for (use with --index; omit when using a subcommand)
        query: Option<String>,

        /// Search index to query, by name (from `hotdata search list`)
        #[arg(long, visible_alias = "in")]
        index: Option<String>,

        /// Database the index lives in (id; defaults to the active database)
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Columns to display (comma-separated, defaults to all)
        #[arg(long)]
        select: Option<String>,

        /// Maximum number of results
        #[arg(long, default_value = "10")]
        limit: u32,

        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w', global = true)]
        workspace_id: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "csv"])]
        output: String,

        #[command(subcommand)]
        command: Option<SearchCommands>,
    },

    /// Account, configuration, and CLI maintenance
    Manage {
        #[command(subcommand)]
        command: ManageCommands,
    },
}

/// Subcommands for `hotdata manage` — account and CLI utilities.
#[derive(Subcommand)]
pub enum ManageCommands {
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

    /// Manage the hotdata agent skill
    Skills {
        #[command(subcommand)]
        command: SkillCommands,
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
