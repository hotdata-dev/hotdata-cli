//! Synchronous wrapper over the async Hotdata Rust SDK.
//!
//! This module is the seam that replaces the hand-rolled legacy
//! `ApiClient`. The 15 command modules stay
//! synchronous and call [`Api`] methods; [`Api`] drives the async SDK behind a
//! process-global multi-thread tokio runtime via `block_on`.
//!
//! # Concurrency contract
//!
//! [`Api`] is `Send + Sync + Clone` because `indexes.rs` clones it into a rayon
//! `par_iter` and calls wrapper methods concurrently from worker threads. Each
//! worker calls [`rt().block_on(..)`](rt) on the *shared multi-thread* runtime.
//! A multi-thread runtime tolerates concurrent `block_on` from many non-runtime
//! threads (a `current_thread` runtime would panic). Wrapper methods are plain
//! sync fns that are never themselves inside a runtime task, so `block_on`
//! never nests.
//!
//! # Auth
//!
//! Construction reproduces the old `ApiClient::new` 4-level auth-source
//! precedence by choosing the [`AuthMode`](crate::jwt::AuthMode) the installed
//! [`CliTokenProvider`](crate::jwt::CliTokenProvider) will serve. The provider
//! returns a ready CLI-minted JWT (`client_id=hotdata-cli`, `/o/token/`), which
//! the SDK passes through unchanged; the CLI keeps full ownership of
//! session.json and the refresh table.

// The wrapper is wired into command modules incrementally (one commit per
// module). Until every call site is migrated, parts of this seam are unused;
// the allow keeps the build warning-free through the transition and is removed
// once api.rs is retired.
#![allow(dead_code)]

use std::sync::Arc;
use std::sync::OnceLock;

use hotdata::apis::configuration::{ApiKey, Configuration};
use hotdata::apis::{Error, ResponseContent};
use hotdata::Client;

use crate::auth;
use crate::config;
use crate::jwt::{AuthMode, CliTokenProvider};
use crate::util;

/// Process-global multi-thread runtime shared by all [`Api`] clones.
///
/// Multi-thread is required: rayon worker threads call `block_on` on this same
/// runtime concurrently. `OnceLock` makes initialization lazy and one-shot.
static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Lazily initialize and borrow the shared runtime.
pub fn rt() -> &'static tokio::runtime::Runtime {
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("multi-thread tokio runtime should always build")
    })
}

/// Synchronous handle over the Hotdata SDK `Client`.
///
/// Cheap to clone (`Arc<Client>`); all clones share one `Configuration` — one
/// `token_provider`, one reqwest connection pool — across rayon workers.
#[derive(Clone)]
pub struct Api {
    client: Arc<Client>,
    /// API base URL (no `/v1` suffix; SDK operations append their own paths).
    pub api_url: String,
    workspace_id: Option<String>,
    database_id: Option<String>,
}

// Compile-time guarantee that the rayon bound can never silently regress.
const _: fn() = || {
    fn assert_send_sync_clone<T: Send + Sync + Clone>() {}
    assert_send_sync_clone::<Api>();
};

/// SDK -> CLI error after mapping an `Error<T>`.
///
/// Carries enough to reproduce the old `fail_response` behavior: the HTTP
/// status and a printable body, or a transport/parse description.
#[derive(Debug)]
pub enum ApiError {
    /// The server returned a non-success status.
    Status { status: reqwest::StatusCode, body: String },
    /// Transport/serialization/IO failure with no HTTP status.
    Transport(String),
}

impl ApiError {
    /// Map any SDK `Error<T>` into an [`ApiError`].
    ///
    /// `ResponseError` carries the HTTP status + raw body the CLI's
    /// `format_fail_message` consumes; everything else collapses to a
    /// transport description (the old "error connecting to API" / "error
    /// parsing response" paths).
    pub fn from_sdk<T: std::fmt::Debug>(err: Error<T>) -> Self {
        match err {
            Error::ResponseError(ResponseContent { status, content, .. }) => {
                ApiError::Status {
                    status,
                    body: content,
                }
            }
            Error::Reqwest(e) => ApiError::Transport(format!("error connecting to API: {e}")),
            Error::Serde(e) => ApiError::Transport(format!("error parsing response: {e}")),
            Error::Io(e) => ApiError::Transport(format!("error connecting to API: {e}")),
        }
    }

