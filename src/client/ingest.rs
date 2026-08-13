//! Raw-HTTP client for the ingest API (`/v1/ingest/*`).
//!
//! These routes are not in the generated SDK yet, so — like the token/session
//! mints — they ride the hand-rolled `reqwest::blocking` seam in
//! [`crate::client::raw_http`].
//! Every request carries a bearer + `X-Workspace-Id`; the gateway validates the
//! pair and derives the ingest destination server-side, so the CLI never sends
//! a destination.
//!
//! **Four nouns, matching the service:** a *datasource* is a reusable external
//! source identity (`ds_…`); each config edit appends an immutable *config
//! version* (`dscv_…`); an *ingest* (`ing_…`) is a saved load definition
//! (datasource + selector + destination + type/schedule); a *run* (`run_…`) is
//! one execution attempt, carrying snapshots of everything it used. Display
//! names are labels only — ids are the identity the API resolves on.
//!
//! Auth is split by endpoint kind. The routes that persist a credential
//! (`POST /datasources`, `PATCH /datasources/{id}/config`, `POST /ingests`)
//! require a durable `hd_...` API key as the bearer — the run executes the
//! credential *after* the request returns, so the server 422s any 5-minute JWT.
//! Read routes accept *workspace-scoped* JWTs, but the CLI's login session is a
//! user-scoped JWT and the worker refuses to trust `X-Workspace-Id` on the JWT
//! route — so in practice only `/connectors` (workspace-free) works without a
//! key. So: when an API key is available (`--api-key` / `HOTDATA_API_KEY`) it is
//! sent directly on every call; otherwise the CLI's session JWT
//! ([`jwt::ensure_access_token`]) is used, credential-persisting calls fail fast
//! with [`IngestError::NeedsApiKey`], and workspace-scoped reads get a 403 with
//! an `--api-key` hint.
//!
//! When the ingest routes land in the public OpenAPI and the SDK regenerates,
//! delete this module and move the commands onto `sdk::Api`.
//!
//! The result-reading endpoints are intentionally absent — once a run lands,
//! that path is the core `query`/`databases`/`results` commands.
#![allow(dead_code)] // Response structs are read only through serde/printing.

use crate::client::jwt;
use crate::config;
use crate::util;
use serde::{Deserialize, Serialize};

/// A typed error from an ingest call. Mirrors the `ApiError::exit()` ergonomics
/// the SDK-backed commands use, so handlers can `.unwrap_or_else(|e| e.exit())`.
#[derive(Debug)]
pub enum IngestError {
    /// Non-2xx response; `body` is the server's (unredacted) response text.
    Http { status: u16, body: String },
    /// Transport/connection failure.
    Connection(String),
    /// 2xx whose body didn't match the expected shape.
    Decode(String),
    /// A credential-persisting call attempted with only a session JWT — the
    /// server would 422 it (the run outlives a short-lived JWT), so fail before
    /// sending credentials that can't work.
    NeedsApiKey,
}

impl IngestError {
    pub fn message(&self) -> String {
        match self {
            // The service's error envelope is
            // {"error": {"code", "message", "details"}}. util::api_error pulls
            // the human message (matching every SDK-backed command);
            // util::error_code pulls the stable code, which is what a script
            // or a follow-up hint should branch on — so print both.
            IngestError::Http { status, body } => {
                let message = util::api_error(body.clone());
                match util::error_code(body) {
                    Some(code) => format!("HTTP {status}: {message} ({code})"),
                    None => format!("HTTP {status}: {message}"),
                }
            }
            IngestError::Connection(e) => format!("connection error: {e}"),
            IngestError::Decode(e) => format!("malformed response: {e}"),
            IngestError::NeedsApiKey => {
                "this command needs a workspace API key (hd_...) — a login session \
                 cannot be used here"
                    .into()
            }
        }
    }

    pub fn exit(&self) -> ! {
        use crossterm::style::Stylize;
        eprintln!("{}", format!("error: {}", self.message()).red());
        // Cold-start / scale-to-zero hint: the worker sits behind KEDA.
        if matches!(
            self,
            IngestError::Http {
                status: 502 | 503,
                ..
            }
        ) {
            eprintln!(
                "{}",
                "the ingest service may be starting up — retry in a few seconds".dark_grey()
            );
        }
        // A transport failure on a create is usually the worker being
        // unavailable — a cold start or a rollout — where the gateway holds the
        // connection until its timeout rather than returning a status. "error
        // sending request" is opaque; point at the actual cause + retry.
        if matches!(self, IngestError::Connection(_)) {
            eprintln!(
                "{}",
                "the request didn't complete — the ingest service may be starting up or \
                 redeploying; retry in a moment."
                    .dark_grey()
            );
        }
        if matches!(self, IngestError::NeedsApiKey) {
            eprintln!(
                "{}",
                "Pass --api-key or set HOTDATA_API_KEY with a workspace API token (hd_...)."
                    .dark_grey()
            );
        }
        // Expired/invalid session: same re-auth hint every SDK-backed
        // command prints, so ingest is not the one group that answers an
        // expired login with raw JSON.
        if matches!(self, IngestError::Http { status: 401, .. }) {
            eprintln!(
                "{}",
                "Run 'hotdata auth login' to authenticate.".dark_grey()
            );
        }
        // The worker refuses to trust X-Workspace-Id on the JWT route, and a
        // CLI login session is a *user*-scoped JWT — so every workspace-scoped
        // ingest endpoint 403s on it. Only an API key carries the workspace.
        if let IngestError::Http { status: 403, body } = self
            && body.contains("workspace-scoped credential")
        {
            eprintln!(
                "{}",
                "This command needs workspace access — pass --api-key or set \
                 HOTDATA_API_KEY with a workspace API key (hd_...)."
                    .dark_grey()
            );
        }
        // Codes whose fix is a specific next command. Keep this list short:
        // the server's message says what happened, these say what to do.
        if let IngestError::Http { body, .. } = self {
            match util::error_code(body).as_deref() {
                Some("destination_table_conflict") => {
                    let conflicting = util::error_detail(body, "conflicting_ingest_id")
                        .unwrap_or_else(|| "<ingest-id>".into());
                    eprintln!(
                        "{}",
                        format!(
                            "A continuous ingest already owns that table. Release it with: \
                             hotdata ingest delete {conflicting}"
                        )
                        .dark_grey()
                    );
                }
                Some("immutable_ingest_definition") => {
                    eprintln!(
                        "{}",
                        "Selector and destination are fixed at creation — create a new ingest \
                         instead ('hotdata ingest create')."
                            .dark_grey()
                    );
                }
                Some("active_ingests_exist") => {
                    eprintln!(
                        "{}",
                        "Delete the datasource's ingests first: hotdata ingest list \
                         --datasource-id <id>, then 'hotdata ingest delete <ingest-id>'."
                            .dark_grey()
                    );
                }
                _ => {}
            }
        }
        std::process::exit(1);
    }
}

