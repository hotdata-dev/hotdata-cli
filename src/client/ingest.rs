//! Raw-HTTP client for the ingest API (`/v1/ingest/*`).
//!
//! These routes are not in the generated SDK yet, so — like the token/session
//! mints — they ride the hand-rolled `reqwest::blocking` seam in
//! [`crate::client::raw_http`].
//! Every request carries a bearer + `X-Workspace-Id`; the gateway validates the
//! pair and derives the ingest destination server-side, so the CLI never sends
//! a destination.
//!
//! Auth is split by endpoint kind. The enqueue routes (`POST /sources`,
//! `POST /queries`) require a durable `hd_...` API key as the bearer — the
//! drain job uses the credential *after* the request returns, so the server
//! 422s any 5-minute JWT. Read routes accept *workspace-scoped* JWTs, but the
//! CLI's login session is a user-scoped JWT and the worker refuses to trust
//! `X-Workspace-Id` on the JWT route — so in practice only `/connectors`
//! (workspace-free) works without a key. So: when an API key is available
//! (`--api-key` / `HOTDATA_API_KEY`) it is sent directly on every call;
//! otherwise the CLI's session JWT ([`jwt::ensure_access_token`]) is used,
//! enqueue calls fail fast with [`IngestError::NeedsApiKey`], and
//! workspace-scoped reads get a 403 with an `--api-key` hint.
//!
//! When the ingest routes land in the public OpenAPI and the SDK regenerates,
//! delete this module and move the commands onto `sdk::Api`.
//!
//! Surface: `create_source` (onboard, backs `new-connection`), `create_query`
//! (SQL front-door, backs `new-import`), `list_sources` / `list_queries` (the
//! registries behind `list-connections` / `list-imports`), `rerun` (backs
//! `trigger-import`), `connectors` (the catalog), `drain` + `job_status` (the
//! async run loop). The result-reading endpoints are intentionally absent —
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
    /// Enqueue attempted with only a session JWT — the server would 422 it
    /// (the drain job runs after a short-lived JWT expires), so fail before
    /// sending credentials that can't work.
    NeedsApiKey,
}