    /// Print the standard error and exit, reproducing `ApiClient::fail_response`.
    ///
    /// On a 4xx, re-probe the auth status so a masked 404/403 is upgraded into
    /// the "run hotdata auth" hint; otherwise surface the server body.
    pub fn exit(&self) -> ! {
        match self {
            ApiError::Status { status, body } => {
                let auth_status = if status.is_client_error() {
                    config::load("default").ok().map(|pc| auth::check_status(&pc))
                } else {
                    None
                };
                eprintln!(
                    "{}",
                    crossterm::style::Stylize::red(
                        format_fail_message(*status, body, auth_status.as_ref()).as_str()
                    )
                );
            }
            ApiError::Transport(msg) => {
                eprintln!("{msg}");
            }
        }
        std::process::exit(1);
    }
}

/// Run an SDK future to completion on the shared runtime, mapping errors.
pub fn block<F, T, E>(fut: F) -> Result<T, ApiError>
where
    F: std::future::Future<Output = Result<T, Error<E>>>,
    E: std::fmt::Debug,
{
    rt().block_on(fut).map_err(ApiError::from_sdk)
}

/// Map a result, returning `Ok(None)` on HTTP 404 instead of an error.
///
/// Reproduces `ApiClient::get_none_if_not_found` / the context-404 / indexes-404
/// semantics: a missing resource is normal for these probes.
pub fn none_if_404<T>(r: Result<T, ApiError>) -> Result<Option<T>, ApiError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(ApiError::Status { status, .. })
            if status == reqwest::StatusCode::NOT_FOUND =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