/// Ingest client bound to a workspace + a resolved bearer token.
pub struct IngestClient {
    /// `{api_url}/ingest` — api_url already carries the `/v1` suffix.
    base: String,
    token: String,
    /// Whether `token` is a durable `hd_...` API key (vs a session JWT).
    /// Credential-persisting endpoints require the former; see the module docs.
    token_is_api_key: bool,
    workspace_id: String,
    client: reqwest::blocking::Client,
}

impl IngestClient {
    /// Build a client for `workspace_id`. An explicit API key (`--api-key` /
    /// `HOTDATA_API_KEY`) is sent as the bearer directly — the extAuth route
    /// accepts it everywhere and the create routes *require* it. Without one,
    /// fall back to the CLI's session JWT, which covers the read routes.
    pub fn new(workspace_id: &str) -> Self {
        let profile = config::load("default").unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });
        // Same trust filter as sdk::Api / credentials: an empty or template
        // key must fall through to the session JWT, not ship as a bearer.
        // (HOTDATA_DATABASE_TOKEN is deliberately NOT consulted here:
        // database-scoped tokens cannot serve as ingest destinations — the
        // run's load fails Forbidden — so ingest always uses the workspace
        // credential.)
        let api_key = profile
            .api_key
            .clone()
            .filter(|k| !k.is_empty() && *k != "PLACEHOLDER");
        let (token, token_is_api_key) = match api_key {
            Some(key) => (key, true),
            None => {
                let jwt = jwt::ensure_access_token(&profile, None).unwrap_or_else(|e| {
                    use crossterm::style::Stylize;
                    eprintln!("{}", format!("auth error: {e}").red());
                    eprintln!("Run 'hotdata auth login' to authenticate.");
                    std::process::exit(1);
                });
                (jwt, false)
            }
        };
        let base = format!("{}/ingest", (*profile.api_url).trim_end_matches('/'));
        IngestClient {
            base,
            token,
            token_is_api_key,
            workspace_id: workspace_id.to_string(),
            client: crate::client::raw_http::build_http_client(),
        }
    }

    /// Test-only constructor bypassing config/session resolution.
    #[cfg(test)]
    pub fn from_parts(base: &str, token: &str, token_is_api_key: bool, workspace_id: &str) -> Self {
        IngestClient {
            base: format!("{}/ingest", base.trim_end_matches('/')),
            token: token.to_string(),
            token_is_api_key,
            workspace_id: workspace_id.to_string(),
            client: crate::client::raw_http::build_http_client(),
        }
    }

    /// Whether this client holds a durable `hd_` API key (vs a session JWT).
    /// Callers use this to skip requests the server will always reject on a
    /// JWT (see the module docs).
    pub fn has_api_key(&self) -> bool {
        self.token_is_api_key
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// A request builder with the bearer + workspace headers already set.
    fn authed(&self, method: reqwest::Method, path: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .request(method, self.url(path))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-Workspace-Id", &self.workspace_id)
    }

    /// A GET with optional `?key=value` filters. Empty filter sets are not
    /// applied at all, so the plain listing URL stays free of a bare `?`.
    fn authed_get(
        &self,
        path: &str,
        filters: &[(&str, String)],
    ) -> reqwest::blocking::RequestBuilder {
        let builder = self.authed(reqwest::Method::GET, path);
        if filters.is_empty() {
            builder
        } else {
            builder.query(filters)
        }
    }

    /// Send a request, enforce a 2xx, and decode the JSON body into `T`.
    ///
    /// `body_log` is the *printable* form for `--debug` — callers whose body
    /// carries secrets must pass a view through [`redact_secret_fields`], never
    /// the wire body itself.
    fn send<T: for<'de> Deserialize<'de>>(
        &self,
        builder: reqwest::blocking::RequestBuilder,
        body_log: Option<&serde_json::Value>,
    ) -> Result<T, IngestError> {
        let (status, body) = util::send_debug(&self.client, builder, body_log)
            .map_err(|e| IngestError::Connection(e.to_string()))?;
        if !status.is_success() {
            return Err(IngestError::Http {
                status: status.as_u16(),
                body,
            });
        }
        serde_json::from_str(&body).map_err(|e| IngestError::Decode(e.to_string()))
    }

    /// Guard for the credential-persisting routes: a session JWT is rejected
    /// server-side (422) because the run outlives it, so fail fast with a
    /// message that says what to do instead.
    fn require_api_key(&self) -> Result<(), IngestError> {
        if self.token_is_api_key {
            Ok(())
        } else {
            Err(IngestError::NeedsApiKey)
        }
    }

    // --- datasources -----------------------------------------------------

    /// Validate a datasource config without persisting a datasource, config
    /// version, managed database, or secret. The credentials are used for the
    /// validation request only, so this route takes a session JWT too.
    pub fn validate_datasource(
        &self,
        req: &DatasourceConfig,
    ) -> Result<ValidateResponse, IngestError> {
        let body = serde_json::to_value(req).expect("DatasourceConfig serializes");
        let body_log = redact_secret_fields(&body);
        self.send(
            self.authed(reqwest::Method::POST, "/datasources/validate")
                .json(&body),
            Some(&body_log),
        )
    }

    /// Create a datasource and its config version 1.
    pub fn create_datasource(&self, req: &DatasourceConfig) -> Result<Datasource, IngestError> {
        self.require_api_key()?;
        let body = serde_json::to_value(req).expect("DatasourceConfig serializes");
        let body_log = redact_secret_fields(&body);
        self.send(
            self.authed(reqwest::Method::POST, "/datasources")
                .json(&body),
            Some(&body_log),
        )
    }

    /// Append a config version and move the datasource's current pointer.
    /// `credentials` omitted inherits the previous source secret refs; an
    /// explicitly empty object means "no source credential" — see
    /// [`ConfigUpdate`].
    pub fn update_datasource_config(
        &self,
        datasource_id: &str,
        req: &ConfigUpdate,
    ) -> Result<ConfigUpdateAck, IngestError> {
        self.require_api_key()?;
        let body = serde_json::to_value(req).expect("ConfigUpdate serializes");
        let body_log = redact_secret_fields(&body);
        self.send(
            self.authed(
                reqwest::Method::PATCH,
                &format!("/datasources/{datasource_id}/config"),
            )
            .json(&body),
            Some(&body_log),
        )
    }

    pub fn list_datasources(
        &self,
        filters: &[(&str, String)],
    ) -> Result<DatasourcesResponse, IngestError> {
        self.send(self.authed_get("/datasources", filters), None)
    }

    pub fn get_datasource(&self, datasource_id: &str) -> Result<Datasource, IngestError> {
        self.send(
            self.authed(
                reqwest::Method::GET,
                &format!("/datasources/{datasource_id}"),
            ),
            None,
        )
    }

    /// Soft-delete a datasource. The server returns `409
    /// active_ingests_exist` while any non-deleted ingest references it —
    /// destination tables and managed databases are never touched.
    pub fn delete_datasource(&self, datasource_id: &str) -> Result<DeleteAck, IngestError> {
        self.send(
            self.authed(
                reqwest::Method::DELETE,
                &format!("/datasources/{datasource_id}"),
            ),
            None,
        )
    }

    // --- ingests ----------------------------------------------------------

    /// Create a saved load definition. A `one_time` ingest also creates its
    /// first run in the same transaction (`initial_run_id` in the response);
    /// `scheduled`/`continuous` ones start on the next scheduler tick.
    pub fn create_ingest(&self, req: &IngestCreate) -> Result<Ingest, IngestError> {
        self.require_api_key()?;
        let body = serde_json::to_value(req).expect("IngestCreate serializes");
        self.send(
            self.authed(reqwest::Method::POST, "/ingests").json(&body),
            Some(&body),
        )
    }

    pub fn list_ingests(&self, filters: &[(&str, String)]) -> Result<IngestsResponse, IngestError> {
        self.send(self.authed_get("/ingests", filters), None)
    }

    pub fn get_ingest(&self, ingest_id: &str) -> Result<Ingest, IngestError> {
        self.send(
            self.authed(reqwest::Method::GET, &format!("/ingests/{ingest_id}")),
            None,
        )
    }

    /// Change future dispatch timing. Rejected (`409`) for `one_time` ingests,
    /// and never creates an extra run — `next_run_at: "now"` is what brings the
    /// next scheduled run forward.
    pub fn update_schedule(
        &self,
        ingest_id: &str,
        req: &SchedulePatch,
    ) -> Result<Ingest, IngestError> {
        let body = serde_json::to_value(req).expect("SchedulePatch serializes");
        self.send(
            self.authed(
                reqwest::Method::PATCH,
                &format!("/ingests/{ingest_id}/schedule"),
            )
            .json(&body),
            Some(&body),
        )
    }

    /// Stop an ingest: cancels the active run *and* prevents future scheduled
    /// dispatch. Idempotent when already stopped.
    pub fn cancel_ingest(&self, ingest_id: &str) -> Result<CancelAck, IngestError> {
        self.send(
            self.authed(
                reqwest::Method::POST,
                &format!("/ingests/{ingest_id}/cancel"),
            ),
            None,
        )
    }

    /// Clear the stop and the scheduler backoff. Deliberately does NOT create a
    /// run — the next one follows the schedule.
    pub fn resume_ingest(&self, ingest_id: &str) -> Result<Ingest, IngestError> {
        self.send(
            self.authed(
                reqwest::Method::POST,
                &format!("/ingests/{ingest_id}/resume"),
            ),
            None,
        )
    }

    /// Soft-delete an ingest: cancels an active run, releases destination
    /// table ownership, and leaves the destination table, its data, and the
    /// datasource alone.
    pub fn delete_ingest(&self, ingest_id: &str) -> Result<DeleteAck, IngestError> {
        self.send(
            self.authed(reqwest::Method::DELETE, &format!("/ingests/{ingest_id}")),
            None,
        )
    }

    // --- runs -------------------------------------------------------------

    /// Runs for one ingest, newest first.
    pub fn list_runs(
        &self,
        ingest_id: &str,
        filters: &[(&str, String)],
    ) -> Result<RunsResponse, IngestError> {
        self.send(
            self.authed_get(&format!("/ingests/{ingest_id}/runs"), filters),
            None,
        )
    }

    pub fn get_run(&self, run_id: &str) -> Result<Run, IngestError> {
        self.send(
            self.authed(reqwest::Method::GET, &format!("/runs/{run_id}")),
            None,
        )
    }

    // --- catalog ----------------------------------------------------------

    /// The connector catalog. REST entries carry a ready-to-edit `template`
    /// (a dlt `rest_api` config with the service's `base_url`, auth shape, and
    /// resources pre-filled and `<PLACEHOLDER>` secrets); generic families
    /// carry a `config_schema` naming the fields `--config` takes.
    pub fn connectors(&self) -> Result<ConnectorsResponse, IngestError> {
        self.send(self.authed(reqwest::Method::GET, "/connectors"), None)
    }
}

