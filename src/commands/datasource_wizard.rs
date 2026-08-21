//! The guided `hotdata ingest sources add` flow: pick a source type, answer for
//! its fields, get a datasource.
//!
//! **The questions come from the service.** Labels, help text, accepted values,
//! defaults and — through `format: password` — which answers are hidden are all
//! read off the family's generated field reference, so a field the API renames,
//! drops, or stops accepting a value for changes the prompts without a CLI
//! release. `schema_form` holds that machinery.
//!
//! **The flow does not.** A schema is an unordered bag of fields: it cannot say
//! that the bucket URL comes before the credentials it decides the shape of,
//! that a Snowflake account belongs beside its warehouse, or that a SASL
//! password is worth asking for only once a mechanism has been named. Those are
//! written by hand below, one per family, and are the reason this module is not
//! a loop over `properties`. Two rules keep the hand-written half from outliving
//! the API: a question for a field the schema does not describe is dropped, and
//! every required field the flow did not name is asked anyway.
//!
//! **Nothing here is reached without a terminal.** `--no-input`, CI, and a
//! non-TTY stdin all skip the wizard entirely (see `util::is_interactive`), so
//! a script gets the same `--config` requirement it always had.

use crate::client::ingest::{ConnectorEntry, FamilyReference, IngestClient};
use crate::commands::ingest_common::{fail, with_spinner};
use crate::commands::prompt;
use crate::commands::schema_form::Form;

/// What the wizard collected — the same three fields `--family`,
/// `--display-name` and `--config`/`--credentials` would have carried.
pub struct Answers {
    pub family: String,
    pub display_name: Option<String>,
    pub config: serde_json::Value,
    /// `None` leaves the key off the request: a public bucket or an
    /// ambient-credential catalog is a source with no credential, which is a
    /// different request from an empty one.
    pub credentials: Option<serde_json::Value>,
}

/// Run the flow. `family` skips the catalog menu when the caller already named
/// one; `display_name` skips the closing question.
pub fn run(client: &IngestClient, family: Option<&str>, display_name: Option<String>) -> Answers {
    let entry = match family {
        // A named family is not a catalog entry, so it carries no presets: the
        // dialect, catalog flavour and format questions the entry would have
        // answered are asked instead.
        Some(f) => ConnectorEntry {
            name: f.to_string(),
            family: f.to_string(),
            ..Default::default()
        },
        None => choose_connector(client),
    };
    let reference = with_spinner("loading the field reference…", || {
        client.family(&entry.family)
    });

    println!();
    let (config, credentials) = build(&entry, &reference);

    let display_name = display_name.or_else(|| {
        let answer = prompt::text(
            "Display name:",
            Some("A label for listings — not identity, and never resolved against."),
            Some(&entry.name),
        );
        let answer = answer.trim();
        (!answer.is_empty()).then(|| answer.to_string())
    });

    Answers {
        family: entry.family,
        display_name,
        config,
        credentials,
    }
}

/// The catalog as a filterable menu. `inquire` narrows the ~170 entries as the
/// user types, and the ordering is the one `datasource types` prints — generic
/// families first, then the API services.
fn choose_connector(client: &IngestClient) -> ConnectorEntry {
    let entries = with_spinner("loading source types…", || client.connectors()).connectors;
    let entries = crate::commands::datasource::sorted_for_display(&entries);
    if entries.is_empty() {
        fail("the service reported no source types");
    }
    let labels: Vec<String> = entries
        .iter()
        .map(|c| {
            if c.description.is_empty() {
                format!("{}  ({})", c.name, c.family)
            } else {
                format!("{}  ({}) — {}", c.name, c.family, c.description)
            }
        })
        .collect();
    let index = prompt::select_index("Source:", Some("type to filter"), &labels);
    entries[index].clone()
}

