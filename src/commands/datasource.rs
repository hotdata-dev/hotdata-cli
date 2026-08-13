//! `hotdata datasource` — reusable external source identities.
//!
//! A datasource is what a credential opens: a Postgres server, a bucket root, a
//! Kafka cluster, an Iceberg catalog. It is *not* the subset of data to load —
//! that is an ingest's selector (`hotdata ingest create`) — and it does not own
//! a destination.
//!
//! **Ids are the identity.** `display_name` is a label: it need not be unique
//! and nothing resolves against it. Every command here takes a `ds_…` id.
//!
//! **Config edits never fork the identity.** `update-config` appends an
//! immutable config version under the same `datasource_id` and moves the
//! current pointer; runs already in flight keep the version they snapshotted.
//! Source credential rotation is the same operation — there is no separate
//! credential verb, because credentials are family-specific and belong with the
//! config they authenticate.
//!
//! Credential semantics on `update-config` are three-valued and deliberate:
//!
//! ```text
//! (neither flag)      inherit the previous source secret refs
//! --credentials …     replace the source credential state
//! --no-credentials    no source credential (families with public sources)
//! ```
//!
//! **Presentation contract:** `validate` persists nothing and is the preflight
//! path; `create` validates again regardless. Secrets are never echoed back by
//! the server and never printed here.

use crate::client::ingest::{
    ConfigUpdate, ConnectorEntry, Datasource, DatasourceConfig, IngestClient,
};
use crate::commands::ingest_common::{
    cell, date_cell, empty_notice, fail, field, hint, parse_json_arg, render, with_spinner,
};
use crate::util;

/// The families the API discriminates config and selector shapes on. SQL
/// dialects (postgres, mysql, …) are `--family sql` with `config.dialect`;
/// buckets are `--family filesystem`; named API services are `--family rest`.
const FAMILIES: [&str; 7] = [
    "sql",
    "filesystem",
    "kafka",
    "iceberg",
    "delta",
    "ducklake",
    "rest",
];

#[derive(clap::Subcommand)]
pub enum DatasourceCommands {
    /// Check a config and credentials without creating anything
    ///
    /// Persists no datasource, config version, managed database, or secret —
    /// run it before `create` to see what the credentials can reach. The
    /// response carries family-specific discovery (schemas/tables, topics, …).
    Validate {
        /// Source family — the shape of --config. Use `sql` for any SQL
        /// dialect (the dialect goes in the config), `filesystem` for buckets.
        #[arg(long, value_parser = FAMILIES)]
        family: String,

        #[command(flatten)]
        payload: ConfigArgs,
    },

    /// Create a datasource and its first config version
    ///
    /// Returns a stable `ds_…` id — the argument every ingest takes. Loads no
    /// data: pull rows with `hotdata ingest create --datasource-id <id>`.
    Create {
        /// Source family — the shape of --config. Use `sql` for any SQL
        /// dialect (the dialect goes in the config), `filesystem` for buckets.
        #[arg(long, value_parser = FAMILIES)]
        family: String,

        /// Human label shown in listings. Not identity: it need not be unique
        /// and nothing resolves against it.
        #[arg(long = "display-name")]
        display_name: Option<String>,

        #[command(flatten)]
        payload: ConfigArgs,
    },

    /// List the datasources in this workspace
    List {
        /// Only this family
        #[arg(long, value_parser = FAMILIES)]
        family: Option<String>,

        /// Only this lifecycle state
        #[arg(long, value_parser = ["creating", "active", "failed", "deleted"])]
        state: Option<String>,

        /// Include soft-deleted datasources
        #[arg(long = "include-deleted")]
        include_deleted: bool,
    },

    /// Show one datasource: state, current config version, and discovery
    Show {
        /// Datasource id (from `hotdata datasource list`)
        datasource_id: String,
    },

    /// Append a config version and point the datasource at it
    ///
    /// Rotating source credentials is this command: pass the same config plus
    /// new `--credentials`. Existing runs keep the version they snapshotted,
    /// and a stopped ingest is NOT resumed by a config change.
    #[command(name = "update-config")]
    UpdateConfig {
        /// Datasource id (from `hotdata datasource list`)
        datasource_id: String,

        #[command(flatten)]
        payload: ConfigArgs,

        /// Drop the source credential entirely (public/no-auth sources).
        /// Omitting both credential flags inherits the previous secret refs.
        #[arg(long = "no-credentials", conflicts_with = "credentials")]
        no_credentials: bool,
    },