impl IngestError {
    pub fn message(&self) -> String {
        match self {
            // util::api_error extracts the server's human message from the
            // JSON body shapes in the wild, matching every SDK-backed command.
            IngestError::Http { status, body } => {
                format!("HTTP {status}: {}", util::api_error(body.clone()))
            }
            IngestError::Connection(e) => format!("connection error: {e}"),
            IngestError::Decode(e) => format!("malformed response: {e}"),
            IngestError::NeedsApiKey => {
                "enqueueing an ingest requires a durable API key — the ingest pipeline \
                 runs after a short-lived session token would expire"
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
        // A transport failure on an enqueue is usually the worker being
        // unavailable — a cold start or a rollout — where the gateway holds the
        // connection until its timeout rather than returning a status. "error
        // sending request" is opaque; point at the actual cause + retry.
        if matches!(self, IngestError::Connection(_)) {
            eprintln!(
                "{}",
                "the request didn't complete — the ingest service may be starting up or \
                 redeploying; retry in a moment (a timed-out enqueue is safe to re-run)."
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
                "Your CLI session is user-scoped; ingest needs a workspace credential — \
                 pass --api-key or set HOTDATA_API_KEY."
                    .dark_grey()
            );
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
    /// Enqueue endpoints require the former; see the module docs.
    token_is_api_key: bool,
    workspace_id: String,
    client: reqwest::blocking::Client,
}

impl IngestClient {
    /// Build a client for `workspace_id`. An explicit API key (`--api-key` /
    /// `HOTDATA_API_KEY`) is sent as the bearer directly — the extAuth route
    /// accepts it everywhere and the enqueue routes *require* it. Without one,
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
        // drain load fails Forbidden — so ingest always uses the workspace
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

    // --- write endpoints -------------------------------------------------

    /// Guard for the enqueue routes: a session JWT is rejected server-side
    /// (422) because the drain job outlives it, so fail fast with a message
    /// that says what to do instead.
    fn require_api_key(&self) -> Result<(), IngestError> {
        if self.token_is_api_key {
            Ok(())
        } else {
            Err(IngestError::NeedsApiKey)
        }
    }

    pub fn create_source(&self, req: &IngestRequest) -> Result<IngestAck, IngestError> {
        self.require_api_key()?;
        let body = serde_json::to_value(req).expect("IngestRequest serializes");
        let body_log = redact_secret_fields(&body);
        self.send(
            self.authed(reqwest::Method::POST, "/sources").json(&body),
            Some(&body_log),
        )
    }

    pub fn create_query(&self, req: &QueryToIngest) -> Result<IngestAck, IngestError> {
        self.require_api_key()?;
        let body = serde_json::to_value(req).expect("QueryToIngest serializes");
        self.send(
            self.authed(reqwest::Method::POST, "/queries").json(&body),
            Some(&body),
        )
    }

    pub fn drain(&self) -> Result<serde_json::Value, IngestError> {
        self.send(self.authed(reqwest::Method::POST, "/jobs/drain"), None)
    }

    /// Re-run an ingest: the worker resets it to pending and fires the drain.
    /// Replace-mode loads + the stored (encrypted) source and destination
    /// credentials mean this refreshes the SAME managed DB from source. No
    /// API-key guard: the drain reuses the destination stored at enqueue, so
    /// the server accepts any credential its write check does. A 409 means a
    /// drain is mid-flight — the worker's message says to retry shortly.
    pub fn rerun(&self, ingest_id: &str) -> Result<IngestAck, IngestError> {
        self.send(
            self.authed(reqwest::Method::POST, &format!("/jobs/{ingest_id}/rerun")),
            None,
        )
    }

    // --- read endpoints --------------------------------------------------

    /// The connector catalog. REST entries carry a ready-to-edit `template`
    /// (a dlt `rest_api` config with the service's `base_url`, auth shape, and
    /// resources pre-filled and `<PLACEHOLDER>` secrets) — this is what lets
    /// the `new` wizard fill in everything but the caller's secrets.
    pub fn connectors(&self) -> Result<ConnectorsResponse, IngestError> {
        self.send(self.authed(reqwest::Method::GET, "/connectors"), None)
    }

    pub fn job_status(&self, ingest_id: &str) -> Result<JobStatus, IngestError> {
        self.send(
            self.authed(reqwest::Method::GET, &format!("/jobs/{ingest_id}")),
            None,
        )
    }

    /// Remove a connection from the registry (`DELETE /sources/{id}`):
    /// the request row (with its stored encrypted credentials) and any run
    /// row. The discovery database is untouched — the command layer decides
    /// its fate. The server 422s import rows and 409s while a drain is in
    /// flight.
    pub fn delete_source(&self, ingest_id: &str) -> Result<DeleteSourceAck, IngestError> {
        self.send(
            self.authed(reqwest::Method::DELETE, &format!("/sources/{ingest_id}")),
            None,
        )
    }

    /// The onboarded-source registry (`GET /sources`) — one row per connection,
    /// newest first, each with its own ingest id (the connection id). The
    /// default view is the current set: the latest onboard per connector name
    /// plus unnamed onboards; `all` includes superseded onboards too.
    pub fn list_sources(&self, all: bool) -> Result<SourcesResponse, IngestError> {
        let path = if all { "/sources?all=true" } else { "/sources" };
        self.send(self.authed(reqwest::Method::GET, path), None)
    }

    /// The import registry (`GET /queries`) — every SQL import, newest first,
    /// with the SQL that produced it, its source connection, and the managed
    /// DB it landed in.
    pub fn list_queries(&self) -> Result<QueriesResponse, IngestError> {
        self.send(self.authed(reqwest::Method::GET, "/queries"), None)
    }

    /// Tables + columns discovered for a result database — the schema preview a
    /// metadata-only onboard lands: `{database_id, tables: {table: [columns]}}`.
    pub fn schema(&self, database_id: &str) -> Result<serde_json::Value, IngestError> {
        self.send(
            self.authed(
                reqwest::Method::GET,
                &format!("/databases/{database_id}/schema"),
            ),
            None,
        )
    }
}

/// `IngestRequest` fields that carry source secrets: SQL passwords /
/// connection strings, REST bearer tokens inside `rest_config`, Iceberg
/// catalog tokens, filesystem access keys.
const SECRET_BODY_FIELDS: &[&str] = &["credentials", "rest_config", "catalog_config"];

/// Debug-log view of an enqueue body with the secret-bearing subtrees
/// replaced wholesale. These fields are nested *objects* whose secret keys
/// vary by connector, so dropping the whole subtree beats field-level
/// masking (`util::redact_json_fields` only masks string values). Mirrors
/// the `redacted_form_body` pattern in `jwt.rs`.
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

// --- request / response types -------------------------------------------

/// Mirrors the worker's `IngestRequest`. Sensitive fields (`credentials`,
/// `rest_config`, `catalog_config`) are forwarded over TLS and Fernet-encrypted
/// at rest by the worker. `None`/empty fields are omitted so the worker applies
/// its own defaults.
#[derive(Debug, Serialize, Default)]
pub struct IngestRequest {
    pub family: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector_type: Option<String>,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub credentials: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rest_config: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub table_names: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_glob: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_config: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub validate_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_id: Option<String>,
}

/// Mirrors the worker's `QueryToIngestRequest`.
#[derive(Serialize)]
pub struct QueryToIngest {
    pub query: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ingest_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_id: Option<String>,
}

/// 202 body from `POST /sources` and `POST /queries`. Common fields are typed;
/// everything else (e.g. `resource`, `columns`, `where`, `limit` on queries) is
/// captured for `--output json`.
#[derive(Debug, Deserialize, Serialize)]
pub struct IngestAck {
    pub ingest_id: String,
    pub database_id: String,
    pub status: String,
    #[serde(default)]
    pub status_url: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `GET /jobs/{id}` body.
#[derive(Debug, Deserialize, Serialize)]
pub struct JobStatus {
    pub ingest_id: String,
    pub status: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub connector_type: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub database_id: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// `DELETE /sources/{id}` body.
#[derive(Debug, Deserialize, Serialize)]
pub struct DeleteSourceAck {
    pub ingest_id: String,
    #[serde(default)]
    pub connector_type: Option<String>,
    #[serde(default)]
    pub database_id: Option<String>,
    pub deleted: bool,
}

/// `GET /sources` body.
#[derive(Deserialize)]
pub struct SourcesResponse {
    pub sources: Vec<SourceRow>,
}

/// One connection in the onboarded-source registry. `ingest_id` is the
/// connection's own id — the pin key `new-import --source` accepts. `active`
/// marks the row by-name resolution picks (always true in the default view;
/// superseded onboards surface with `all=true`).
#[derive(Debug, Deserialize, Serialize)]
pub struct SourceRow {
    pub ingest_id: String,
    #[serde(default)]
    pub connector_type: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub database_id: Option<String>,
    pub status: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub active: bool,
}

/// `GET /queries` body.
#[derive(Deserialize)]
pub struct QueriesResponse {
    pub queries: Vec<QueryRow>,
}

/// One import in the registry: the SQL that produced it (verbatim for new
/// rows, server-reconstructed for older ones), the connection it drew from,
/// and the managed DB it landed in.
#[derive(Debug, Deserialize, Serialize)]
pub struct QueryRow {
    pub ingest_id: String,
    pub source_ingest_id: String,
    #[serde(default)]
    pub connector_type: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub database_id: Option<String>,
    pub status: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Deserialize)]
pub struct ConnectorsResponse {
    pub connectors: Vec<ConnectorEntry>,
}

/// One catalog entry. `sql` names are dialects, `filesystem`/`iceberg`/`rest`
/// are family templates. REST entries additionally carry `auth` (the method
/// name, e.g. `bearer`, `oauth_client_credentials`, `none`) and a `template`
/// dlt config with `<PLACEHOLDER>` secrets the wizard prompts for.
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

    // --- enqueue auth ------------------------------------------------------

    #[test]
    fn create_source_sends_api_key_bearer_and_workspace_header() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/sources")
            .match_header("authorization", "Bearer hd_test")
            .match_header("x-workspace-id", "ws-1")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "family": "sql",
                "connector_type": "postgres",
                "credentials": {"connection_string": "postgresql://u:p@h:5432/d"},
                "validate_only": true,
            })))
            .with_status(202)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ingest_id":"ing-1","database_id":"db-1","workspace_id":"ws-1",
                    "status":"pending","status_url":"/ingest/jobs/ing-1"}"#,
            )
            .create();

        let req = IngestRequest {
            family: "sql".into(),
            connector_type: Some("postgres".into()),
            credentials: serde_json::json!({"connection_string": "postgresql://u:p@h:5432/d"}),
            validate_only: true,
            ..Default::default()
        };
        let ack = api_key_client(&server).create_source(&req).unwrap();
        m.assert();
        assert_eq!(ack.ingest_id, "ing-1");
        assert_eq!(ack.database_id, "db-1");
        assert_eq!(ack.status, "pending");
        // Untyped extras (workspace_id, …) survive for --output json.
        assert_eq!(ack.extra["workspace_id"], "ws-1");
    }

    #[test]
    fn enqueue_with_session_jwt_fails_fast_without_http() {
        // Point at a dead port: reaching the network would surface as a
        // Connection error instead of NeedsApiKey.
        let client = IngestClient::from_parts("http://127.0.0.1:1", "eyJ.fake.jwt", false, "ws-1");

        let source_err = client.create_source(&IngestRequest::default()).unwrap_err();
        assert!(matches!(source_err, IngestError::NeedsApiKey));

        let query_err = client
            .create_query(&QueryToIngest {
                query: "SELECT * FROM x".into(),
                source_ingest_id: None,
                database_id: None,
            })
            .unwrap_err();
        assert!(matches!(query_err, IngestError::NeedsApiKey));
        assert!(
            query_err.message().contains("API key"),
            "got: {}",
            query_err.message()
        );
    }

    // --- read endpoints accept a JWT ---------------------------------------

    #[test]
    fn job_status_works_with_jwt_and_decodes_body() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/jobs/ing-1")
            .match_header("authorization", "Bearer eyJ.fake.jwt")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ingest_id":"ing-1","status":"failed","detail":"boom",
                    "connector_type":"postgres","family":"sql","database_id":"db-1",
                    "created_at":"2026-07-07T15:30:10+00:00",
                    "updated_at":"2026-07-07T15:30:45+00:00",
                    "dlt_workspace_id":"ignored-unknown-field"}"#,
            )
            .create();

        let st = jwt_client(&server).job_status("ing-1").unwrap();
        m.assert();
        assert_eq!(st.status, "failed");
        assert_eq!(st.detail.as_deref(), Some("boom"));
        assert_eq!(st.database_id.as_deref(), Some("db-1"));
    }

    #[test]
    fn http_error_carries_status_and_body() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/jobs/nope")
            .with_status(404)
            .with_body(r#"{"detail":"ingest not found"}"#)
            .create();

        let err = api_key_client(&server).job_status("nope").unwrap_err();
        m.assert();
        match err {
            IngestError::Http { status, body } => {
                assert_eq!(status, 404);
                assert!(body.contains("ingest not found"), "got: {body}");
            }
            other => panic!("expected Http, got: {}", other.message()),
        }
    }

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

    // --- registry reads ------------------------------------------------------

    #[test]
    fn list_sources_default_and_all_hit_the_right_paths() {
        let mut server = mockito::Server::new();
        let m_default = server
            .mock("GET", "/ingest/sources")
            .match_header("x-workspace-id", "ws-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"sources":[{"ingest_id":"a1","connector_type":"postgres","family":"sql",
                    "database_id":"db-1","status":"done","detail":null,
                    "created_at":"2026-07-08T10:00:00+00:00","updated_at":null,"active":true}]}"#,
            )
            .create();
        let m_all = server
            .mock("GET", "/ingest/sources?all=true")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"sources":[]}"#)
            .create();

        let client = api_key_client(&server);
        let resp = client.list_sources(false).unwrap();
        assert_eq!(resp.sources.len(), 1);
        let s = &resp.sources[0];
        assert_eq!(s.ingest_id, "a1");
        assert_eq!(s.connector_type.as_deref(), Some("postgres"));
        assert!(s.active);

        assert!(client.list_sources(true).unwrap().sources.is_empty());
        m_default.assert();
        m_all.assert();
    }

    #[test]
    fn list_queries_decodes_sql_and_source_link() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/queries")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"queries":[{"ingest_id":"q1","source_ingest_id":"a1",
                    "connector_type":"bitcoin","family":"rest",
                    "query":"SELECT id, height FROM bitcoin.blocks LIMIT 100",
                    "database_id":"db-2","status":"done","detail":null,
                    "created_at":"2026-07-08T10:05:00+00:00","updated_at":null}]}"#,
            )
            .create();

        let resp = api_key_client(&server).list_queries().unwrap();
        m.assert();
        let q = &resp.queries[0];
        assert_eq!(q.source_ingest_id, "a1");
        assert_eq!(
            q.query.as_deref(),
            Some("SELECT id, height FROM bitcoin.blocks LIMIT 100")
        );
        assert_eq!(q.database_id.as_deref(), Some("db-2"));
    }

    #[test]
    fn rerun_posts_and_decodes_the_pending_ack() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/jobs/q1/rerun")
            .match_header("authorization", "Bearer hd_test")
            .match_header("x-workspace-id", "ws-1")
            .with_status(202)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ingest_id":"q1","database_id":"db-2","connector_type":"bitcoin",
                    "family":"rest","status":"pending","status_url":"/ingest/jobs/q1"}"#,
            )
            .create();

        let ack = api_key_client(&server).rerun("q1").unwrap();
        m.assert();
        assert_eq!(ack.ingest_id, "q1");
        assert_eq!(ack.database_id, "db-2");
        assert_eq!(ack.status, "pending");
    }

    #[test]
    fn rerun_409_surfaces_the_in_flight_detail() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/ingest/jobs/q1/rerun")
            .with_status(409)
            .with_body(r#"{"detail":"a drain appears to be running (in flight: a1); retry once it settles"}"#)
            .create();

        let err = api_key_client(&server).rerun("q1").unwrap_err();
        m.assert();
        match err {
            IngestError::Http { status, body } => {
                assert_eq!(status, 409);
                assert!(body.contains("retry once it settles"), "got: {body}");
            }
            other => panic!("expected Http, got: {}", other.message()),
        }
    }

    #[test]
    fn delete_source_removes_and_acks() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("DELETE", "/ingest/sources/a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1")
            .match_header("authorization", "Bearer hd_test")
            .match_header("x-workspace-id", "ws-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ingest_id":"a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
                    "connector_type":"postgres","database_id":"db-1","deleted":true}"#,
            )
            .create();

        let ack = api_key_client(&server)
            .delete_source("a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1")
            .unwrap();
        m.assert();
        assert!(ack.deleted);
        assert_eq!(ack.database_id.as_deref(), Some("db-1"));
    }

    #[test]
    fn delete_source_surfaces_import_422() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("DELETE", "/ingest/sources/b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2")
            .with_status(422)
            .with_body(r#"{"detail":"that ingest is an import, not a connection"}"#)
            .create();

        let err = api_key_client(&server)
            .delete_source("b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2")
            .unwrap_err();
        m.assert();
        match err {
            IngestError::Http { status, body } => {
                assert_eq!(status, 422);
                assert!(body.contains("import"), "got: {body}");
            }
            other => panic!("expected Http, got: {}", other.message()),
        }
    }

    // --- debug-log redaction -------------------------------------------------

    #[test]
    fn redact_secret_fields_masks_all_secret_subtrees_and_keeps_the_rest() {
        let body = serde_json::json!({
            "family": "rest",
            "connector_type": "svc",
            "credentials": {"connection_string": "postgresql://u:s3cret@h/db"},
            "rest_config": {"client": {"auth": {"type": "bearer", "token": "tok-abc"}}},
            "catalog_config": {"catalog_uri": "http://c", "token": "iceberg-tok"},
            "table_names": ["users"],
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
            !printed.contains("s3cret")
                && !printed.contains("tok-abc")
                && !printed.contains("iceberg-tok"),
            "no secret may survive into the printable body: {printed}"
        );
        // Non-secret fields stay readable, and the wire body is untouched.
        assert_eq!(logged["family"], "rest");
        assert_eq!(logged["table_names"][0], "users");
        assert_eq!(
            body["credentials"]["connection_string"],
            "postgresql://u:s3cret@h/db"
        );
    }

    // --- request serialization ----------------------------------------------

    #[test]
    fn ingest_request_omits_unset_fields() {
        // The worker applies its own defaults; nulls/empties must not be sent.
        let req = IngestRequest {
            family: "filesystem".into(),
            bucket_url: Some("s3://b/prefix".into()),
            ..Default::default()
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"family": "filesystem", "bucket_url": "s3://b/prefix"})
        );
    }
}