/// Ask a family's questions, in the order that family is best asked in.
fn build(
    entry: &ConnectorEntry,
    reference: &FamilyReference,
) -> (serde_json::Value, Option<serde_json::Value>) {
    let mut config = Form::new(&reference.config_schema);
    let mut credentials = Form::new(&reference.credentials_schema);

    match entry.family.as_str() {
        "sql" => sql(entry, &mut config, &mut credentials),
        "filesystem" | "delta" => object_store(entry, &mut config, &mut credentials),
        "iceberg" => iceberg(entry, &mut config, &mut credentials),
        "ducklake" => ducklake(entry, &mut config, &mut credentials),
        "kafka" => kafka(entry, &mut config, &mut credentials),
        "rest" => rest(entry, &mut config, &mut credentials),
        // A family added after this build still works: it has no authored
        // order, so it gets its required fields asked from the schema alone.
        // Worse than a written flow, and far better than "unknown family".
        _ => {}
    }

    config.ask_remaining_required();
    credentials.ask_remaining_required();
    let credentials = (!credentials.is_empty()).then(|| credentials.finish());
    (config.finish(), credentials)
}

// --- sql ---------------------------------------------------------------------

/// Engines addressed by an account or a file path rather than a host:port pair.
/// Asking a MotherDuck user for a hostname is asking for a field their engine
/// has no equivalent of, and the schema cannot say so — `host` is optional for
/// every dialect because it is required for most of them.
const HOSTLESS_DIALECTS: [&str; 4] = ["bigquery", "motherduck", "duckdb", "snowflake"];

/// A DSN resolving to an in-process engine would stand up an embedded database
/// inside the loader, so the service refuses one; offering the choice for those
/// dialects would be offering a request that is rejected on arrival.
const NO_DSN_DIALECTS: [&str; 2] = ["duckdb", "motherduck"];

/// Well-known default ports. Engine facts rather than service facts — they are
/// the same number this year as last, and the schema has no `default` for
/// `port` because the right one depends on the dialect the caller just picked.
fn default_port(dialect: &str) -> Option<&'static str> {
    match dialect {
        "postgres" | "postgresql" | "redshift" => Some("5432"),
        "mysql" | "mariadb" => Some("3306"),
        "mssql" | "sqlserver" => Some("1433"),
        "oracle" => Some("1521"),
        _ => None,
    }
}

fn sql(entry: &ConnectorEntry, config: &mut Form, credentials: &mut Form) {
    // For this family the catalog entry's NAME is the dialect.
    if config.accepts("dialect", &entry.name) {
        config.set("dialect", entry.name.clone().into());
    } else {
        config.ask("dialect");
    }
    let dialect = config.get_str("dialect").unwrap_or_default();

    // The DSN question comes first because it decides whether the connection
    // fields are worth asking at all: a connection string carries host, port,
    // database and the credential in one value.
    if credentials.has("connection_string") && !NO_DSN_DIALECTS.contains(&dialect.as_str()) {
        let choice = prompt::select_index(
            "Connect with:",
            None,
            &[
                "Host, user and password".to_string(),
                "Connection string (DSN)".to_string(),
            ],
        );
        if choice == 1 {
            credentials.ask_required("connection_string");
            config.ask_map("options", entry.options_hint.as_ref());
            return;
        }
    }

    if !HOSTLESS_DIALECTS.contains(&dialect.as_str()) {
        config.ask("host");
        config.ask_with_default("port", default_port(&dialect));
    }
    config.ask("database");
    config.ask("default_schema");
    // The engine-specific knobs — Snowflake's account, Databricks' http_path —
    // live in the free-form map, and the catalog says which keys this entry
    // wants there.
    config.ask_map("options", entry.options_hint.as_ref());

    // Which secret authenticates is a property of the engine, not of the
    // family: every one of these is an optional field on one shared schema.
    match dialect.as_str() {
        "bigquery" => credentials.ask_required("credentials_json"),
        "motherduck" => credentials.ask_required("motherduck_token"),
        "databricks" => credentials.ask_required("access_token"),
        _ => {
            credentials.ask("username");
            credentials.ask("password");
        }
    }
}