    /// Delete a datasource (its ingests must be deleted first)
    ///
    /// Soft-delete. Returns 409 while any non-deleted ingest references it —
    /// destination tables, their data, and managed databases are never touched.
    Delete {
        /// Datasource id (from `hotdata datasource list`)
        datasource_id: String,
    },

    /// Browse the catalog of source types and their config fields
    #[command(alias = "connectors")]
    Types {
        /// Filter to entries whose name contains this text
        name: Option<String>,
    },
}

/// The config payload flags, shared by `validate`, `create`, and
/// `update-config` so the accepted shapes cannot drift between them.
#[derive(clap::Args)]
pub struct ConfigArgs {
    /// Source config as JSON (inline, @file.json, or @- for stdin). Either a
    /// bare config object, or the envelope {"config": …, "credentials": …}.
    /// Field reference: `hotdata datasource types -o json` (config_schema).
    #[arg(long)]
    config: Option<String>,

    /// Source credentials as JSON (inline, @file.json, or @-). Wins over any
    /// `credentials` inside --config. Keep secrets out of argv with @file.
    #[arg(long)]
    credentials: Option<String>,
}

/// Entry point from `main`. Keeps `main.rs` thin — one call per group.
pub fn dispatch(workspace_id: &str, output: &str, command: DatasourceCommands) {
    match command {
        DatasourceCommands::Validate { family, payload } => {
            validate(workspace_id, output, &family, payload)
        }
        DatasourceCommands::Create {
            family,
            display_name,
            payload,
        } => create(workspace_id, output, &family, display_name, payload),
        DatasourceCommands::List {
            family,
            state,
            include_deleted,
        } => list(workspace_id, output, family, state, include_deleted),
        DatasourceCommands::Show { datasource_id } => show(workspace_id, output, &datasource_id),
        DatasourceCommands::UpdateConfig {
            datasource_id,
            payload,
            no_credentials,
        } => update_config(
            workspace_id,
            output,
            &datasource_id,
            payload,
            no_credentials,
        ),
        DatasourceCommands::Delete { datasource_id } => {
            delete(workspace_id, output, &datasource_id)
        }
        DatasourceCommands::Types { name } => types(workspace_id, output, name.as_deref()),
    }
}

// --- payload construction --------------------------------------------------

/// Split a `--config`/`--credentials` pair into the two wire fields.
///
/// Pure (the JSON is pre-parsed, errors are returned) so the envelope handling
/// and the three-valued credential semantics — the part a server-side 422 would
/// otherwise be the first to catch — are unit-testable.
///
/// `Ok((config, credentials))` where `credentials == None` means "omit the key"
/// (inherit on update) and `Some({})` means "explicitly no credential".
fn split_payload(
    config: Option<serde_json::Value>,
    credentials: Option<serde_json::Value>,
    no_credentials: bool,
) -> Result<(serde_json::Value, Option<serde_json::Value>), String> {
    let Some(config) = config else {
        return Err(
            "--config is required (inline JSON, @file.json, or @-). See the family's \
             config_schema in 'hotdata datasource types -o json'"
                .into(),
        );
    };
    if !config.is_object() {
        return Err("--config must be a JSON object".into());
    }
    // The documented source.json is an envelope carrying both halves; a bare
    // config object is accepted too, so `--config '{"dialect":"postgres"}'`
    // works without ceremony.
    let (config, enveloped_credentials) = match config.get("config") {
        Some(inner) if inner.is_object() => (inner.clone(), config.get("credentials").cloned()),
        _ => (config, None),
    };

    let credentials = if no_credentials {
        // Explicitly empty ≠ omitted: this asks the service to create a config
        // version with no source secret refs.
        Some(serde_json::json!({}))
    } else {
        credentials.or(enveloped_credentials)
    };
    if let Some(c) = &credentials
        && !c.is_object()
    {
        return Err("--credentials must be a JSON object".into());
    }
    Ok((config, credentials))
}