/// Request fields that can carry source secrets. `credentials` is secrets by
/// definition; `config` is documented as secret-free, but a caller can always
/// inline a password there, and `--debug` output lands in a terminal scrollback
/// — so both subtrees are dropped from the printable body.
const SECRET_BODY_FIELDS: &[&str] = &["credentials", "config"];

/// Debug-log view of a request body with the secret-bearing subtrees replaced
/// wholesale. These fields are nested *objects* whose secret keys vary by
/// family, so dropping the whole subtree beats field-level masking
/// (`util::redact_json_fields` only masks string values). Mirrors the
/// `redacted_form_body` pattern in `jwt.rs`.
fn redact_secret_fields(body: &serde_json::Value) -> serde_json::Value {
    let mut v = body.clone();
    if let serde_json::Value::Object(map) = &mut v {
        for key in SECRET_BODY_FIELDS {
            if let Some(val) = map.get_mut(*key) {
                *val = serde_json::Value::String("***".into());
            }
        }
    }
    v
}

// --- request types --------------------------------------------------------

/// Body of `POST /datasources/validate` and `POST /datasources`.
///
/// `config` and `credentials` are both family-specific: the datasource is what
/// a credential opens (host/root/cluster/catalog), never the subset to read —
/// that is the ingest's selector.
#[derive(Debug, Serialize, Default)]
pub struct DatasourceConfig {
    pub family: String,
    /// Label only. Not identity, not unique, never resolved against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub config: serde_json::Value,
    /// `None` omits the key entirely; `Some({})` sends an explicitly empty
    /// object. The two are different requests — see [`ConfigUpdate`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials: Option<serde_json::Value>,
}