// --- filesystem and delta ----------------------------------------------------

/// The bucket-backed families. Both open a storage root with one credential;
/// which credential depends on the provider, which the root URI already names.
fn object_store(entry: &ConnectorEntry, config: &mut Form, credentials: &mut Form) {
    config.ask("root_uri");
    let root = config.get_str("root_uri").unwrap_or_default();
    // Asking for the provider after the URI that spells it out is asking the
    // user to repeat themselves — and to get it wrong, since `s3://` and `s3`
    // are not the same string.
    match provider_of(&root) {
        Some(p) if config.accepts("provider", p) => config.set("provider", p.into()),
        _ => config.ask("provider"),
    }
    let _ = entry;
    object_store_credentials(config.get_str("provider").as_deref(), credentials);
}

/// The provider a storage URI names, or `None` for a scheme this build does not
/// recognise — in which case the question is asked rather than guessed.
///
/// Shared with `--bucket-url`, so the flag and the guided flow cannot disagree
/// about what `s3://` is.
pub fn provider_of(root_uri: &str) -> Option<&'static str> {
    let scheme = root_uri.split("://").next().unwrap_or("").to_lowercase();
    match scheme.as_str() {
        "s3" | "s3a" => Some("s3"),
        "gs" | "gcs" => Some("gs"),
        "az" | "abfs" | "abfss" | "azure" => Some("az"),
        "file" => Some("file"),
        // A bare path is a local root; anything else is a scheme the CLI has
        // no mapping for.
        _ if !root_uri.contains("://") && !root_uri.is_empty() => Some("file"),
        _ => None,
    }
}

/// The credential a storage provider takes. Every one of these is optional on
/// the schema, because a public bucket and an ambient instance role are both
/// legitimate — so the prompts say that blank is an answer.
fn object_store_credentials(provider: Option<&str>, credentials: &mut Form) {
    let public = "Leave blank for a public bucket or an ambient role.";
    match provider {
        Some("s3") => {
            credentials.ask_hinted("aws_access_key_id", public);
            credentials.ask("aws_secret_access_key");
        }
        Some("gs") => credentials.ask_hinted("gs_token", public),
        Some("az") => {
            credentials.ask_hinted("azure_storage_account_name", public);
            credentials.ask("azure_storage_account_key");
        }
        // A local path has nothing to authenticate against.
        Some("file") => {}
        _ => {
            credentials.ask_hinted("aws_access_key_id", public);
            credentials.ask("aws_secret_access_key");
        }
    }
}

// --- iceberg -----------------------------------------------------------------

fn iceberg(entry: &ConnectorEntry, config: &mut Form, credentials: &mut Form) {
    match entry.catalog_type.as_deref() {
        Some(t) if config.accepts("catalog_type", t) => config.set("catalog_type", t.into()),
        _ => config.ask("catalog_type"),
    }
    config.ask("catalog_name");
    config.ask_map("catalog_config", entry.options_hint.as_ref());

    // Glue authenticates through the AWS chain, so its keys are the exception
    // rather than the question; a REST catalog needs a token or an OAuth pair,
    // and asking for all four would have three of them answered blank.
    if config.get_str("catalog_type").as_deref() == Some("glue") {
        credentials.ask_hinted(
            "aws_access_key_id",
            "Leave blank to use the ambient AWS credentials.",
        );
        credentials.ask("aws_secret_access_key");
        return;
    }
    let choice = prompt::select_index(
        "Catalog auth:",
        None,
        &[
            "Bearer token".to_string(),
            "OAuth client credentials".to_string(),
            "None".to_string(),
        ],
    );
    match choice {
        0 => credentials.ask_required("token"),
        1 => credentials.ask_required("credential"),
        _ => {}
    }
}

// --- ducklake ----------------------------------------------------------------