impl ConfigArgs {
    fn parse(self) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
        (
            self.config
                .as_deref()
                .map(|a| parse_json_arg("--config", a)),
            self.credentials
                .as_deref()
                .map(|a| parse_json_arg("--credentials", a)),
        )
    }
}

// --- validate ---------------------------------------------------------------

fn validate(workspace_id: &str, output: &str, family: &str, payload: ConfigArgs) {
    let (config, credentials) = payload.parse();
    let (config, credentials) =
        split_payload(config, credentials, false).unwrap_or_else(|m| fail(&m));
    let req = DatasourceConfig {
        family: family.to_string(),
        display_name: None,
        config,
        credentials,
    };

    let client = IngestClient::new(workspace_id);
    let resp = with_spinner("checking the source…", || {
        client.validate_datasource(&req)
    });

    render(output, &resp, || {
        use crossterm::style::Stylize;
        if resp.valid {
            field("valid:", &"yes".green().to_string());
        } else {
            field("valid:", &"no".red().to_string());
        }
        field("family:", &cell(resp.family.as_deref()));
        if let Some(d) = resp.detail.as_deref().filter(|d| !d.trim().is_empty()) {
            field("detail:", d);
        }
        for line in discovered_lines(resp.discovered.as_ref()) {
            println!("{line}");
        }
        if resp.valid {
            hint(&format!(
                "Nothing was created. Persist it with: hotdata datasource create --family {family} --config @source.json"
            ));
        }
    });
    if !resp.valid {
        std::process::exit(1);
    }
}

// --- create -----------------------------------------------------------------

fn create(
    workspace_id: &str,
    output: &str,
    family: &str,
    display_name: Option<String>,
    payload: ConfigArgs,
) {
    let (config, credentials) = payload.parse();
    let (config, credentials) =
        split_payload(config, credentials, false).unwrap_or_else(|m| fail(&m));
    let req = DatasourceConfig {
        family: family.to_string(),
        display_name,
        config,
        credentials,
    };

    let client = IngestClient::new(workspace_id);
    // The first datasource in a workspace provisions the runtime (~15-30s);
    // later ones are quick. The HTTP client allows 300s.
    let ds = with_spinner(
        "creating datasource… (the first one in a workspace takes ~30s)",
        || client.create_datasource(&req),
    );

    render(output, &ds, || {
        use crossterm::style::Stylize;
        println!("{}", "datasource created".green());
        print_datasource_identity(&ds);
        hint(&format!(
            "Load data with: hotdata ingest create --datasource-id {} --type one-time \
             --selector @selector.json --destination @destination.json",
            ds.datasource_id
        ));
    });
}

// --- list -------------------------------------------------------------------

fn list(
    workspace_id: &str,
    output: &str,
    family: Option<String>,
    state: Option<String>,
    include_deleted: bool,
) {
    let mut filters: Vec<(&str, String)> = Vec::new();
    if let Some(f) = family {
        filters.push(("family", f));
    }
    if let Some(s) = state {
        filters.push(("state", s));
    }
    if include_deleted {
        filters.push(("include_deleted", "true".into()));
    }

    let client = IngestClient::new(workspace_id);
    let resp = with_spinner("loading datasources…", || {
        client.list_datasources(&filters)
    });

    render(output, &resp.datasources, || {
        if resp.datasources.is_empty() {
            empty_notice(
                "No datasources yet. Add one with 'hotdata datasource create --family <f> \
                 --config @source.json'.",
            );
            return;
        }
        let rows: Vec<Vec<String>> = resp
            .datasources
            .iter()
            // Oldest at the top, newest at the bottom — the freshest row lands
            // next to the prompt. (The server returns newest-first; json/yaml
            // keep that order for scripting.)
            .rev()
            .map(|d| {
                vec![
                    cell(d.display_name.as_deref()),
                    cell(d.family.as_deref()),
                    d.state
                        .as_deref()
                        .map(util::color_status)
                        .unwrap_or_else(|| "-".into()),
                    date_cell(d.created_at.as_deref()),
                    d.datasource_id.clone(),
                ]
            })
            .collect();
        crate::output::table::print(
            &[
                "DISPLAY NAME",
                "FAMILY",
                "STATE",
                "CREATED",
                "DATASOURCE ID",
            ],
            &rows,
        );
    });
}