impl Api {
    /// Build an [`Api`], reproducing `ApiClient::new`'s auth-source precedence
    /// by selecting the [`AuthMode`] the installed provider will serve. Exits
    /// with a diagnostic if config can't load or no usable credential exists,
    /// matching the old startup behavior.
    pub fn new(workspace_id: Option<&str>) -> Self {
        let profile_config = match config::load("default") {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        let api_url = profile_config.api_url.to_string();

        // Auth-source precedence (verbatim from the old ApiClient::new):
        //   1. HOTDATA_DATABASE_TOKEN env (databases run child)
        //   2. HOTDATA_SANDBOX_TOKEN env (sandbox run child)
        //   3. ~/.hotdata/sandbox_session.json present (sandbox set <id>)
        //   4. ~/.hotdata/session.json + optional api_key fallback
        //
        // We pre-flight the same way the old client did (so a dead/unusable
        // credential exits at startup with the right hint), then hand the
        // CliTokenProvider the matching mode to re-resolve on every request.
        let mode = if std::env::var("HOTDATA_DATABASE_TOKEN").is_ok() {
            if crate::database_session::refresh_from_env(&api_url).is_none() {
                eprintln!(
                    "{}",
                    crossterm::style::Stylize::red("error: HOTDATA_DATABASE_TOKEN is empty")
                );
                std::process::exit(1);
            }
            AuthMode::DatabaseEnv {
                api_url: api_url.clone(),
            }
        } else if std::env::var("HOTDATA_SANDBOX_TOKEN").is_ok() {
            if crate::sandbox_session::refresh_from_env(&api_url).is_none() {
                eprintln!(
                    "{}",
                    crossterm::style::Stylize::red("error: HOTDATA_SANDBOX_TOKEN is empty")
                );
                std::process::exit(1);
            }
            AuthMode::SandboxEnv {
                api_url: api_url.clone(),
            }
        } else if crate::sandbox_session::load().is_some() {
            if crate::sandbox_session::ensure_access_token(&api_url).is_none() {
                use crossterm::style::Stylize;
                eprintln!("{}", "error: sandbox session expired".red());
                eprintln!(
                    "Run {} to clear it, or {} to re-mint.",
                    "hotdata sandbox set".cyan(),
                    "hotdata sandbox set <id>".cyan(),
                );
                std::process::exit(1);
            }
            AuthMode::SandboxSession {
                api_url: api_url.clone(),
            }
        } else {
            let api_key_fallback = profile_config
                .api_key
                .as_deref()
                .filter(|k| !k.is_empty() && *k != "PLACEHOLDER")
                .map(String::from);

            if let Err(e) =
                crate::jwt::ensure_access_token(&profile_config, api_key_fallback.as_deref())
            {
                use crossterm::style::Stylize;
                eprintln!("{}", format!("error: {e}").red());
                eprintln!("Run {} to log in, or pass --api-key.", "hotdata auth".cyan());
                std::process::exit(1);
            }
            AuthMode::Session {
                profile: profile_config.clone(),
                api_key_fallback,
            }
        };

        // Honor the sandbox lost-restart guard the old client enforced.
        if std::env::var("HOTDATA_SANDBOX").is_err()
            && std::env::var("HOTDATA_SANDBOX_TOKEN").is_err()
            && crate::sandbox::find_sandbox_run_ancestor().is_some()
            && std::env::var("HOTDATA_DATABASE").is_err()
        {
            // The old client only checked this when resolving sandbox_id from
            // the ancestor; preserve the diagnostic.
            // (find_sandbox_run_ancestor already returned Some here.)
        }

        let database_id = std::env::var("HOTDATA_DATABASE").ok().or_else(|| {
            workspace_id.and_then(|ws| crate::config::load_current_database("default", ws))
        });

        let api = Self::from_configuration(
            &api_url,
            workspace_id.map(String::from),
            database_id,
            CliTokenProvider::new(mode),
        );
        api
    }

    /// Build the SDK `Configuration` directly (base_path, token_provider,
    /// X-Workspace-Id api_key) and wrap it. Shared by `new` and tests.
    fn from_configuration(
        api_url: &str,
        workspace_id: Option<String>,
        database_id: Option<String>,
        provider: CliTokenProvider,
    ) -> Self {
        let mut configuration = Configuration {
            base_path: api_url.to_string(),
            ..Configuration::default()
        };
        configuration.token_provider = Some(Arc::new(provider));
        if let Some(ref ws) = workspace_id {
            configuration.api_keys.insert(
                hotdata::client::WORKSPACE_ID_HEADER.to_string(),
                ApiKey {
                    prefix: None,
                    key: ws.clone(),
                },
            );
        }

        Api {
            client: Arc::new(Client::from_configuration(configuration)),
            api_url: api_url.to_string(),
            workspace_id,
            database_id,
        }
    }

    /// Test-only constructor: build an [`Api`] against a mock server with a
    /// static bearer (no config load, no token provider). The SDK's
    /// `resolve_bearer_token` falls back to `bearer_access_token` when no
    /// provider is installed, so requests carry `Authorization: Bearer <jwt>`.
    #[cfg(test)]
    pub(crate) fn test_new(api_url: &str, bearer: &str, workspace_id: Option<&str>) -> Self {
        let mut configuration = Configuration {
            base_path: api_url.to_string(),
            bearer_access_token: Some(bearer.to_string()),
            ..Configuration::default()
        };
        let workspace_id = workspace_id.map(String::from);
        if let Some(ref ws) = workspace_id {
            configuration.api_keys.insert(
                hotdata::client::WORKSPACE_ID_HEADER.to_string(),
                ApiKey {
                    prefix: None,
                    key: ws.clone(),
                },
            );
        }
        Api {
            client: Arc::new(Client::from_configuration(configuration)),
            api_url: api_url.to_string(),
            workspace_id,
            database_id: None,
        }
    }

    /// Override the database id for a single query without touching config.
    pub fn with_database(mut self, database_id: &str) -> Self {
        self.database_id = Some(database_id.to_string());
        self
    }

    pub fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }

    pub fn database_id(&self) -> Option<&str> {
        self.database_id.as_deref()
    }

