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

use std::sync::Arc;
use std::sync::OnceLock;

use hotdata::Client;
use hotdata::apis::configuration::{ApiKey, Configuration};
use hotdata::apis::{Error, ResponseContent};

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
    /// API base URL as configured (carries the `/v1` suffix; used by the raw
    /// session-token mints, which target `/v1/auth/*` directly).
    pub api_url: String,
    workspace_id: Option<String>,
    database_id: Option<String>,
}

/// Request timeout for SDK-routed calls. Mirrors the old `ApiClient` so a hung
/// server cannot stall the CLI indefinitely. The streaming `/files` upload
/// keeps its own no-timeout client on the raw-HTTP path.
const HTTP_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// TCP keepalive probe interval, matching the old client.
const TCP_KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Build the `reqwest::Client` backing every SDK call, with a request timeout +
/// TCP keepalive. The CLI shares the SDK's reqwest 0.13, so this is the exact
/// type `Configuration.client` expects.
fn sdk_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_REQUEST_TIMEOUT)
        .tcp_keepalive(TCP_KEEPALIVE_INTERVAL)
        .build()
        .expect("reqwest client with timeout should build")
}

/// The `reqwest::Client` backing the streaming `/files` upload. Deliberately has
/// **no** request timeout: an upload's duration scales with file size and uplink
/// (a 10 GB parquet far outlives [`HTTP_REQUEST_TIMEOUT`], which is sized for
/// slow server-side work), so a wall-clock cap would abort a healthy-but-slow
/// transfer. TCP keepalive is kept so a genuinely dead peer is still reaped by
/// the OS; a live-but-slow upload runs to completion and the user can Ctrl-C.
fn upload_reqwest_client() -> reqwest::Client {
    reqwest::Client::builder()
        .tcp_keepalive(TCP_KEEPALIVE_INTERVAL)
        .build()
        .expect("reqwest client should build without a timeout")
}

/// Size of each chunk pulled from the blocking reader (1 MiB). Large enough to
/// keep per-chunk overhead negligible on a multi-GB upload, small enough that an
/// in-flight chunk is a trivial allocation.
const UPLOAD_CHUNK_SIZE: usize = 1 << 20;
/// Bound on chunks buffered between the blocking reader and the async sender.
/// Caps in-flight memory so a fast local disk can't outrun a slow uplink; the
/// read task blocks on a full channel (back-pressure).
const UPLOAD_CHANNEL_DEPTH: usize = 4;