// --- show -------------------------------------------------------------------

fn show(workspace_id: &str, output: &str, datasource_id: &str) {
    let client = IngestClient::new(workspace_id);
    let ds = client
        .get_datasource(datasource_id)
        .unwrap_or_else(|e| e.exit());

    render(output, &ds, || {
        print_datasource_identity(&ds);
        if let Some(d) = ds.detail.as_deref().filter(|d| !d.trim().is_empty()) {
            field("detail:", d);
        }
        if let Some(t) = ds.created_at.as_deref() {
            field("created:", &util::format_date(t));
        }
        if let Some(t) = ds.updated_at.as_deref() {
            field("updated:", &util::format_date(t));
        }
        if let Some(c) = ds.config.as_ref() {
            field("config:", &compact_json(c));
        }
        for line in discovered_lines(ds.discovered.as_ref()) {
            println!("{line}");
        }
        hint(&format!(
            "Its ingests: hotdata ingest list --datasource-id {}",
            ds.datasource_id
        ));
    });
}

/// The identity block every datasource view opens with. One definition so
/// `create` and `show` cannot disagree about what a datasource *is*.
fn print_datasource_identity(ds: &Datasource) {
    field("datasource id:", &ds.datasource_id);
    field("family:", &cell(ds.family.as_deref()));
    field("display name:", &cell(ds.display_name.as_deref()));
    field(
        "state:",
        &ds.state
            .as_deref()
            .map(util::color_status)
            .unwrap_or_else(|| "-".into()),
    );
    if let Some(v) = ds.current_config_version_id.as_deref() {
        let versioned = match ds.version {
            Some(n) => format!("{v} (v{n})"),
            None => v.to_string(),
        };
        field("config version:", &versioned);
    }
}

// --- update-config ----------------------------------------------------------

fn update_config(
    workspace_id: &str,
    output: &str,
    datasource_id: &str,
    payload: ConfigArgs,
    no_credentials: bool,
) {
    let (config, credentials) = payload.parse();
    let (config, credentials) =
        split_payload(config, credentials, no_credentials).unwrap_or_else(|m| fail(&m));
    let req = ConfigUpdate {
        config,
        credentials,
    };

    let client = IngestClient::new(workspace_id);
    let ack = with_spinner("appending config version…", || {
        client.update_datasource_config(datasource_id, &req)
    });

    render(output, &ack, || {
        use crossterm::style::Stylize;
        println!("{}", "config version appended".green());
        field("datasource id:", &ack.datasource_id);
        if let Some(p) = ack.previous_config_version_id.as_deref() {
            field("previous:", p);
        }
        if let Some(c) = ack.current_config_version_id.as_deref() {
            let versioned = match ack.version {
                Some(n) => format!("{c} (v{n})"),
                None => c.to_string(),
            };
            field("current:", &versioned);
        }
        field(
            "state:",
            &ack.state
                .as_deref()
                .map(util::color_status)
                .unwrap_or_else(|| "-".into()),
        );
        hint(
            "Runs already in flight keep the config version they snapshotted. \
             Stopped ingests stay stopped — resume them explicitly.",
        );
    });
}

// --- delete -----------------------------------------------------------------

fn delete(workspace_id: &str, output: &str, datasource_id: &str) {
    let client = IngestClient::new(workspace_id);
    let ack = with_spinner("deleting datasource…", || {
        client.delete_datasource(datasource_id)
    });

    render(output, &ack, || {
        use crossterm::style::Stylize;
        println!(
            "{} {}",
            "datasource deleted".green(),
            datasource_id.dark_grey()
        );
    });
}

// --- types (the catalog) ----------------------------------------------------