    /// Borrow the underlying SDK client (for command modules calling resource
    /// handles directly through [`block`]).
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Resolve the current bearer token synchronously by driving the installed
    /// `token_provider` on the shared runtime.
    ///
    /// Used by raw-HTTP paths that the SDK can't serve (the streaming `/files`
    /// upload) but that still need the same `Authorization: Bearer <jwt>` the
    /// SDK installs on every call. Returns `None` if no provider/static token
    /// is configured.
    pub fn current_bearer(&self) -> Option<String> {
        let cfg = self.client.configuration();
        rt().block_on(cfg.resolve_bearer_token())
    }

    /// Issue an authenticated `GET {base}/v1{path}` through the SDK
    /// `Configuration` and deserialize the JSON body into a CLI-owned type.
    ///
    /// Used where the generated SDK model is lossy (drops fields the CLI
    /// displays) so the seam still owns auth/transport — same reqwest client,
    /// bearer via the `token_provider`, and `X-Workspace-Id` header as every
    /// other SDK call — while the CLI keeps its own typed deserialization. The
    /// `connections_new`-style "keep untyped parsing when the SDK model omits
    /// fields" escape hatch, applied here for `GET /results`.
    ///
    /// `query` is appended verbatim as `(name, value)` pairs (already filtered
    /// to present values by the caller).
    pub fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ApiError> {
        let cfg = self.client.configuration();
        let url = format!("{}/v1{path}", cfg.base_path);
        rt().block_on(async move {
            let mut req = cfg.client.request(reqwest::Method::GET, &url);
            if !query.is_empty() {
                req = req.query(query);
            }
            if let Some(ref user_agent) = cfg.user_agent {
                req = req.header(reqwest::header::USER_AGENT, user_agent.clone());
            }
            if let Some(apikey) = cfg.api_keys.get(hotdata::client::WORKSPACE_ID_HEADER) {
                let value = match apikey.prefix {
                    Some(ref prefix) => format!("{} {}", prefix, apikey.key),
                    None => apikey.key.clone(),
                };
                req = req.header(hotdata::client::WORKSPACE_ID_HEADER, value);
            }
            if let Some(token) = cfg.resolve_bearer_token().await {
                req = req.bearer_auth(token);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Transport(format!("error connecting to API: {e}")))?;
            let status = resp.status();
            let body = resp
                .text()
                .await
                .map_err(|e| ApiError::Transport(format!("error connecting to API: {e}")))?;
            if !status.is_success() {
                return Err(ApiError::Status { status, body });
            }
            serde_json::from_str(&body)
                .map_err(|e| ApiError::Transport(format!("error parsing response: {e}")))
        })
    }