fn ducklake(entry: &ConnectorEntry, config: &mut Form, credentials: &mut Form) {
    config.ask("catalog");
    config.ask_map("storage", entry.options_hint.as_ref());
    credentials.ask_hinted("catalog_password", "Leave blank if the catalog needs none.");
    credentials.ask_hinted(
        "aws_access_key_id",
        "For the object store holding the data files. Blank for an ambient role.",
    );
    credentials.ask("aws_secret_access_key");
}

// --- kafka -------------------------------------------------------------------

fn kafka(entry: &ConnectorEntry, config: &mut Form, credentials: &mut Form) {
    config.ask("bootstrap_servers");
    match entry.connector_type.as_deref() {
        Some(t) if config.accepts("connector_type", t) => config.set("connector_type", t.into()),
        _ => config.ask("connector_type"),
    }
    config.ask_hinted(
        "security_protocol",
        "e.g. SASL_SSL. Blank for a plaintext broker.",
    );
    config.ask_hinted("sasl_mechanism", "e.g. PLAIN or SCRAM-SHA-256.");
    config.ask("group_id_prefix");
    // A SASL user and password mean nothing to a cluster reached over
    // PLAINTEXT, and a broker that wants them has just been named a mechanism.
    if config.get_str("sasl_mechanism").is_some() {
        credentials.ask_required("sasl_username");
        credentials.ask_required("sasl_password");
    }
}

// --- rest --------------------------------------------------------------------

fn rest(entry: &ConnectorEntry, config: &mut Form, credentials: &mut Form) {
    // A catalogued service ships a request template with everything but the
    // secrets filled in. Its values are offered as DEFAULTS rather than set
    // silently: the template describes a whole dlt client and only parts of it
    // are datasource config, so a value that lands in the wrong slot is one the
    // user can see on the prompt and correct, instead of a 422 to decode.
    let template = entry.template.as_ref();
    let base_url = template
        .and_then(|t| t.pointer("/client/base_url"))
        .and_then(|v| v.as_str());
    config.ask_with_default("base_url", base_url);
    config.set("source_name", entry.name.clone().into());

    match entry.auth.as_deref() {
        Some(a) if config.accepts("auth_type", a) => config.set("auth_type", a.into()),
        _ => config.ask("auth_type"),
    }
    let auth_type = config.get_str("auth_type").unwrap_or_else(|| "none".into());

    // The non-secret half of the auth block: the URL a token is exchanged at,
    // the header or query parameter a key rides in. Without them the request
    // authenticates against the wrong place, and the template is where the
    // right value exists.
    let auth = template.and_then(|t| t.pointer("/client/auth"));
    let mut params = serde_json::Map::new();
    if auth_type == "oauth_client_credentials" {
        let url = prompt::text(
            "OAuth token URL:",
            Some("Where the client credentials are exchanged for a token."),
            auth.and_then(|a| a.get("access_token_url"))
                .and_then(|v| v.as_str()),
        );
        if !url.trim().is_empty() {
            params.insert("oauth_token_url".into(), url.trim().into());
        }
    }
    if auth_type == "api_key" {
        let header = prompt::text(
            "API key header:",
            Some("The header the key is sent in."),
            auth.and_then(|a| a.get("name")).and_then(|v| v.as_str()),
        );
        if !header.trim().is_empty() {
            params.insert("api_key_header".into(), header.trim().into());
        }
    }
    if !params.is_empty() {
        config.set("auth_params", params.into());
    }

    rest_credentials(&auth_type, credentials);
}