fn types(workspace_id: &str, output: &str, filter: Option<&str>) {
    let client = IngestClient::new(workspace_id);
    let mut entries = with_spinner("loading source types…", || client.connectors()).connectors;
    if let Some(f) = filter {
        let f = f.to_lowercase();
        entries.retain(|c| c.name.to_lowercase().contains(&f));
    }
    let entries = sorted_for_display(&entries);

    let projected: Vec<_> = entries
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "family": c.family,
                "description": c.description,
                "config_schema": c.config_schema,
            })
        })
        .collect();
    render(output, &projected, || {
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|c| vec![c.name.clone(), c.family.clone(), c.description.clone()])
            .collect();
        crate::output::table::print(&["NAME", "FAMILY", "DESCRIPTION"], &rows);
        hint(
            "The FAMILY column is what --family takes. \
             'hotdata datasource types -o json' shows each entry's config_schema.",
        );
    });
}

fn family_rank(family: &str) -> u8 {
    match family {
        "sql" => 0,
        "filesystem" => 1,
        "iceberg" => 2,
        "delta" => 3,
        "ducklake" => 4,
        "kafka" => 5,
        _ => 6, // rest services
    }
}

/// Sort the catalog for display: generic families first, then the REST
/// services, each group alphabetical. Redundant SQL dialect aliases are
/// collapsed at the source (the catalog), not here.
fn sorted_for_display(entries: &[ConnectorEntry]) -> Vec<ConnectorEntry> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| {
        family_rank(&a.family)
            .cmp(&family_rank(&b.family))
            .then_with(|| a.name.cmp(&b.name))
    });
    sorted
}

// --- discovery rendering ----------------------------------------------------

/// Human lines for a family-specific `discovered` blob. Pure so the shapes it
/// understands are pinned by tests rather than by whatever the server last
/// returned.
///
/// Understood shapes: `{"tables": [{"schema","table"}| "name", …]}`,
/// `{"schemas": […]}`, and any other object, which is printed compactly rather
/// than dropped.
fn discovered_lines(discovered: Option<&serde_json::Value>) -> Vec<String> {
    use crossterm::style::Stylize;
    let Some(d) = discovered.filter(|d| !d.is_null()) else {
        return Vec::new();
    };
    let mut lines = Vec::new();

    if let Some(schemas) = d.get("schemas").and_then(|s| s.as_array())
        && !schemas.is_empty()
    {
        let names: Vec<String> = schemas.iter().map(json_scalar).collect();
        lines.push(format!(
            "{}{}",
            format!("{:<16}", "schemas:").dark_grey(),
            names.join(", ")
        ));
    }

    match d.get("tables").and_then(|t| t.as_array()) {
        Some(tables) if !tables.is_empty() => {
            lines.push(
                format!("discovered {} table(s):", tables.len())
                    .dark_grey()
                    .to_string(),
            );
            for t in tables {
                let name = match (t.get("schema").and_then(|v| v.as_str()), t.get("table")) {
                    (Some(s), Some(tbl)) => format!("{s}.{}", json_scalar(tbl)),
                    _ => json_scalar(t),
                };
                let columns = t
                    .get("columns")
                    .and_then(|c| c.as_array())
                    .map(|c| c.iter().map(json_scalar).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                lines.push(format!("  {}  {}", name.cyan(), columns.dark_grey()));
            }
            return lines;
        }
        _ => {}
    }

    // Anything else the family reports still reaches the user, compactly —
    // dropping an unrecognized shape would silently hide discovery output.
    if d.get("schemas").is_none()
        && let Some(obj) = d.as_object()
        && !obj.is_empty()
    {
        lines.push(format!(
            "{}{}",
            format!("{:<16}", "discovered:").dark_grey(),
            compact_json(d)
        ));
    }
    lines
}

/// A JSON value as a bare display string: strings unquoted, everything else
/// compact — so `"orders"` prints as `orders`, not `"orders"`.
fn json_scalar(v: &serde_json::Value) -> String {
    v.as_str()
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string())
}