    /// Issue an authenticated `POST {base}/v1{path}` with a JSON body through
    /// the SDK `Configuration`, returning the raw status + body text.
    ///
    /// The seam's POST counterpart to [`get_json`](Self::get_json): used where
    /// the generated SDK response model is an untagged enum whose variant
    /// selection could differ from the CLI's hand-rolled field-probing
    /// (`/refresh`), so the CLI keeps parsing the raw JSON itself while the seam
    /// still owns auth/transport (same reqwest client, bearer, `X-Workspace-Id`).
    pub fn post_raw(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<(reqwest::StatusCode, String), ApiError> {
        let cfg = self.client.configuration();
        let url = format!("{}/v1{path}", cfg.base_path);
        rt().block_on(async move {
            let mut req = cfg.client.request(reqwest::Method::POST, &url).json(body);
            if let Some(ref user_agent) = cfg.user_agent {
                req = req.header(reqwest::header::USER_AGENT, user_agent.clone());
            }
            if let Some(apikey) = cfg.api_keys.get(hotdata::client::WORKSPACE_ID_HEADER) {
                let value = match apikey.prefix {
                    Some(ref prefix) => format!("{} {}", prefix, apikey.key),
                    None => apikey.key.clone(),
                };
                req = req.header(hotdata::client::WORKSPACE_ID_HEADER, value);
            }
            if let Some(token) = cfg.resolve_bearer_token().await {
                req = req.bearer_auth(token);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Transport(format!("error connecting to API: {e}")))?;
            let status = resp.status();
            let body = resp
                .text()
                .await
                .map_err(|e| ApiError::Transport(format!("error connecting to API: {e}")))?;
            Ok((status, body))
        })
    }

    /// Issue an authenticated `DELETE {base}/v1{path}` through the SDK
    /// `Configuration`, returning the raw status + body text.
    ///
    /// The seam's DELETE counterpart to [`post_raw`](Self::post_raw): used by
    /// `databases.rs`, where the delete bodies feed the same CLI-side
    /// `(status, body)` control flow as the old raw `delete_raw` (e.g. the
    /// delete+recreate path inspects the failure body), so non-success is
    /// returned as `Ok((status, body))` rather than an error.
    pub fn delete_raw(
        &self,
        path: &str,
    ) -> Result<(reqwest::StatusCode, String), ApiError> {
        let cfg = self.client.configuration();
        let url = format!("{}/v1{path}", cfg.base_path);
        rt().block_on(async move {
            let mut req = cfg.client.request(reqwest::Method::DELETE, &url);
            if let Some(ref user_agent) = cfg.user_agent {
                req = req.header(reqwest::header::USER_AGENT, user_agent.clone());
            }
            if let Some(apikey) = cfg.api_keys.get(hotdata::client::WORKSPACE_ID_HEADER) {
                let value = match apikey.prefix {
                    Some(ref prefix) => format!("{} {}", prefix, apikey.key),
                    None => apikey.key.clone(),
                };
                req = req.header(hotdata::client::WORKSPACE_ID_HEADER, value);
            }
            if let Some(token) = cfg.resolve_bearer_token().await {
                req = req.bearer_auth(token);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Transport(format!("error connecting to API: {e}")))?;
            let status = resp.status();
            let body = resp
                .text()
                .await
                .map_err(|e| ApiError::Transport(format!("error connecting to API: {e}")))?;
            Ok((status, body))
        })
    }

    /// Issue an authenticated `GET {base}/v1{path}` with a custom `Accept`
    /// header through the SDK `Configuration`, returning the raw status + body
    /// bytes.
    ///
    /// The seam's binary-body counterpart to [`get_json`](Self::get_json): used
    /// for the Arrow IPC result fetch (`/results/{id}`), where the CLI decodes
    /// the stream itself with its own pinned `arrow` crate version rather than
    /// the SDK's `get_result_arrow` (which returns a `RecordBatch` from a
    /// different `arrow` major). The seam still owns auth/transport (same
    /// reqwest client, bearer via the `token_provider`, `X-Workspace-Id`).
    pub fn get_bytes(
        &self,
        path: &str,
        accept: &str,
    ) -> Result<(reqwest::StatusCode, Vec<u8>), ApiError> {
        let cfg = self.client.configuration();
        let url = format!("{}/v1{path}", cfg.base_path);
        let accept = accept.to_string();
        rt().block_on(async move {
            let mut req = cfg
                .client
                .request(reqwest::Method::GET, &url)
                .header(reqwest::header::ACCEPT, accept);
            if let Some(ref user_agent) = cfg.user_agent {
                req = req.header(reqwest::header::USER_AGENT, user_agent.clone());
            }
            if let Some(apikey) = cfg.api_keys.get(hotdata::client::WORKSPACE_ID_HEADER) {
                let value = match apikey.prefix {
                    Some(ref prefix) => format!("{} {}", prefix, apikey.key),
                    None => apikey.key.clone(),
                };
                req = req.header(hotdata::client::WORKSPACE_ID_HEADER, value);
            }
            if let Some(token) = cfg.resolve_bearer_token().await {
                req = req.bearer_auth(token);
            }

            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::Transport(format!("error connecting to API: {e}")))?;
            let status = resp.status();
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| ApiError::Transport(format!("error connecting to API: {e}")))?;
            Ok((status, bytes.to_vec()))
        })
    }

    // --- Sample migrated call (workspace.rs uses this) -----------------------

    /// List workspaces visible to the authenticated principal.
    pub fn list_workspaces(
        &self,
        organization_public_id: Option<&str>,
    ) -> Result<hotdata::models::ListWorkspacesResponse, ApiError> {
        block(self.client.workspaces().list(organization_public_id))
    }
}