/// Body of `PATCH /datasources/{id}/config`. Always appends a config version.
///
/// Credential semantics are three-valued and the difference is on the wire:
///
/// ```text
/// credentials omitted (None)      -> inherit the previous source secret refs
/// credentials present (Some(v))   -> replace the source credential state
/// credentials empty   (Some({}))  -> no source credential (no-auth families)
/// ```
#[derive(Debug, Serialize)]
pub struct ConfigUpdate {
    pub config: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials: Option<serde_json::Value>,
}

/// Body of `POST /ingests`: the saved load definition. `selector` and
/// `destination` are immutable after creation — changing either means a new
/// ingest.
#[derive(Debug, Serialize)]
pub struct IngestCreate {
    pub datasource_id: String,
    /// Wire values: `one_time` | `scheduled` | `continuous`.
    pub r#type: String,
    pub selector: serde_json::Value,
    pub destination: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<serde_json::Value>,
}

/// Body of `PATCH /ingests/{id}/schedule`.
#[derive(Debug, Serialize)]
pub struct SchedulePatch {
    pub schedule: serde_json::Value,
}

// --- response types -------------------------------------------------------

/// `POST /datasources/validate` body. Nothing was persisted; `discovered` is
/// family-specific (schemas/tables, bucket listings, topics, …).
#[derive(Debug, Deserialize, Serialize)]
pub struct ValidateResponse {
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub normalized_config: Option<serde_json::Value>,
    #[serde(default)]
    pub discovered: Option<serde_json::Value>,
    #[serde(default)]
    pub detail: Option<String>,
}

/// A datasource row. `POST /datasources` returns the same shape with only the
/// identity fields populated, so one struct serves create, list, and show.
#[derive(Debug, Deserialize, Serialize)]
pub struct Datasource {
    pub datasource_id: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    /// `creating` | `active` | `failed` | `deleted`.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub current_config_version_id: Option<String>,
    #[serde(default)]
    pub version: Option<i64>,
    /// Current non-secret config (show only). Secret refs are never returned
    /// as values.
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub discovered: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DatasourcesResponse {
    #[serde(default)]
    pub datasources: Vec<Datasource>,
}

/// `PATCH /datasources/{id}/config` body: the pointer move, so the caller can
/// see which version replaced which.
#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigUpdateAck {
    pub datasource_id: String,
    #[serde(default)]
    pub previous_config_version_id: Option<String>,
    #[serde(default)]
    pub current_config_version_id: Option<String>,
    #[serde(default)]
    pub version: Option<i64>,
    #[serde(default)]
    pub state: Option<String>,
}