/// One-line JSON for a detail-view value.
fn compact_json(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "-".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_accepts_the_documented_envelope() {
        // source.json as the design doc writes it: both halves in one file.
        let source = serde_json::json!({
            "config": {"dialect": "postgres", "host": "pg.example.com"},
            "credentials": {"username": "reader", "password": "s3cret"},
        });
        let (config, credentials) = split_payload(Some(source), None, false).unwrap();
        assert_eq!(config["dialect"], "postgres");
        assert!(
            config.get("credentials").is_none(),
            "envelope must be split"
        );
        assert_eq!(credentials.unwrap()["username"], "reader");
    }

    #[test]
    fn payload_accepts_a_bare_config_object() {
        let bare = serde_json::json!({"provider": "s3", "root_uri": "s3://events"});
        let (config, credentials) = split_payload(Some(bare), None, false).unwrap();
        assert_eq!(config["root_uri"], "s3://events");
        // Omitted, not empty — the two mean different things on the wire.
        assert!(credentials.is_none());
    }

    #[test]
    fn explicit_credentials_flag_wins_over_the_envelope() {
        let source = serde_json::json!({
            "config": {"dialect": "postgres"},
            "credentials": {"password": "old"},
        });
        let flag = serde_json::json!({"password": "new"});
        let (_, credentials) = split_payload(Some(source), Some(flag), false).unwrap();
        assert_eq!(credentials.unwrap()["password"], "new");
    }

    #[test]
    fn no_credentials_sends_an_explicitly_empty_object() {
        // Three-valued: omitted inherits, `{}` drops the credential. A truthy
        // check would collapse the two.
        let bare = serde_json::json!({"provider": "s3", "root_uri": "s3://public"});
        let (_, credentials) = split_payload(Some(bare), None, true).unwrap();
        assert_eq!(credentials, Some(serde_json::json!({})));
    }

    #[test]
    fn payload_requires_config_and_rejects_non_objects() {
        assert!(
            split_payload(None, None, false)
                .unwrap_err()
                .contains("--config")
        );
        assert!(
            split_payload(Some(serde_json::json!("nope")), None, false)
                .unwrap_err()
                .contains("JSON object")
        );
        assert!(
            split_payload(
                Some(serde_json::json!({"a": 1})),
                Some(serde_json::json!([])),
                false
            )
            .unwrap_err()
            .contains("--credentials")
        );
    }

    #[test]
    fn discovered_lines_render_schema_qualified_tables() {
        let d = serde_json::json!({
            "schemas": ["public", "billing"],
            "tables": [
                {"schema": "public", "table": "orders", "columns": ["id", "status"]},
                {"schema": "public", "table": "customers"}
            ],
        });
        let lines = discovered_lines(Some(&d));
        let joined = strip_ansi(&lines.join("\n"));
        assert!(joined.contains("public, billing"), "{joined}");
        assert!(joined.contains("discovered 2 table(s):"), "{joined}");
        assert!(joined.contains("public.orders"), "{joined}");
        assert!(joined.contains("id, status"), "{joined}");
    }

    #[test]
    fn discovered_lines_survive_an_unrecognized_family_shape() {
        // A kafka/iceberg blob must still reach the user rather than vanish.
        let d = serde_json::json!({"topics": ["orders", "events"]});
        let joined = strip_ansi(&discovered_lines(Some(&d)).join("\n"));
        assert!(joined.contains("orders"), "{joined}");
        // Nothing to say when there is nothing to show.
        assert!(discovered_lines(None).is_empty());
        assert!(discovered_lines(Some(&serde_json::Value::Null)).is_empty());
        assert!(discovered_lines(Some(&serde_json::json!({}))).is_empty());
    }

    #[test]
    fn catalog_sorts_generic_families_before_rest_services() {
        let entries = vec![
            entry("stripe", "rest"),
            entry("postgres", "sql"),
            entry("buckets", "filesystem"),
            entry("aikido", "rest"),
            entry("iceberg", "iceberg"),
        ];
        let names: Vec<String> = sorted_for_display(&entries)
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            names,
            vec!["postgres", "buckets", "iceberg", "aikido", "stripe"]
        );
    }

    fn entry(name: &str, family: &str) -> ConnectorEntry {
        ConnectorEntry {
            name: name.into(),
            family: family.into(),
            description: String::new(),
            auth: None,
            template: None,
            config_schema: None,
        }
    }

    /// The rendering helpers colorize; assertions care about the text.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for c in chars.by_ref() {
                    if c == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }
}