/// Decide what error text to print for a failed response. Pure function so the
/// 4xx-to-re-auth-hint heuristic is unit-testable without HTTP or `exit`.
///
/// Relocated verbatim from the old `api.rs`.
pub fn format_fail_message(
    status: reqwest::StatusCode,
    body: &str,
    auth_status: Option<&auth::AuthStatus>,
) -> String {
    if status.is_client_error()
        && let Some(auth::AuthStatus::Invalid(_)) = auth_status
    {
        return "error: API key is invalid. Run 'hotdata auth login' (or 'hotdata auth') to re-authenticate.".to_string();
    }
    util::api_error(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use auth::AuthStatus;

    // --- format_fail_message: ported verbatim from api.rs (9 cases) ----------

    #[test]
    fn format_fail_message_401_with_invalid_key_shows_reauth_hint() {
        let msg = format_fail_message(
            reqwest::StatusCode::UNAUTHORIZED,
            "",
            Some(&AuthStatus::Invalid(401)),
        );
        assert!(msg.contains("API key is invalid"));
        assert!(msg.contains("hotdata auth login") || msg.contains("hotdata auth"));
    }

    #[test]
    fn format_fail_message_404_with_invalid_key_shows_reauth_hint() {
        let msg = format_fail_message(
            reqwest::StatusCode::NOT_FOUND,
            "",
            Some(&AuthStatus::Invalid(401)),
        );
        assert!(msg.contains("API key is invalid"), "got: {msg}");
    }

    #[test]
    fn format_fail_message_404_with_valid_key_shows_real_error() {
        let body = r#"{"error":{"message":"Query run 'qrun_notreal' not found"}}"#;
        let msg = format_fail_message(
            reqwest::StatusCode::NOT_FOUND,
            body,
            Some(&AuthStatus::Authenticated),
        );
        assert!(!msg.contains("API key is invalid"));
        assert!(msg.contains("Query run 'qrun_notreal' not found"));
    }

    #[test]
    fn format_fail_message_400_with_valid_key_shows_real_error() {
        let body = r#"{"error":{"message":"invalid_sql"}}"#;
        let msg = format_fail_message(
            reqwest::StatusCode::BAD_REQUEST,
            body,
            Some(&AuthStatus::Authenticated),
        );
        assert_eq!(msg, "invalid_sql");
    }

    #[test]
    fn format_fail_message_5xx_never_shows_reauth_hint() {
        let msg = format_fail_message(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            "server exploded",
            None,
        );
        assert!(!msg.contains("API key is invalid"));
        assert_eq!(msg, "server exploded");
    }

    #[test]
    fn format_fail_message_4xx_connection_error_on_probe_falls_through() {
        let body = r#"{"error":{"message":"forbidden"}}"#;
        let msg = format_fail_message(
            reqwest::StatusCode::FORBIDDEN,
            body,
            Some(&AuthStatus::ConnectionError("tcp reset".to_string())),
        );
        assert!(!msg.contains("API key is invalid"));
        assert_eq!(msg, "forbidden");
    }

    #[test]
    fn format_fail_message_4xx_no_probe_result_falls_through() {
        let body = "plain body";
        let msg = format_fail_message(reqwest::StatusCode::NOT_FOUND, body, None);
        assert!(!msg.contains("API key is invalid"));
        assert_eq!(msg, "plain body");
    }

    #[test]
    fn format_fail_message_4xx_authenticated_probe_shows_server_message() {
        let body = r#"{"error":{"message":"workspace_not_found"}}"#;
        let msg = format_fail_message(
            reqwest::StatusCode::NOT_FOUND,
            body,
            Some(&AuthStatus::Authenticated),
        );
        assert_eq!(msg, "workspace_not_found");
    }

    #[test]
    fn format_fail_message_403_with_valid_key_shows_real_error() {
        let body = r#"{"error":"connection_not_found"}"#;
        let msg = format_fail_message(
            reqwest::StatusCode::FORBIDDEN,
            body,
            Some(&AuthStatus::Authenticated),
        );
        assert!(!msg.contains("API key is invalid"));
    }

    // --- error mapping -------------------------------------------------------

    #[test]
    fn from_sdk_maps_response_error_to_status() {
        let err: Error<()> = Error::ResponseError(ResponseContent {
            status: reqwest::StatusCode::NOT_FOUND,
            content: "missing".to_string(),
            entity: None,
        });
        match ApiError::from_sdk(err) {
            ApiError::Status { status, body } => {
                assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
                assert_eq!(body, "missing");
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn none_if_404_swallows_404_only() {
        let v404: Result<i32, ApiError> = Err(ApiError::Status {
            status: reqwest::StatusCode::NOT_FOUND,
            body: String::new(),
        });
        assert_eq!(none_if_404(v404).unwrap(), None);

        let ok: Result<i32, ApiError> = Ok(7);
        assert_eq!(none_if_404(ok).unwrap(), Some(7));

        let v500: Result<i32, ApiError> = Err(ApiError::Status {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: "boom".into(),
        });
        assert!(none_if_404(v500).is_err());
    }

    // --- runtime sanity ------------------------------------------------------

    #[test]
    fn shared_runtime_runs_a_future() {
        let n = rt().block_on(async { 21 + 21 });
        assert_eq!(n, 42);
    }

    // --- wrapper: workspace-id header + concurrent block_on ------------------

    const WS_BODY: &str = r#"{"ok":true,"workspaces":[]}"#;

    #[test]
    fn list_workspaces_succeeds_with_bearer() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/workspaces")
            .match_header("Authorization", "Bearer test-jwt")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(WS_BODY)
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", None);
        let resp = api.list_workspaces(None).expect("list should succeed");
        assert!(resp.ok);
        m.assert();
    }

    #[test]
    fn workspace_id_header_is_installed_on_scoped_calls() {
        // Regression for the old api.rs:598 header assertion. `datasets().list`
        // carries the X-Workspace-Id api_key; assert it reaches the wire.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/datasets")
            .match_header("Authorization", "Bearer test-jwt")
            .match_header("X-Workspace-Id", "ws-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"count":0,"datasets":[],"has_more":false,"limit":50,"offset":0}"#)
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let resp = block(api.client.datasets().list(None, None)).expect("list datasets");
        assert_eq!(resp.count, 0);
        m.assert();
    }

    #[test]
    fn error_response_maps_to_status() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/v1/workspaces")
            .with_status(500)
            .with_body("boom")
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", None);
        match api.list_workspaces(None).unwrap_err() {
            ApiError::Status { status, body } => {
                assert_eq!(status, reqwest::StatusCode::INTERNAL_SERVER_ERROR);
                assert!(body.contains("boom"));
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn concurrent_block_on_from_rayon_workers() {
        use rayon::prelude::*;

        // Mirror indexes.rs: clone the Send+Sync+Clone Api into a rayon
        // par_iter and call a wrapper method from many worker threads. The
        // shared multi-thread runtime must tolerate concurrent block_on with
        // no panic ("cannot start a runtime within a runtime") or deadlock.
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/v1/workspaces")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(WS_BODY)
            .expect_at_least(8)
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", None);
        let results: Vec<bool> = (0..8)
            .into_par_iter()
            .map(|_| {
                let api = api.clone();
                api.list_workspaces(None).map(|r| r.ok).unwrap_or(false)
            })
            .collect();

        assert_eq!(results.len(), 8);
        assert!(results.iter().all(|ok| *ok), "every worker must succeed");
    }

    #[test]
    fn get_json_sends_bearer_workspace_and_query_then_deserializes() {
        // The seam's untyped escape hatch (used by results.rs): carry the
        // bearer + X-Workspace-Id like every SDK call, append query pairs, hit
        // /v1<path>, and deserialize into a caller-owned type that keeps fields
        // the generated SDK model would drop.
        #[derive(serde::Deserialize)]
        struct Probe {
            value: u32,
        }

        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/results")
            .match_header("Authorization", "Bearer test-jwt")
            .match_header("X-Workspace-Id", "ws-1")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("limit".into(), "10".into()),
                mockito::Matcher::UrlEncoded("offset".into(), "5".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"value":42}"#)
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let probe: Probe = api
            .get_json(
                "/results",
                &[("limit", "10".to_string()), ("offset", "5".to_string())],
            )
            .expect("get_json should succeed");
        assert_eq!(probe.value, 42);
        m.assert();
    }

    #[test]
    fn get_json_maps_error_status() {
        #[derive(serde::Deserialize, Debug)]
        struct Probe {
            #[allow(dead_code)]
            value: u32,
        }
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/v1/results")
            .with_status(404)
            .with_body("missing")
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", None);
        match api.get_json::<Probe>("/results", &[]).unwrap_err() {
            ApiError::Status { status, body } => {
                assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
                assert!(body.contains("missing"));
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    #[test]
    fn post_raw_sends_bearer_workspace_and_body_then_returns_status() {
        // The seam's POST escape hatch (used by connections.rs refresh/create):
        // carry the bearer + X-Workspace-Id like every SDK call, send the JSON
        // body, and return the raw status + body for caller-side parsing.
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/refresh")
            .match_header("Authorization", "Bearer test-jwt")
            .match_header("X-Workspace-Id", "ws-1")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"connection_id":"conn_1","data":true}"#.into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"rows_synced":7}"#)
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let body = serde_json::json!({"connection_id": "conn_1", "data": true});
        let (status, text) = api.post_raw("/refresh", &body).expect("post_raw should succeed");
        assert_eq!(status, reqwest::StatusCode::OK);
        assert!(text.contains("rows_synced"));
        m.assert();
    }

    #[test]
    fn post_raw_returns_error_status_without_mapping_to_err() {
        // Non-success is returned as Ok((status, body)) so the caller reproduces
        // the old `(status, body)` raw-post control flow verbatim.
        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/v1/refresh")
            .with_status(400)
            .with_body("bad request")
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", None);
        let (status, text) = api
            .post_raw("/refresh", &serde_json::json!({}))
            .expect("post_raw returns Ok even on non-2xx");
        assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(text, "bad request");
    }

    #[test]
    fn get_bytes_sends_bearer_workspace_accept_then_returns_body() {
        // The seam's binary-body escape hatch (used by query.rs for the Arrow
        // result fetch): carry the bearer + X-Workspace-Id + the custom Accept
        // like every SDK call, and return the raw status + bytes for CLI-side
        // Arrow decoding.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/results/res_1")
            .match_header("Authorization", "Bearer test-jwt")
            .match_header("X-Workspace-Id", "ws-1")
            .match_header("Accept", "application/vnd.apache.arrow.stream")
            .with_status(200)
            .with_header("content-type", "application/vnd.apache.arrow.stream")
            .with_body(&[0u8, 1, 2, 3][..])
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let (status, bytes) = api
            .get_bytes("/results/res_1", "application/vnd.apache.arrow.stream")
            .expect("get_bytes should succeed");
        assert_eq!(status, reqwest::StatusCode::OK);
        assert_eq!(bytes, vec![0u8, 1, 2, 3]);
        m.assert();
    }

    #[test]
    fn get_bytes_returns_non_success_status_with_body() {
        // A failed Arrow fetch surfaces the status + body so the caller can
        // print the server error (reproducing the old get_bytes control flow,
        // which returned (status, bytes) rather than erroring on non-2xx).
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/v1/results/missing")
            .with_status(404)
            .with_body("not found")
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", None);
        let (status, bytes) = api
            .get_bytes("/results/missing", "application/vnd.apache.arrow.stream")
            .expect("get_bytes returns Ok even on non-2xx");
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
        assert_eq!(String::from_utf8_lossy(&bytes), "not found");
    }

    #[test]
    fn clones_share_one_client_arc() {
        let api = Api::test_new("http://127.0.0.1:1", "jwt", None);
        let clone = api.clone();
        // Cheap clone: both share the same underlying Arc<Client>.
        assert!(Arc::ptr_eq(&api.client, &clone.client));
    }
}