/// Soft-delete acknowledgement, shared by datasources and ingests. Both id
/// fields are optional so a bodyless 200 still decodes.
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct DeleteAck {
    #[serde(default)]
    pub datasource_id: Option<String>,
    #[serde(default)]
    pub ingest_id: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

/// An ingest row. `POST /ingests` returns the identity fields plus
/// `initial_run_id`; list/show/resume/schedule return the fuller view — one
/// struct serves them all.
#[derive(Debug, Deserialize, Serialize)]
pub struct Ingest {
    pub ingest_id: String,
    #[serde(default)]
    pub datasource_id: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    /// `one_time` | `scheduled` | `continuous`.
    #[serde(default)]
    pub r#type: Option<String>,
    /// `creating` | `active` | `stopped` | `completed` | `failed` | `deleted`.
    #[serde(default)]
    pub state: Option<String>,
    /// Set by the machine when repeated failures stopped the ingest; empty for
    /// a user cancel.
    #[serde(default)]
    pub stopped_reason: Option<String>,
    #[serde(default)]
    pub selector: Option<serde_json::Value>,
    /// The logical write target as one object — `{database_id, schema, table,
    /// write_mode}`, the same document `POST /ingests` sent. The service also
    /// keeps those three in their own columns for the destination-ownership
    /// index, but they are not on the wire: this object is the whole
    /// destination a response carries.
    #[serde(default)]
    pub destination: Option<serde_json::Value>,
    #[serde(default)]
    pub schedule: Option<serde_json::Value>,
    #[serde(default)]
    pub next_attempt_at: Option<String>,
    /// Only on the `POST /ingests` response for a `one_time` ingest.
    #[serde(default)]
    pub initial_run_id: Option<String>,
    #[serde(default)]
    pub latest_run: Option<Run>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct IngestsResponse {
    #[serde(default)]
    pub ingests: Vec<Ingest>,
}

/// A `GET /ingests` body in the service's own ingest-view shape, shared by the
/// decode test here and the rendering test in `commands::ingest` so both read
/// the same bytes a real response carries.
///
/// Pinned because the destination arrives ONLY as the nested object: a fixture
/// invented with top-level `destination_*` fields decodes, renders, and passes
/// green while every real response prints a blank destination.
#[cfg(test)]
pub const WORKER_INGEST_LIST_BODY: &str = r#"{"ingests":[
    {"ingest_id":"ing_1","datasource_id":"ds_1","family":"sql","type":"continuous",
     "state":"active",
     "selector":{"mode":"tables","schema":"public","tables":["orders"]},
     "destination":{"database_id":"db_1","schema":"public","table":"orders_raw",
                    "write_mode":"replace"},
     "schedule":{"interval_seconds":300},
     "next_attempt_at":"2026-08-02T09:05:00+00:00","interval_seconds":300,
     "consecutive_failures":0,"stopped_reason":null,"attempt":7,
     "job_name":"drain-run-7","created_at":"2026-08-02T09:00:00+00:00",
     "updated_at":"2026-08-02T09:04:00+00:00","deleted_at":null}
]}"#;

/// `POST /ingests/{id}/cancel` body. Cancel means both "stop the active run"
/// and "stop future runs", so the ack reports the run it cancelled *and* the
/// resulting ingest state.
#[derive(Debug, Deserialize, Serialize)]
pub struct CancelAck {
    pub ingest_id: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub cancelled_run_id: Option<String>,
    #[serde(default)]
    pub stopped: bool,
}

/// One execution attempt. The `*_snapshot` fields are what makes a historical
/// run explainable after the datasource config or schedule has moved on.
#[derive(Debug, Deserialize, Serialize)]
pub struct Run {
    pub run_id: String,
    #[serde(default)]
    pub ingest_id: Option<String>,
    #[serde(default)]
    pub datasource_id: Option<String>,
    #[serde(default)]
    pub config_version_id: Option<String>,
    #[serde(default)]
    pub attempt: Option<i64>,
    /// `queued` | `running` | `succeeded` | `failed` | `cancelled`.
    pub status: String,
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub error: Option<serde_json::Value>,
    #[serde(default)]
    pub selector_snapshot: Option<serde_json::Value>,
    #[serde(default)]
    pub destination_snapshot: Option<serde_json::Value>,
    #[serde(default)]
    pub schedule_snapshot: Option<serde_json::Value>,
    #[serde(default)]
    pub job_name: Option<String>,
    #[serde(default)]
    pub queued_at: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RunsResponse {
    #[serde(default)]
    pub runs: Vec<Run>,
}

#[derive(Deserialize)]
pub struct ConnectorsResponse {
    pub connectors: Vec<ConnectorEntry>,
}

/// One catalog entry. `sql` names are dialects, `filesystem`/`iceberg`/`rest`
/// are family templates. REST entries additionally carry `auth` (the method
/// name, e.g. `bearer`, `oauth_client_credentials`, `none`) and a `template`
/// dlt config with `<PLACEHOLDER>` secrets to fill in.
#[derive(Clone, Deserialize)]
pub struct ConnectorEntry {
    pub name: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub template: Option<serde_json::Value>,
    /// JSON Schema for the entry's `--config` payload (generic families).
    #[serde(default)]
    pub config_schema: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_key_client(server: &mockito::Server) -> IngestClient {
        IngestClient::from_parts(&server.url(), "hd_test", true, "ws-1")
    }

    fn jwt_client(server: &mockito::Server) -> IngestClient {
        IngestClient::from_parts(&server.url(), "eyJ.fake.jwt", false, "ws-1")
    }

    // --- datasources -------------------------------------------------------