/// Bridge a blocking [`Read`](std::io::Read) source into the async
/// `Stream<Item = Result<Bytes, _>>` the SDK's `upload_stream` consumes.
///
/// A `spawn_blocking` task reads fixed-size chunks and forwards them through a
/// bounded tokio mpsc channel; the returned [`ReceiverStream`] yields them to
/// the request body. The blocking task lives on the runtime's blocking pool, so
/// it does not stall an async worker, and a full channel back-pressures the
/// reader (which keeps the caller's progress bar — wrapped around `reader` —
/// honest). If the receiver is dropped (request aborted/failed) the send errors
/// and the task exits; a read error is forwarded as the stream's terminal item.
fn reader_into_stream(
    mut reader: impl std::io::Read + Send + 'static,
) -> impl futures_core::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send + 'static {
    let (tx, rx) = tokio::sync::mpsc::channel(UPLOAD_CHANNEL_DEPTH);
    rt().spawn_blocking(move || {
        let mut buf = vec![0u8; UPLOAD_CHUNK_SIZE];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = bytes::Bytes::copy_from_slice(&buf[..n]);
                    if tx.blocking_send(Ok(chunk)).is_err() {
                        break; // receiver gone — request aborted
                    }
                }
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    break;
                }
            }
        }
    });
    tokio_stream::wrappers::ReceiverStream::new(rx)
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
    Status {
        status: reqwest::StatusCode,
        body: String,
    },
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
            Error::ResponseError(ResponseContent {
                status, content, ..
            }) => ApiError::Status {
                status,
                body: content,
            },
            Error::Reqwest(e) => ApiError::Transport(format!("error connecting to API: {e}")),
            Error::Serde(e) => ApiError::Transport(format!("error parsing response: {e}")),
            Error::Io(e) => ApiError::Transport(format!("error connecting to API: {e}")),
        }
    }

    /// Map the SDK's [`hotdata::ArrowError`] (the Arrow result-fetch error type,
    /// which is *not* an `Error<T>`) into an [`ApiError`].
    ///
    /// Status-bearing variants are preserved as [`ApiError::Status`] so the 4xx
    /// re-auth hint in [`exit`](Self::exit) still fires; the statusless variants
    /// (decode/transport/not-ready/failed) collapse to [`ApiError::Transport`]
    /// carrying the SDK's own descriptive message. `ArrowError` is
    /// `#[non_exhaustive]`, hence the wildcard arm.
    pub fn from_arrow(err: hotdata::ArrowError) -> Self {
        use hotdata::ArrowError;
        match err {
            ArrowError::NotFound => ApiError::Status {
                status: reqwest::StatusCode::NOT_FOUND,
                body: "result not found".to_string(),
            },
            ArrowError::InvalidParams { ref message } => ApiError::Status {
                status: reqwest::StatusCode::BAD_REQUEST,
                body: message.clone(),
            },
            ArrowError::Http { status, ref body } => ApiError::Status {
                status,
                body: body.clone(),
            },
            other => ApiError::Transport(other.to_string()),
        }
    }

    /// A printable, single-line description of the failure.
    ///
    /// Used where the error is surfaced inline (e.g. folded into a query
    /// `warning`) rather than printed-and-exited via [`exit`](Self::exit).
    pub fn message(&self) -> String {
        match self {
            ApiError::Status { status, body } => format!("{status}: {body}"),
            ApiError::Transport(msg) => msg.clone(),
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
                    config::load("default")
                        .ok()
                        .map(|pc| auth::check_status(&pc))
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

/// KEDA scale state of a workspace's runtimedb worker, as reported by the
/// always-warm control plane. Used only to upgrade a spinner message on a
/// cold start — it never affects control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    /// At least one ready replica; the request is being served warm.
    Ready,
    /// Scaling up (desired >= 1) but no ready replica yet.
    Waking,
    /// Scaled to zero; the request triggered a cold start.
    Asleep,
    /// Couldn't determine (no control plane, error, non-2xx, unscoped).
    Unknown,
}

impl RuntimeState {
    fn from_state_str(s: &str) -> Self {
        match s {
            "ready" => Self::Ready,
            "waking" => Self::Waking,
            "asleep" => Self::Asleep,
            _ => Self::Unknown,
        }
    }

    /// True when the worker is cold — i.e. the in-flight request is (or will be)
    /// blocked waiting for KEDA to bring a replica up.
    fn is_cold(self) -> bool {
        matches!(self, Self::Waking | Self::Asleep)
    }
}

/// How long a request may run before we suspect a cold start and probe the
/// control plane. Short enough that a real wake-up is flagged promptly, long
/// enough that a warm-but-slow request never triggers the probe.
const WAKE_PROBE_DELAY: std::time::Duration = std::time::Duration::from_millis(1500);
/// Upper bound on the status probe itself, so a slow/stuck probe never delays
/// the real response (the probe runs in a select arm that's dropped the moment
/// the real request completes, but this caps its own footprint regardless).
const WAKE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
/// Spinner message shown once a cold start is confirmed.
const WAKE_MESSAGE: &str = "waking up worker after inactivity (this can take ~20s)…";

/// Like [`block`], but shows a spinner with `msg` and, if the request hasn't
/// returned within [`WAKE_PROBE_DELAY`], probes the control plane for the
/// worker's scale state. On a confirmed cold start the spinner message is
/// upgraded to explain the wait. Warm requests pay nothing: the probe only
/// fires after the delay, and returns the real result the instant it lands.
pub fn block_with_wakeup<F, T, E>(api: &Api, msg: &str, fut: F) -> Result<T, ApiError>
where
    F: std::future::Future<Output = Result<T, Error<E>>>,
    E: std::fmt::Debug,
{
    let pb = util::spinner(msg);
    let hint_pb = pb.clone();
    let result = rt().block_on(async {
        tokio::pin!(fut);
        // After the delay, probe once and (if cold) upgrade the message, then
        // idle forever so the select keeps driving the real request to
        // completion. Dropped — cancelling any in-flight probe — as soon as
        // `fut` wins.
        let hint = async {
            tokio::time::sleep(WAKE_PROBE_DELAY).await;
            let state = tokio::time::timeout(WAKE_PROBE_TIMEOUT, api.probe_runtime_status())
                .await
                .unwrap_or(RuntimeState::Unknown);
            if state.is_cold() {
                hint_pb.set_message(WAKE_MESSAGE);
            }
            std::future::pending::<()>().await;
        };
        tokio::pin!(hint);
        loop {
            tokio::select! {
                r = &mut fut => break r,
                // `hint` ends in `pending()`, so this arm never completes; it
                // exists only to drive the probe alongside the real request.
                _ = &mut hint => {}
            }
        }
    });
    pb.finish_and_clear();
    result.map_err(ApiError::from_sdk)
}

/// Map a result, returning `Ok(None)` on HTTP 404 instead of an error.
///
/// Reproduces `ApiClient::get_none_if_not_found` / the context-404 / indexes-404
/// semantics: a missing resource is normal for these probes.
pub fn none_if_404<T>(r: Result<T, ApiError>) -> Result<Option<T>, ApiError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(ApiError::Status { status, .. }) if status == reqwest::StatusCode::NOT_FOUND => {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Normalize a configured `api_url` into the SDK `base_path`.
///
/// The CLI's `api_url` carries a `/v1` suffix (`DEFAULT_API_URL`), but every
/// generated SDK op appends its own `/v1` to `base_path`, and the seam's raw
/// helpers ([`Api::get_json`] etc.) prepend `/v1` too. Passing the `/v1`-suffixed
/// url through verbatim would produce `/v1/v1/...` on every call. Strip one
/// trailing `/v1` (and any trailing slash) so both paths resolve to a single
/// `/v1`. Session-token mints are unaffected: they use the full `self.api_url`
/// to hit `/v1/auth/*` directly.
fn sdk_base_path(api_url: &str) -> String {
    let trimmed = api_url.trim_end_matches('/');
    trimmed.strip_suffix("/v1").unwrap_or(trimmed).to_string()
}

/// Apply the seam's common request headers to a raw `RequestBuilder`: User-Agent,
/// the `X-Workspace-Id` api_key, the database `X-Database-Id` scope, and the
/// resolved bearer. Generated SDK ops inject the api_key headers themselves; the
/// raw seam helpers ([`Api::get_json`] etc.) bypass the generated client, so
/// they funnel through this one place rather than repeating the block per verb.
async fn apply_seam_headers(
    mut req: reqwest::RequestBuilder,
    cfg: &Configuration,
    database_id: Option<&str>,
) -> reqwest::RequestBuilder {
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
    // Database scope — generated ops don't forward it, so the seam must
    // (e.g. `hotdata query --database`).
    if let Some(db) = database_id {
        req = req.header("X-Database-Id", db);
    }
    if let Some(token) = cfg.resolve_bearer_token().await {
        req = req.bearer_auth(token);
    }
    req
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

        // Auth-source precedence:
        //   1. HOTDATA_DATABASE_TOKEN env (databases run child)
        //   2. ~/.hotdata/session.json + optional api_key fallback
        //
        // We pre-flight (so a dead/unusable credential exits at startup with
        // the right hint), then hand the CliTokenProvider the matching mode to
        // re-resolve on every request.
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
                eprintln!(
                    "Run {} to log in, or pass --api-key.",
                    "hotdata auth".cyan()
                );
                std::process::exit(1);
            }
            AuthMode::Session {
                profile: profile_config.clone(),
                api_key_fallback,
            }
        };

        let database_id = std::env::var("HOTDATA_DATABASE").ok().or_else(|| {
            workspace_id.and_then(|ws| crate::config::load_current_database("default", ws))
        });

        Self::from_configuration(
            &api_url,
            workspace_id.map(String::from),
            database_id,
            CliTokenProvider::new(mode),
        )
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
            base_path: sdk_base_path(api_url),
            client: sdk_http_client(),
            // Attribute CLI traffic as the CLI, not the SDK default
            // (`hotdata-rust/...`). The old ApiClient sent no User-Agent; an
            // explicit CLI agent is the correct attribution.
            user_agent: Some(format!("hotdata-cli/{}", env!("CARGO_PKG_VERSION"))),
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
            base_path: sdk_base_path(api_url),
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

    /// Test-only constructor that also scopes the client to a database, so tests
    /// can assert the `X-Database-Id` header reaches the wire.
    #[cfg(test)]
    pub(crate) fn test_new_scoped(
        api_url: &str,
        bearer: &str,
        workspace_id: Option<&str>,
        database_id: Option<&str>,
    ) -> Self {
        let mut configuration = Configuration {
            base_path: sdk_base_path(api_url),
            bearer_access_token: Some(bearer.to_string()),
            ..Configuration::default()
        };
        if let Some(ws) = workspace_id {
            configuration.api_keys.insert(
                hotdata::client::WORKSPACE_ID_HEADER.to_string(),
                ApiKey {
                    prefix: None,
                    key: ws.to_string(),
                },
            );
        }
        Api {
            client: Arc::new(Client::from_configuration(configuration)),
            api_url: api_url.to_string(),
            workspace_id: workspace_id.map(String::from),
            database_id: database_id.map(String::from),
        }
    }

    pub fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }

    pub fn database_id(&self) -> Option<&str> {
        self.database_id.as_deref()
    }

    /// Best-effort probe of the scoped workspace's runtimedb scale state, via
    /// the control-plane `GET /v1/workspaces/{id}/runtime/status` endpoint.
    ///
    /// Returns [`RuntimeState::Unknown`] on any failure (no workspace scope,
    /// transport error, non-2xx, or unparseable body) so callers degrade to
    /// their latency heuristic rather than surfacing an error.
    ///
    /// Deliberately omits the `X-Workspace-Id` header: that header is what the
    /// gateway matches to route `/v1` traffic to the KEDA interceptor, and
    /// routing this probe there would wake the very worker we're asking about.
    /// Without it the request lands on the always-warm control plane, which
    /// answers from Kubernetes state.
    async fn probe_runtime_status(&self) -> RuntimeState {
        let Some(ws) = self.workspace_id.as_deref() else {
            return RuntimeState::Unknown;
        };
        let cfg = self.client.configuration();
        // `base_path` has no `/v1` suffix (see `sdk_base_path`); add the one
        // the control-plane route expects.
        let url = format!(
            "{}/v1/workspaces/{}/runtime/status",
            cfg.base_path.trim_end_matches('/'),
            ws
        );
        let mut req = cfg.client.get(&url);
        if let Some(ref user_agent) = cfg.user_agent {
            req = req.header(reqwest::header::USER_AGENT, user_agent.clone());
        }
        if let Some(token) = cfg.resolve_bearer_token().await {
            req = req.bearer_auth(token);
        }
        let Ok(resp) = req.send().await else {
            return RuntimeState::Unknown;
        };
        if !resp.status().is_success() {
            return RuntimeState::Unknown;
        }
        match resp.json::<serde_json::Value>().await {
            Ok(body) => body
                .get("state")
                .and_then(|s| s.as_str())
                .map(RuntimeState::from_state_str)
                .unwrap_or(RuntimeState::Unknown),
            Err(_) => RuntimeState::Unknown,
        }
    }

    /// Borrow the underlying SDK client (for command modules calling resource
    /// handles directly through [`block`]).
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Stream a file/URL body to `POST /v1/files` through the SDK's
    /// [`Client::upload_stream`], returning the upload id.
    ///
    /// Drives the async SDK from the CLI's synchronous call site, like every
    /// other seam method, but on a **dedicated no-timeout client**: a 10 GB+
    /// parquet far outlives the shared client's 300s request timeout, so a
    /// wall-clock cap would abort a healthy-but-slow transfer. We clone the
    /// configured `Configuration` (same base_path, token_provider, scope
    /// api_keys, user-agent) and swap only the reqwest client, so the upload
    /// carries the identical auth + headers.
    ///
    /// `reader` is the progress-wrapped blocking source (file or URL response);
    /// it is bridged into the async byte stream the SDK consumes by
    /// [`reader_into_stream`]. `content_length`, when known, is sent as
    /// `Content-Length` so the server can reject an oversized upload up front
    /// (the `--url` path may not know the length, hence `Option`).
    ///
    /// The `Content-Type` is left to the SDK default (`application/octet-stream`):
    /// the managed-table load keys off the parquet file extension, not the
    /// upload's recorded content type.
    pub fn upload_stream(
        &self,
        reader: impl std::io::Read + Send + 'static,
        content_length: Option<u64>,
    ) -> Result<String, ApiError> {
        let mut cfg = self.client.configuration().clone();
        cfg.client = upload_reqwest_client();
        let upload_client = Client::from_configuration(cfg);

        let stream = reader_into_stream(reader);
        let resp = rt()
            .block_on(upload_client.upload_stream(stream, None, content_length))
            .map_err(ApiError::from_sdk)?;
        Ok(resp.id)
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
        let database_id = self.database_id.clone();
        rt().block_on(async move {
            let mut req = cfg.client.request(reqwest::Method::GET, &url);
            if !query.is_empty() {
                req = req.query(query);
            }
            req = apply_seam_headers(req, cfg, database_id.as_deref()).await;

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
        let database_id = self.database_id.clone();
        rt().block_on(async move {
            let mut req = cfg.client.request(reqwest::Method::POST, &url).json(body);
            req = apply_seam_headers(req, cfg, database_id.as_deref()).await;

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
    pub fn delete_raw(&self, path: &str) -> Result<(reqwest::StatusCode, String), ApiError> {
        let cfg = self.client.configuration();
        let url = format!("{}/v1{path}", cfg.base_path);
        let database_id = self.database_id.clone();
        rt().block_on(async move {
            let mut req = cfg.client.request(reqwest::Method::DELETE, &url);
            req = apply_seam_headers(req, cfg, database_id.as_deref()).await;

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

    /// Fetch `/v1/results/{id}` as Arrow IPC and decode it through the SDK's
    /// `get_result_arrow`, returning the fully-buffered [`hotdata::ArrowResult`].
    ///
    /// The SDK owns transport (same reqwest client, bearer via the
    /// `token_provider`, `X-Workspace-Id`) and decode. Its
    /// `ArrowError` (the Arrow-path error type, which is not an `Error<T>`) is
    /// mapped to [`ApiError`] via [`from_arrow`](ApiError::from_arrow) so callers
    /// keep the same `.exit()` handling.
    pub fn get_result_arrow(&self, id: &str) -> Result<hotdata::ArrowResult, ApiError> {
        rt().block_on(self.client.get_result_arrow(id, None, None))
            .map_err(ApiError::from_arrow)
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

    #[test]
    fn api_error_message_formats_status_and_transport() {
        let status = ApiError::Status {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: "boom".to_string(),
        };
        let m = status.message();
        assert!(m.contains("500"), "{m}");
        assert!(m.contains("boom"), "{m}");

        let transport = ApiError::Transport("error connecting to API: refused".to_string());
        assert_eq!(transport.message(), "error connecting to API: refused");
    }

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
        let (status, text) = api
            .post_raw("/refresh", &body)
            .expect("post_raw should succeed");
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
    fn get_result_arrow_fetches_decodes_and_forwards_headers() {
        use arrow::array::{Int64Array, RecordBatch, StringArray};
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::ipc::writer::StreamWriter;

        // Build a small Arrow IPC stream the mock server can hand back.
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec!["a", "b", "c"])),
            ],
        )
        .unwrap();
        let mut ipc: Vec<u8> = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut ipc, &schema).unwrap();
            writer.write(&batch).unwrap();
            writer.finish().unwrap();
        }

        // The SDK's get_result_arrow carries the bearer + X-Workspace-Id like
        // every SDK call and negotiates Arrow via ?format=arrow + Accept.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/results/res_1")
            .match_query(mockito::Matcher::UrlEncoded(
                "format".into(),
                "arrow".into(),
            ))
            .match_header("Authorization", "Bearer test-jwt")
            .match_header("X-Workspace-Id", "ws-1")
            .match_header("Accept", "application/vnd.apache.arrow.stream")
            .with_status(200)
            .with_header("content-type", "application/vnd.apache.arrow.stream")
            .with_body(ipc)
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let result = api
            .get_result_arrow("res_1")
            .expect("get_result_arrow should succeed");
        assert_eq!(result.num_rows(), 3);
        assert_eq!(result.schema.fields().len(), 2);
        assert_eq!(result.schema.field(0).name(), "id");
        assert_eq!(result.schema.field(1).name(), "name");
        m.assert();
    }

    #[test]
    fn get_result_arrow_maps_not_found_to_status() {
        // A 404 surfaces as ApiError::Status so the CLI's 4xx re-auth hint path
        // still fires.
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/v1/results/missing")
            .match_query(mockito::Matcher::Any)
            .with_status(404)
            .with_body("not found")
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", None);
        match api.get_result_arrow("missing").unwrap_err() {
            ApiError::Status { status, .. } => {
                assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
            }
            other => panic!("expected ApiError::Status, got {other:?}"),
        }
    }

    #[test]
    fn get_result_arrow_preserves_generic_http_status() {
        // Statuses outside the SDK's explicitly-mapped set (404/400/409/202)
        // come back as ArrowError::Http; from_arrow must preserve them so a
        // 401/403 still reaches the CLI's 4xx re-auth hint. 403 stands in for
        // that family.
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/v1/results/forbidden")
            .match_query(mockito::Matcher::Any)
            .with_status(403)
            .with_body("forbidden")
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", None);
        match api.get_result_arrow("forbidden").unwrap_err() {
            ApiError::Status { status, body } => {
                assert_eq!(status, reqwest::StatusCode::FORBIDDEN);
                assert_eq!(body, "forbidden");
            }
            other => panic!("expected ApiError::Status, got {other:?}"),
        }
    }

    #[test]
    fn clones_share_one_client_arc() {
        let api = Api::test_new("http://127.0.0.1:1", "jwt", None);
        let clone = api.clone();
        // Cheap clone: both share the same underlying Arc<Client>.
        assert!(Arc::ptr_eq(&api.client, &clone.client));
    }

    // --- base path: no double /v1 -------------------------------------------

    #[test]
    fn sdk_base_path_strips_one_trailing_v1() {
        // The configured api_url carries /v1; base_path must not, so a single
        // /v1 is appended downstream.
        assert_eq!(
            sdk_base_path("https://api.hotdata.dev/v1"),
            "https://api.hotdata.dev"
        );
        assert_eq!(
            sdk_base_path("https://api.hotdata.dev/v1/"),
            "https://api.hotdata.dev"
        );
        // A host without /v1 is left alone.
        assert_eq!(
            sdk_base_path("http://127.0.0.1:1234"),
            "http://127.0.0.1:1234"
        );
        // Only ONE trailing /v1 is stripped.
        assert_eq!(sdk_base_path("https://h/v1/v1"), "https://h/v1");
    }

    #[test]
    fn calls_hit_single_v1_when_api_url_has_v1_suffix() {
        // Regression for the production-breaking double-/v1 bug: DEFAULT_API_URL
        // ends in /v1 and the SDK appends its own /v1, so an Api built from a
        // /v1-suffixed url must still land on a single /v1 (not /v1/v1/...).
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/workspaces")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(WS_BODY)
            .create();

        // api_url WITH the /v1 suffix the real profile uses.
        let api = Api::test_new(&format!("{}/v1", server.url()), "test-jwt", None);
        api.list_workspaces(None)
            .expect("call must resolve to a single /v1");
        m.assert(); // before the fix this 404s at /v1/v1/workspaces
    }

    // --- database scope header ----------------------------------------------

    #[test]
    fn database_scope_sends_x_database_id_on_raw_calls() {
        // Regression: `hotdata query --database X` must scope the request. The
        // old ApiClient sent X-Database-Id on every request; the seam must too,
        // and the raw /query submit path is where the scope is applied.
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/query")
            .match_header("Authorization", "Bearer test-jwt")
            .match_header("X-Workspace-Id", "ws-1")
            .match_header("X-Database-Id", "db-1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true}"#)
            .create();

        let api = Api::test_new_scoped(&server.url(), "test-jwt", Some("ws-1"), Some("db-1"));
        let (status, _body) = api
            .post_raw("/query", &serde_json::json!({"sql": "select 1"}))
            .expect("post_raw should succeed");
        assert_eq!(status, reqwest::StatusCode::OK);
        m.assert();
    }

    // --- streaming /files upload --------------------------------------------

    /// A deterministic ASCII payload of `len` bytes, so a body can be matched
    /// exactly to prove the bridged stream delivered every byte in order.
    fn upload_payload(len: usize) -> Vec<u8> {
        (0..len).map(|i| b'a' + (i % 26) as u8).collect()
    }

    fn upload_response_body(id: &str, size: usize) -> String {
        format!(
            r#"{{"id":"{id}","status":"ready","size_bytes":{size},"content_type":"application/octet-stream","created_at":"2026-06-05T00:00:00Z"}}"#
        )
    }

    /// A sized upload (`content_length = Some`) streams the blocking reader
    /// through the bridge to `POST /v1/files`, sends a matching `Content-Length`
    /// (so the server can fast-fail oversize before reading), and delivers every
    /// byte intact. The 5 MiB payload spans several `UPLOAD_CHUNK_SIZE` chunks
    /// and overruns `UPLOAD_CHANNEL_DEPTH`, exercising the channel back-pressure.
    #[test]
    fn upload_stream_sends_sized_body_with_content_length() {
        let payload = upload_payload(5 * 1024 * 1024);
        let len = payload.len();

        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/files")
            .match_header("Authorization", "Bearer test-jwt")
            .match_header("X-Workspace-Id", "ws-1")
            .match_header("Content-Type", "application/octet-stream")
            .match_header("Content-Length", len.to_string().as_str())
            .match_body(mockito::Matcher::Exact(
                String::from_utf8(payload.clone()).unwrap(),
            ))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(upload_response_body("upload_sized", len))
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let id = api
            .upload_stream(std::io::Cursor::new(payload), Some(len as u64))
            .expect("sized upload should succeed");

        assert_eq!(id, "upload_sized");
        m.assert();
    }

    /// With an unknown length (`content_length = None`, the `--url` source) the
    /// body streams chunked and still arrives intact. Multiple chunks, so the
    /// bridge is genuinely streaming rather than buffering a single read.
    #[test]
    fn upload_stream_streams_chunked_when_length_unknown() {
        let payload = upload_payload(3 * 1024 * 1024);
        let len = payload.len();

        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/files")
            .match_body(mockito::Matcher::Exact(
                String::from_utf8(payload.clone()).unwrap(),
            ))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(upload_response_body("upload_chunked", len))
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let id = api
            .upload_stream(std::io::Cursor::new(payload), None)
            .expect("chunked upload should succeed");

        assert_eq!(id, "upload_chunked");
        m.assert();
    }

    /// A non-success status surfaces as an `ApiError::Status` carrying the body,
    /// the same mapping every other seam call uses (so the CLI prints the server
    /// message and the 4xx re-auth hint still fires).
    #[test]
    fn upload_stream_maps_error_status() {
        let payload = upload_payload(64);
        let len = payload.len();

        let mut server = mockito::Server::new();
        let _m = server
            .mock("POST", "/v1/files")
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"Upload exceeds maximum size"}"#)
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let err = api
            .upload_stream(std::io::Cursor::new(payload), Some(len as u64))
            .expect_err("a 400 should map to an error");

        match err {
            ApiError::Status { status, body } => {
                assert_eq!(status, reqwest::StatusCode::BAD_REQUEST);
                assert!(body.contains("Upload exceeds maximum size"));
            }
            other => panic!("expected Status error, got {other:?}"),
        }
    }

    #[test]
    fn runtime_state_parsing_and_coldness() {
        assert_eq!(RuntimeState::from_state_str("ready"), RuntimeState::Ready);
        assert_eq!(RuntimeState::from_state_str("waking"), RuntimeState::Waking);
        assert_eq!(RuntimeState::from_state_str("asleep"), RuntimeState::Asleep);
        assert_eq!(
            RuntimeState::from_state_str("garbage"),
            RuntimeState::Unknown
        );
        // Only the not-yet-serving states count as cold.
        assert!(RuntimeState::Asleep.is_cold());
        assert!(RuntimeState::Waking.is_cold());
        assert!(!RuntimeState::Ready.is_cold());
        assert!(!RuntimeState::Unknown.is_cold());
    }

    #[test]
    fn probe_runtime_status_reads_state_without_workspace_header() {
        let mut server = mockito::Server::new();
        // The probe must hit the control-plane route *without* X-Workspace-Id,
        // or the gateway would send it to the KEDA interceptor and wake the
        // worker we're only trying to inspect.
        let m = server
            .mock("GET", "/v1/workspaces/work-1/runtime/status")
            .match_header("x-workspace-id", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"state":"asleep","estimated_wake_seconds":20}"#)
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("work-1"));
        let state = rt().block_on(api.probe_runtime_status());
        assert_eq!(state, RuntimeState::Asleep);
        m.assert();
    }

    #[test]
    fn probe_runtime_status_non_2xx_is_unknown() {
        let mut server = mockito::Server::new();
        let _m = server
            .mock("GET", "/v1/workspaces/work-1/runtime/status")
            .with_status(500)
            .with_body("boom")
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("work-1"));
        assert_eq!(
            rt().block_on(api.probe_runtime_status()),
            RuntimeState::Unknown
        );
    }

    #[test]
    fn probe_runtime_status_unscoped_is_unknown_without_request() {
        // No workspace scope -> nothing to probe; must not make a request.
        let mut server = mockito::Server::new();
        let m = server.mock("GET", mockito::Matcher::Any).expect(0).create();

        let api = Api::test_new(&server.url(), "test-jwt", None);
        assert_eq!(
            rt().block_on(api.probe_runtime_status()),
            RuntimeState::Unknown
        );
        m.assert();
    }
}
