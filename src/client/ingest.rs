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
            IngestError::Http { status, body } => format!("HTTP {status}: {body}"),
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
        if matches!(self, IngestError::NeedsApiKey) {
            eprintln!(
                "{}",
                "Pass --api-key or set HOTDATA_API_KEY with a workspace API token (hd_...)."
                    .dark_grey()
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
        let (token, token_is_api_key) = match profile.api_key.clone() {
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
        self.send(
            self.authed(reqwest::Method::POST, "/sources").json(&body),
            Some(&body),
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

    pub fn translate(
        &self,
        text: &str,
        connections: serde_json::Value,
    ) -> Result<serde_json::Value, IngestError> {
        let body = serde_json::json!({ "text": text, "connections": connections });
        self.send(
            self.authed(reqwest::Method::POST, "/translate").json(&body),
            Some(&body),
        )
    }

    pub fn drain(&self) -> Result<serde_json::Value, IngestError> {
        self.send(self.authed(reqwest::Method::POST, "/jobs/drain"), None)
    }

    // --- read endpoints --------------------------------------------------

    pub fn connectors(&self) -> Result<ConnectorsResponse, IngestError> {
        self.send(self.authed(reqwest::Method::GET, "/connectors"), None)
    }

    pub fn job_status(&self, ingest_id: &str) -> Result<JobStatus, IngestError> {
        self.send(
            self.authed(reqwest::Method::GET, &format!("/jobs/{ingest_id}")),
            None,
        )
    }

    pub fn schema(&self, database_id: &str) -> Result<serde_json::Value, IngestError> {
        self.send(
            self.authed(
                reqwest::Method::GET,
                &format!("/databases/{database_id}/schema"),
            ),
            None,
        )
    }

    pub fn preview(&self, database_id: &str, limit: u32) -> Result<serde_json::Value, IngestError> {
        self.send(
            self.authed(
                reqwest::Method::GET,
                &format!("/databases/{database_id}/preview"),
            )
            .query(&[("limit", limit)]),
            None,
        )
    }

    /// Download a loaded table as parquet, returning the raw bytes and whether
    /// the server truncated the result (`X-Truncated: true`).
    pub fn download(
        &self,
        database_id: &str,
        table: &str,
        limit: u32,
    ) -> Result<(Vec<u8>, bool), IngestError> {
        let req = self
            .authed(
                reqwest::Method::GET,
                &format!("/databases/{database_id}/tables/{table}/parquet"),
            )
            .query(&[("limit", limit)]);
        let resp = req
            .send()
            .map_err(|e| IngestError::Connection(e.to_string()))?;
        let status = resp.status();
        let truncated = resp
            .headers()
            .get("X-Truncated")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let bytes = resp
            .bytes()
            .map_err(|e| IngestError::Connection(e.to_string()))?;
        if !status.is_success() {
            return Err(IngestError::Http {
                status: status.as_u16(),
                body: String::from_utf8_lossy(&bytes).into_owned(),
            });
        }
        Ok((bytes.to_vec(), truncated))
    }
}

// --- request / response types -------------------------------------------

/// Mirrors the worker's `IngestRequest`. Sensitive fields (`credentials`,
/// `rest_config`, `catalog_config`) are forwarded over TLS and Fernet-encrypted
/// at rest by the worker. `None`/empty fields are omitted so the worker applies
/// its own defaults.
#[derive(Serialize, Default)]
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

#[derive(Deserialize)]
pub struct ConnectorsResponse {
    pub connectors: Vec<ConnectorEntry>,
}

#[derive(Deserialize)]
pub struct ConnectorEntry {
    pub name: String,
    #[serde(default)]
    pub family: String,
    #[serde(default)]
    pub description: String,
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
    fn download_returns_bytes_and_truncated_flag() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/ingest/databases/db-1/tables/users/parquet")
            .match_query(mockito::Matcher::UrlEncoded("limit".into(), "100".into()))
            .with_status(200)
            .with_header("X-Truncated", "true")
            .with_body(b"PAR1fake".as_slice())
            .create();

        let (bytes, truncated) = api_key_client(&server)
            .download("db-1", "users", 100)
            .unwrap();
        m.assert();
        assert_eq!(bytes, b"PAR1fake");
        assert!(truncated);
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