    #[test]
    fn validate_datasource_posts_family_config_and_credentials() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/datasources/validate")
            .match_header("authorization", "Bearer hd_test")
            .match_header("x-workspace-id", "ws-1")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "family": "sql",
                "config": {"dialect": "postgres", "host": "pg.example.com", "database": "prod"},
                "credentials": {"username": "reader", "password": "s3cret"},
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"valid":true,"family":"sql",
                    "normalized_config":{"dialect":"postgres","host":"pg.example.com"},
                    "discovered":{"schemas":["public"],
                                  "tables":[{"schema":"public","table":"orders"}]}}"#,
            )
            .create();

        let req = DatasourceConfig {
            family: "sql".into(),
            config: serde_json::json!({
                "dialect": "postgres", "host": "pg.example.com", "database": "prod"
            }),
            credentials: Some(serde_json::json!({"username": "reader", "password": "s3cret"})),
            ..Default::default()
        };
        let resp = api_key_client(&server).validate_datasource(&req).unwrap();
        m.assert();
        assert!(resp.valid);
        assert_eq!(resp.family.as_deref(), Some("sql"));
        assert_eq!(resp.discovered.unwrap()["schemas"][0], "public");
    }

    #[test]
    fn validate_datasource_works_with_a_session_jwt() {
        // Validation persists nothing, so it does not need a durable key.
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/datasources/validate")
            .match_header("authorization", "Bearer eyJ.fake.jwt")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"valid":false,"family":"sql","detail":"connection refused"}"#)
            .create();

        let resp = jwt_client(&server)
            .validate_datasource(&DatasourceConfig {
                family: "sql".into(),
                ..Default::default()
            })
            .unwrap();
        m.assert();
        assert!(!resp.valid);
        assert_eq!(resp.detail.as_deref(), Some("connection refused"));
    }

    #[test]
    fn create_datasource_sends_display_name_and_decodes_the_new_identity() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/datasources")
            .match_header("authorization", "Bearer hd_test")
            .match_header("x-workspace-id", "ws-1")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "family": "sql",
                "display_name": "prod postgres",
                "config": {"dialect": "postgres"},
                "credentials": {"username": "reader"},
            })))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"datasource_id":"ds_01J","family":"sql","display_name":"prod postgres",
                    "current_config_version_id":"dscv_01J","state":"active"}"#,
            )
            .create();

        let ds = api_key_client(&server)
            .create_datasource(&DatasourceConfig {
                family: "sql".into(),
                display_name: Some("prod postgres".into()),
                config: serde_json::json!({"dialect": "postgres"}),
                credentials: Some(serde_json::json!({"username": "reader"})),
            })
            .unwrap();
        m.assert();
        assert_eq!(ds.datasource_id, "ds_01J");
        assert_eq!(ds.current_config_version_id.as_deref(), Some("dscv_01J"));
        assert_eq!(ds.state.as_deref(), Some("active"));
    }

    #[test]
    fn credential_persisting_calls_fail_fast_on_a_session_jwt() {
        // Point at a dead port: reaching the network would surface as a
        // Connection error instead of NeedsApiKey.
        let client = IngestClient::from_parts("http://127.0.0.1:1", "eyJ.fake.jwt", false, "ws-1");

        let create = client
            .create_datasource(&DatasourceConfig::default())
            .unwrap_err();
        assert!(matches!(create, IngestError::NeedsApiKey));

        let patch = client
            .update_datasource_config(
                "ds_1",
                &ConfigUpdate {
                    config: serde_json::json!({}),
                    credentials: None,
                },
            )
            .unwrap_err();
        assert!(matches!(patch, IngestError::NeedsApiKey));

        let ingest = client
            .create_ingest(&IngestCreate {
                datasource_id: "ds_1".into(),
                r#type: "one_time".into(),
                selector: serde_json::json!({}),
                destination: serde_json::json!({}),
                schedule: None,
            })
            .unwrap_err();
        assert!(matches!(ingest, IngestError::NeedsApiKey));
        assert!(ingest.message().contains("API key"), "{}", ingest.message());
    }

    #[test]
    fn update_config_omits_credentials_when_inheriting() {
        let mut server = mockito::Server::new();
        // Omitted credentials must not appear as `null` — the server reads
        // "key absent" as "inherit the previous secret refs".
        let m = server
            .mock("PATCH", "/ingest/datasources/ds_01J/config")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "config": {"dialect": "postgres", "host": "pg.example.com"},
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"datasource_id":"ds_01J","previous_config_version_id":"dscv_old",
                    "current_config_version_id":"dscv_new","version":2,"state":"active"}"#,
            )
            .create();

        let ack = api_key_client(&server)
            .update_datasource_config(
                "ds_01J",
                &ConfigUpdate {
                    config: serde_json::json!({"dialect": "postgres", "host": "pg.example.com"}),
                    credentials: None,
                },
            )
            .unwrap();
        m.assert();
        assert_eq!(ack.version, Some(2));
        assert_eq!(ack.previous_config_version_id.as_deref(), Some("dscv_old"));
    }

    #[test]
    fn update_config_sends_an_explicitly_empty_credentials_object() {
        let mut server = mockito::Server::new();
        // `{}` is a different request from omission: it drops the source
        // credential for families that support no-auth sources.
        let m = server
            .mock("PATCH", "/ingest/datasources/ds_01J/config")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "config": {"provider": "s3", "root_uri": "s3://public"},
                "credentials": {},
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"datasource_id":"ds_01J","version":3,"state":"active"}"#)
            .create();

        api_key_client(&server)
            .update_datasource_config(
                "ds_01J",
                &ConfigUpdate {
                    config: serde_json::json!({"provider": "s3", "root_uri": "s3://public"}),
                    credentials: Some(serde_json::json!({})),
                },
            )
            .unwrap();
        m.assert();
    }

    #[test]
    fn list_datasources_applies_filters_and_omits_an_empty_query() {
        let mut server = mockito::Server::new();
        let plain = server
            .mock("GET", "/ingest/datasources")
            .match_header("x-workspace-id", "ws-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"datasources":[{"datasource_id":"ds_1","family":"sql",
                    "display_name":"prod postgres","state":"active",
                    "current_config_version_id":"dscv_1",
                    "created_at":"2026-08-01T10:00:00+00:00"}]}"#,
            )
            .create();
        let filtered = server
            .mock("GET", "/ingest/datasources?family=sql&state=active")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"datasources":[]}"#)
            .create();

        let client = api_key_client(&server);
        let resp = client.list_datasources(&[]).unwrap();
        assert_eq!(resp.datasources[0].datasource_id, "ds_1");
        assert_eq!(
            resp.datasources[0].display_name.as_deref(),
            Some("prod postgres")
        );

        let resp = client
            .list_datasources(&[("family", "sql".into()), ("state", "active".into())])
            .unwrap();
        assert!(resp.datasources.is_empty());
        plain.assert();
        filtered.assert();
    }

    #[test]
    fn delete_datasource_409s_while_ingests_reference_it() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("DELETE", "/ingest/datasources/ds_1")
            .with_status(409)
            .with_body(
                r#"{"error":{"code":"active_ingests_exist",
                    "message":"2 ingests still reference ds_1"}}"#,
            )
            .create();

        let err = api_key_client(&server)
            .delete_datasource("ds_1")
            .unwrap_err();
        m.assert();
        match &err {
            IngestError::Http { status, .. } => assert_eq!(*status, 409),
            other => panic!("expected Http, got: {}", other.message()),
        }
        // Both halves of the error envelope reach the user.
        let msg = err.message();
        assert!(msg.contains("2 ingests still reference ds_1"), "{msg}");
        assert!(msg.contains("active_ingests_exist"), "{msg}");
    }

    // --- ingests -----------------------------------------------------------

    #[test]
    fn create_ingest_posts_structured_selector_and_destination() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/ingests")
            .match_header("authorization", "Bearer hd_test")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "datasource_id": "ds_pg_prod",
                "type": "one_time",
                "selector": {"mode": "tables", "schema": "public",
                             "tables": ["orders"]},
                "destination": {"database_id": "db_123", "schema": "public",
                                "table": "orders", "write_mode": "replace"},
            })))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ingest_id":"ing_01J","datasource_id":"ds_pg_prod","type":"one_time",
                    "state":"active","initial_run_id":"run_01J"}"#,
            )
            .create();

        let ing = api_key_client(&server)
            .create_ingest(&IngestCreate {
                datasource_id: "ds_pg_prod".into(),
                r#type: "one_time".into(),
                selector: serde_json::json!({
                    "mode": "tables", "schema": "public", "tables": ["orders"]
                }),
                destination: serde_json::json!({
                    "database_id": "db_123", "schema": "public",
                    "table": "orders", "write_mode": "replace"
                }),
                schedule: None,
            })
            .unwrap();
        m.assert();
        assert_eq!(ing.ingest_id, "ing_01J");
        assert_eq!(ing.initial_run_id.as_deref(), Some("run_01J"));
    }

    #[test]
    fn create_ingest_409s_on_a_destination_table_conflict() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/ingests")
            .with_status(409)
            .with_body(
                r#"{"error":{"code":"destination_table_conflict",
                    "message":"continuous filesystem ingest already owns db_456.public.orders_raw",
                    "details":{"conflicting_ingest_id":"ing_old"}}}"#,
            )
            .create();

        let err = api_key_client(&server)
            .create_ingest(&IngestCreate {
                datasource_id: "ds_s3".into(),
                r#type: "continuous".into(),
                selector: serde_json::json!({}),
                destination: serde_json::json!({}),
                schedule: None,
            })
            .unwrap_err();
        m.assert();
        let msg = err.message();
        assert!(
            msg.contains("already owns db_456.public.orders_raw"),
            "{msg}"
        );
        assert!(msg.contains("destination_table_conflict"), "{msg}");
    }

    #[test]
    fn list_ingests_filters_by_datasource_id() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/ingests?datasource_id=ds_1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(WORKER_INGEST_LIST_BODY)
            .create();

        let resp = api_key_client(&server)
            .list_ingests(&[("datasource_id", "ds_1".into())])
            .unwrap();
        m.assert();
        let ing = &resp.ingests[0];
        assert_eq!(ing.ingest_id, "ing_1");
        assert_eq!(ing.r#type.as_deref(), Some("continuous"));
        assert_eq!(ing.destination.as_ref().unwrap()["table"], "orders_raw");
    }

    #[test]
    fn cancel_reports_the_cancelled_run_and_the_stopped_state() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/ingests/ing_1/cancel")
            .match_header("x-workspace-id", "ws-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ingest_id":"ing_1","state":"stopped","cancelled_run_id":"run_9",
                    "stopped":true}"#,
            )
            .create();

        let ack = api_key_client(&server).cancel_ingest("ing_1").unwrap();
        m.assert();
        assert_eq!(ack.state.as_deref(), Some("stopped"));
        assert_eq!(ack.cancelled_run_id.as_deref(), Some("run_9"));
        assert!(ack.stopped);
    }

    #[test]
    fn resume_returns_the_active_ingest_without_a_new_run() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/ingests/ing_1/resume")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ingest_id":"ing_1","state":"active","type":"scheduled",
                    "next_attempt_at":"2026-08-13T12:05:00+00:00"}"#,
            )
            .create();

        let ing = api_key_client(&server).resume_ingest("ing_1").unwrap();
        m.assert();
        assert_eq!(ing.state.as_deref(), Some("active"));
        // No run id anywhere in the resume ack — DR-12.
        assert!(ing.initial_run_id.is_none());
    }

    #[test]
    fn schedule_patch_sends_the_schedule_envelope() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("PATCH", "/ingest/ingests/ing_1/schedule")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "schedule": {"interval_seconds": 300, "next_run_at": "now"},
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ingest_id":"ing_1","state":"active","type":"continuous",
                    "schedule":{"interval_seconds":300}}"#,
            )
            .create();

        api_key_client(&server)
            .update_schedule(
                "ing_1",
                &SchedulePatch {
                    schedule: serde_json::json!({"interval_seconds": 300, "next_run_at": "now"}),
                },
            )
            .unwrap();
        m.assert();
    }

    #[test]
    fn schedule_patch_409s_for_a_one_time_ingest() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("PATCH", "/ingest/ingests/ing_1/schedule")
            .with_status(409)
            .with_body(
                r#"{"error":{"code":"unsupported_ingest_type",
                    "message":"one_time ingests have no schedule"}}"#,
            )
            .create();

        let err = api_key_client(&server)
            .update_schedule(
                "ing_1",
                &SchedulePatch {
                    schedule: serde_json::json!({"interval_seconds": 60}),
                },
            )
            .unwrap_err();
        m.assert();
        assert!(
            err.message().contains("unsupported_ingest_type"),
            "{}",
            err.message()
        );
    }

    // --- runs --------------------------------------------------------------

    #[test]
    fn list_runs_decodes_newest_first_with_snapshots() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/ingests/ing_1/runs")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"runs":[
                    {"run_id":"run_3","ingest_id":"ing_1","status":"running","stage":"loading",
                     "attempt":3,"started_at":"2026-08-13T10:10:00+00:00"},
                    {"run_id":"run_2","ingest_id":"ing_1","status":"failed","detail":"boom",
                     "attempt":2,"finished_at":"2026-08-13T10:06:00+00:00"}
                ]}"#,
            )
            .create();

        let resp = api_key_client(&server).list_runs("ing_1", &[]).unwrap();
        m.assert();
        assert_eq!(resp.runs.len(), 2);
        assert_eq!(resp.runs[0].run_id, "run_3");
        assert_eq!(resp.runs[0].stage.as_deref(), Some("loading"));
        assert_eq!(resp.runs[1].status, "failed");
    }

    #[test]
    fn list_runs_filters_by_status() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/ingests/ing_1/runs?status=failed")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"runs":[]}"#)
            .create();

        api_key_client(&server)
            .list_runs("ing_1", &[("status", "failed".into())])
            .unwrap();
        m.assert();
    }

    #[test]
    fn get_run_carries_the_config_version_it_used() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/runs/run_2")
            .match_header("authorization", "Bearer eyJ.fake.jwt")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"run_id":"run_2","ingest_id":"ing_1","datasource_id":"ds_1",
                    "config_version_id":"dscv_1","attempt":2,"status":"succeeded",
                    "destination_snapshot":{"database_id":"db_1","table":"orders"},
                    "job_name":"drain-run-2","queued_at":"2026-08-13T10:05:00+00:00",
                    "finished_at":"2026-08-13T10:06:00+00:00",
                    "unknown_future_field":"ignored"}"#,
            )
            .create();

        let run = jwt_client(&server).get_run("run_2").unwrap();
        m.assert();
        assert_eq!(run.config_version_id.as_deref(), Some("dscv_1"));
        assert_eq!(run.destination_snapshot.unwrap()["table"], "orders");
        assert_eq!(run.job_name.as_deref(), Some("drain-run-2"));
    }

    #[test]
    fn missing_run_surfaces_the_404_code() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/runs/nope")
            .with_status(404)
            .with_body(r#"{"error":{"code":"run_not_found","message":"no run 'nope'"}}"#)
            .create();

        let err = api_key_client(&server).get_run("nope").unwrap_err();
        m.assert();
        match err {
            IngestError::Http { status, body } => {
                assert_eq!(status, 404);
                assert!(body.contains("run_not_found"), "got: {body}");
            }
            other => panic!("expected Http, got: {}", other.message()),
        }
    }

    // --- catalog -----------------------------------------------------------

    #[test]
    fn connectors_decodes_rest_template_and_auth() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/connectors")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"connectors":[
                    {"name":"postgres","family":"sql","description":"PostgreSQL"},
                    {"name":"aikido","family":"rest","auth":"oauth_client_credentials",
                     "description":"Security posture",
                     "template":{"client":{"base_url":"https://app.aikido.dev/api/public/v1/",
                                 "auth":{"type":"oauth2_client_credentials","client_id":"<CLIENT_ID>"}}}}
                ]}"#,
            )
            .create();

        let resp = api_key_client(&server).connectors().unwrap();
        m.assert();
        assert_eq!(resp.connectors.len(), 2);
        let pg = &resp.connectors[0];
        assert_eq!(pg.family, "sql");
        assert!(pg.template.is_none() && pg.auth.is_none());
        let aikido = &resp.connectors[1];
        assert_eq!(aikido.auth.as_deref(), Some("oauth_client_credentials"));
        assert_eq!(
            aikido.template.as_ref().unwrap()["client"]["base_url"],
            "https://app.aikido.dev/api/public/v1/"
        );
    }

    // --- debug-log redaction -------------------------------------------------

    #[test]
    fn redact_secret_fields_masks_all_secret_subtrees_and_keeps_the_rest() {
        let body = serde_json::json!({
            "family": "sql",
            "display_name": "prod postgres",
            "config": {"dialect": "postgres", "host": "pg.example.com"},
            "credentials": {"connection_string": "postgresql://u:s3cret@h/db"},
        });
        let logged = redact_secret_fields(&body);
        for key in super::SECRET_BODY_FIELDS {
            assert_eq!(
                logged[*key], "***",
                "{key} must be dropped from the debug view"
            );
        }
        let printed = logged.to_string();
        assert!(
            !printed.contains("s3cret"),
            "no secret may survive into the printable body: {printed}"
        );
        // Non-secret fields stay readable, and the wire body is untouched.
        assert_eq!(logged["family"], "sql");
        assert_eq!(logged["display_name"], "prod postgres");
        assert_eq!(
            body["credentials"]["connection_string"],
            "postgresql://u:s3cret@h/db"
        );
    }

    // --- request serialization ----------------------------------------------

    #[test]
    fn datasource_config_omits_unset_fields() {
        // The worker applies its own defaults; nulls must not be sent, and an
        // omitted `credentials` key is what "inherit" means on the wire.
        let req = DatasourceConfig {
            family: "filesystem".into(),
            config: serde_json::json!({"provider": "s3", "root_uri": "s3://b"}),
            ..Default::default()
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "family": "filesystem",
                "config": {"provider": "s3", "root_uri": "s3://b"},
            })
        );
    }

    #[test]
    fn ingest_create_serializes_type_as_the_wire_name() {
        let req = IngestCreate {
            datasource_id: "ds_1".into(),
            r#type: "one_time".into(),
            selector: serde_json::json!({"mode": "tables"}),
            destination: serde_json::json!({"database_id": "db_1"}),
            schedule: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["type"], "one_time");
        assert!(v.get("schedule").is_none());
    }
}