/// Which secret each auth type wants. Authored, because the credentials schema
/// describes every field every REST service could need and marks all of them
/// optional — it is `auth_type` that decides which two of the eleven this
/// service reads, and the schema has no way to say so.
///
/// An auth type this build has no entry for falls through to offering the whole
/// list. A longer questionnaire, and the alternative is a service the wizard
/// refuses to configure because the CLI is a release behind.
fn rest_credential_fields(auth_type: &str) -> &'static [&'static str] {
    match auth_type {
        "none" => &[],
        "bearer" => &["token"],
        "api_key" | "query_token" | "mapbox_query_token" | "basic_token" => &["api_key"],
        "basic" | "http_basic" => &["username", "password"],
        "oauth_client_credentials" => &["client_id", "client_secret"],
        "xero" => &["token", "tenant_id"],
        "supabase" => &["api_key"],
        "raw_authorization" => &["authorization"],
        "plaid" => &["client_id", "secret", "access_token"],
        _ => &[
            "token",
            "api_key",
            "username",
            "password",
            "client_id",
            "client_secret",
            "tenant_id",
            "authorization",
            "secret",
            "access_token",
        ],
    }
}

fn rest_credentials(auth_type: &str, credentials: &mut Form) {
    let fields = rest_credential_fields(auth_type);
    if fields.len() > 3 {
        println!(
            "  This service's auth type is '{auth_type}'; fill in what it needs and \
             leave the rest blank."
        );
    }
    for field in fields {
        credentials.ask(field);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_storage_uri_names_its_provider() {
        assert_eq!(provider_of("s3://events-prod/"), Some("s3"));
        assert_eq!(provider_of("S3://Events"), Some("s3"));
        assert_eq!(provider_of("gs://bucket"), Some("gs"));
        assert_eq!(
            provider_of("abfss://c@acct.dfs.core.windows.net"),
            Some("az")
        );
        assert_eq!(provider_of("file:///data"), Some("file"));
        // A bare path is a local root.
        assert_eq!(provider_of("/data/parquet"), Some("file"));
        // A scheme with no mapping is asked for rather than guessed at.
        assert_eq!(provider_of("hdfs://nn/data"), None);
        assert_eq!(provider_of(""), None);
    }

    #[test]
    fn port_defaults_are_offered_only_where_the_engine_has_one() {
        assert_eq!(default_port("postgres"), Some("5432"));
        assert_eq!(default_port("mysql"), Some("3306"));
        assert_eq!(default_port("mssql"), Some("1433"));
        // Engines addressed by account or path have no port to default.
        assert_eq!(default_port("bigquery"), None);
        assert_eq!(default_port("snowflake"), None);
    }

    #[test]
    fn every_rest_auth_type_the_service_accepts_names_its_secret() {
        // `none` is the one auth type with nothing to ask for.
        assert!(rest_credential_fields("none").is_empty());
        for auth in [
            "bearer",
            "api_key",
            "basic",
            "http_basic",
            "basic_token",
            "oauth_client_credentials",
            "query_token",
            "mapbox_query_token",
            "raw_authorization",
            "plaid",
            "xero",
            "supabase",
        ] {
            let fields = rest_credential_fields(auth);
            assert!(!fields.is_empty(), "{auth} asks for no credential");
            // Short enough to be a question list rather than a form: an auth
            // type that reached the fall-through would be far longer, which is
            // what the printed note in `rest_credentials` keys on.
            assert!(
                fields.len() <= 3,
                "{auth}: {fields:?} reads as the catch-all"
            );
        }
        // An auth type this build has never heard of still gets somewhere.
        assert!(rest_credential_fields("mtls").len() > 3);
    }

    #[test]
    fn a_wizard_answer_is_only_kept_when_the_schema_has_the_field() {
        // Every flow names fields by hand. The schema is what decides whether
        // one is asked, so a family reference describing nothing produces an
        // empty body rather than a body of invented keys.
        let reference: FamilyReference = serde_json::from_str(
            r#"{"family":"sql","config_schema":{"type":"object"},
                "credentials_schema":{"type":"object"},"selector_schema":{}}"#,
        )
        .unwrap();
        let entry = ConnectorEntry {
            name: "postgres".into(),
            family: "sql".into(),
            ..Default::default()
        };
        let (config, credentials) = build(&entry, &reference);
        assert_eq!(config, serde_json::json!({}));
        assert!(credentials.is_none());
    }
}
