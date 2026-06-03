use crate::auth;
use crate::config;
use crate::util;
use crossterm::style::Stylize;
use serde::de::DeserializeOwned;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Mints a fresh bearer token on demand. Returns `None` if no fresh token
/// could be obtained (e.g. the refresh token is dead and there's no API key
/// to re-mint from). Must be `Send + Sync` because `ApiClient` is shared
/// across rayon worker threads (see `indexes.rs`).
pub type TokenRefresher = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Cap on any single HTTP request. Connection create + synchronous schema
/// discovery against a slow remote catalog can take well over a minute, so
/// this needs to be generous; 5 minutes leaves headroom while still bounding
/// the worst case if the server genuinely hangs.
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// TCP keepalive cadence. Without this, macOS will drop a TCP connection
/// that has been quiet (e.g. while the server is doing slow synchronous
/// work) and reqwest surfaces it as "error sending request" even though the
/// request itself completed server-side.
const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

fn build_http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(HTTP_REQUEST_TIMEOUT)
        .tcp_keepalive(TCP_KEEPALIVE_INTERVAL)
        .build()
        .expect("reqwest blocking client should always build with these defaults")
}

/// Client used only for streaming file uploads. Deliberately has **no**
/// request timeout: an upload's duration scales with file size and the
/// user's uplink (a 10 GB parquet on a normal connection takes far longer
/// than the 300s `HTTP_REQUEST_TIMEOUT` that's sized for slow server-side
/// work), so a wall-clock cap would abort healthy-but-slow transfers. TCP
/// keepalive is kept so a genuinely dead peer is still reaped by the OS; a
/// live-but-slow upload runs to completion, and the user can Ctrl-C if it
/// truly stalls.
fn build_upload_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .tcp_keepalive(TCP_KEEPALIVE_INTERVAL)
        .build()
        .expect("reqwest blocking client should always build with these defaults")
}

#[derive(Clone)]
pub struct ApiClient {
    client: reqwest::blocking::Client,
    /// The current bearer token. Wrapped so it can be refreshed in place
    /// through a `&self` borrow (every request method takes `&self`), and
    /// `Arc<Mutex<_>>` rather than `RefCell` so the client stays `Send +
    /// Sync` for the rayon-parallel paths and `#[derive(Clone)]` keeps
    /// clones sharing the same refreshed token.
    token: Arc<Mutex<String>>,
    /// How to obtain a fresh token when the current one is rejected.
    refresh: TokenRefresher,
    pub api_url: String,
    workspace_id: Option<String>,
    sandbox_id: Option<String>,
    database_id: Option<String>,
}

impl ApiClient {
    /// Create a new API client. Loads config, pre-flights a JWT session.
    /// Pass `workspace_id` for endpoints that require it, or `None` for
    /// workspace-less endpoints.
    pub fn new(workspace_id: Option<&str>) -> Self {
        let profile_config = match config::load("default") {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };

        // Auth source precedence:
        //
        // 1. `HOTDATA_DATABASE_TOKEN` env var — a `databases run` child
        //    is executing with the parent's credentials scrubbed and a
        //    database-scoped JWT injected. Refresh in-memory via
        //    `HOTDATA_DATABASE_REFRESH_TOKEN` near expiry; never write
        //    to disk (the child's FS may not be writable).
        // 2. `HOTDATA_SANDBOX_TOKEN` env var — a `sandbox run` child
        //    is executing with the parent's credentials scrubbed.
        //    Refresh in-memory via `HOTDATA_SANDBOX_REFRESH_TOKEN` if
        //    the JWT is close to expiry; never write to disk (the
        //    child's FS may not be writable).
        // 3. `~/.hotdata/sandbox_session.json` — the user ran
        //    `hotdata sandbox set <id>` (or `sandbox new` / `sandbox
        //    run` in the parent shell). The sandbox JWT is the active
        //    bearer for *every* command until `sandbox set` (with no
        //    id) clears the file.
        // 4. `~/.hotdata/session.json` + optional api_key fallback —
        //    normal user-scoped CLI session.
        let api_url = profile_config.api_url.to_string();
        let access_token = if std::env::var("HOTDATA_DATABASE_TOKEN").is_ok() {
            match crate::database_session::refresh_from_env(&api_url) {
                Some(t) => t,
                None => {
                    eprintln!("{}", "error: HOTDATA_DATABASE_TOKEN is empty".red());
                    std::process::exit(1);
                }
            }
        } else if std::env::var("HOTDATA_SANDBOX_TOKEN").is_ok() {
            match crate::sandbox_session::refresh_from_env(&api_url) {
                Some(t) => t,
                None => {
                    eprintln!("{}", "error: HOTDATA_SANDBOX_TOKEN is empty".red());
                    std::process::exit(1);
                }
            }
        } else if crate::sandbox_session::load().is_some() {
            match crate::sandbox_session::ensure_access_token(&api_url) {
                Some(t) => t,
                None => {
                    eprintln!("{}", "error: sandbox session expired".red());
                    eprintln!(
                        "Run {} to clear it, or {} to re-mint.",
                        "hotdata sandbox set".cyan(),
                        "hotdata sandbox set <id>".cyan(),
                    );
                    std::process::exit(1);
                }
            }
        } else {
            let api_key_fallback = profile_config
                .api_key
                .as_deref()
                .filter(|k| !k.is_empty() && *k != "PLACEHOLDER");

            // Pre-flight: return the cached JWT if valid, refresh it if
            // close to expiry, or mint a new one from the API key. The
            // returned string is a JWT — that's what we send on the wire.
            match crate::jwt::ensure_access_token(&profile_config, api_key_fallback) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{}", format!("error: {e}").red());
                    eprintln!(
                        "Run {} to log in, or pass --api-key.",
                        "hotdata auth".cyan()
                    );
                    std::process::exit(1);
                }
            }
        };

        // Refresher used when a request comes back 401: reload config (to
        // pick up a session that `ensure_access_token` may have just
        // persisted) and re-run the same auth-source precedence, best-effort.
        let refresh: TokenRefresher = Arc::new(|| {
            let pc = config::load("default").ok()?;
            resolve_fresh_token(&pc)
        });

        Self {
            client: build_http_client(),
            token: Arc::new(Mutex::new(access_token)),
            refresh,
            api_url: profile_config.api_url.to_string(),
            workspace_id: workspace_id.map(String::from),
            sandbox_id: std::env::var("HOTDATA_SANDBOX").ok().or_else(|| {
                if crate::sandbox::find_sandbox_run_ancestor().is_some() {
                    eprintln!("error: sandbox has been lost -- restart the process");
                    std::process::exit(1);
                }
                profile_config.sandbox
            }),
            database_id: std::env::var("HOTDATA_DATABASE").ok().or_else(|| {
                workspace_id.and_then(|ws| crate::config::load_current_database("default", ws))
            }),
        }
    }

    /// Override the database ID for a single query without touching config.
    pub fn with_database(mut self, database_id: &str) -> Self {
        self.database_id = Some(database_id.to_string());
        self
    }

    /// Test-only client (no config load). Used with a local mock HTTP server.
    /// The refresher returns `None`, so 401s are not retried — matching the
    /// behavior of tests that don't exercise the refresh path.
    #[cfg(test)]
    pub(crate) fn test_new(api_url: &str, api_key: &str, workspace_id: Option<&str>) -> Self {
        Self::test_new_with_refresh(api_url, api_key, workspace_id, Arc::new(|| None))
    }

    /// Test-only client with an injectable token refresher, for exercising the
    /// 401-retry path without touching real config or the JWT machinery.
    #[cfg(test)]
    pub(crate) fn test_new_with_refresh(
        api_url: &str,
        api_key: &str,
        workspace_id: Option<&str>,
        refresh: TokenRefresher,
    ) -> Self {
        Self {
            client: build_http_client(),
            token: Arc::new(Mutex::new(api_key.to_string())),
            refresh,
            api_url: api_url.to_string(),
            workspace_id: workspace_id.map(String::from),
            sandbox_id: None,
            database_id: None,
        }
    }

    /// Prints an error for a non-2xx response and exits. On 4xx, first re-probes
    /// the API key: if it's actually invalid, a clear re-auth hint is shown
    /// instead of whatever cryptic body the primary endpoint returned.
    fn fail_response(&self, status: reqwest::StatusCode, body: String) -> ! {
        let auth_status = if status.is_client_error() {
            config::load("default")
                .ok()
                .map(|pc| auth::check_status(&pc))
        } else {
            None
        };
        eprintln!(
            "{}",
            format_fail_message(status, &body, auth_status.as_ref()).red()
        );
        std::process::exit(1);
    }

    fn build_request(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> reqwest::blocking::RequestBuilder {
        let bearer = self.token.lock().expect("token mutex poisoned").clone();
        let mut req = self
            .client
            .request(method, url)
            .header("Authorization", format!("Bearer {bearer}"));
        if let Some(ref ws) = self.workspace_id {
            req = req.header("X-Workspace-Id", ws);
        }
        if let Some(ref sid) = self.sandbox_id {
            // Send both headers during the session→sandbox migration window.
            req = req.header("X-Session-Id", sid);
            req = req.header("X-Sandbox-Id", sid);
        }
        if let Some(ref db_id) = self.database_id {
            req = req.header("X-Database-Id", db_id);
        }
        req
    }

    /// Send via `util::send_debug` and unwrap connection errors with the
    /// CLI's standard "error connecting" exit. All public HTTP methods
    /// route through here so debug logging is uniform.
    fn send(
        &self,
        builder: reqwest::blocking::RequestBuilder,
        body_for_log: Option<&serde_json::Value>,
    ) -> (reqwest::StatusCode, String) {
        match util::send_debug(&self.client, builder, body_for_log) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        }
    }

    /// Mint a fresh bearer and swap it in. Returns whether a new token was
    /// obtained — `false` means the refresher gave up, so the caller should
    /// surface the original failure rather than pointlessly retrying.
    fn refresh_token(&self) -> bool {
        match (self.refresh)() {
            Some(new) => {
                *self.token.lock().expect("token mutex poisoned") = new;
                true
            }
            None => false,
        }
    }

    /// Send a request and, if the server rejects the bearer with 401, mint a
    /// fresh token and retry exactly once. `build` reconstructs the request
    /// from scratch on each attempt so the retry picks up the refreshed bearer
    /// (the Authorization header is baked into an already-built request and
    /// can't be mutated). Streaming uploads can't use this — their body is
    /// consumed on the first send and is not replayable.
    fn send_with_retry(
        &self,
        build: impl Fn() -> reqwest::blocking::RequestBuilder,
        body_for_log: Option<&serde_json::Value>,
    ) -> (reqwest::StatusCode, String) {
        let (status, body) = self.send(build(), body_for_log);
        if status == reqwest::StatusCode::UNAUTHORIZED && self.refresh_token() {
            return self.send(build(), body_for_log);
        }
        (status, body)
    }

    fn parse_json<T: DeserializeOwned>(body: &str) -> T {
        match serde_json::from_str(body) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error parsing response: {e}");
                std::process::exit(1);
            }
        }
    }

    /// GET request with query parameters, returns parsed response.
    /// Parameters with `None` values are omitted.
    pub fn get_with_params<T: DeserializeOwned>(
        &self,
        path: &str,
        params: &[(&str, Option<String>)],
    ) -> T {
        let filtered: Vec<(&str, &String)> = params
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|val| (*k, val)))
            .collect();
        let url = format!("{}{path}", self.api_url);
        let (status, body) = self.send_with_retry(
            || {
                self.build_request(reqwest::Method::GET, &url)
                    .query(&filtered)
            },
            None,
        );
        if !status.is_success() {
            self.fail_response(status, body);
        }
        Self::parse_json(&body)
    }

    /// GET request, returns parsed response.
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> T {
        let url = format!("{}{path}", self.api_url);
        let (status, body) =
            self.send_with_retry(|| self.build_request(reqwest::Method::GET, &url), None);
        if !status.is_success() {
            self.fail_response(status, body);
        }
        Self::parse_json(&body)
    }

    /// GET request; returns `None` on HTTP 404. Other status codes use the same handling as
    /// [`Self::get`]. Used when probing many paths where a missing resource is normal.
    pub fn get_none_if_not_found<T: DeserializeOwned>(&self, path: &str) -> Option<T> {
        let url = format!("{}{path}", self.api_url);
        let (status, body) =
            self.send_with_retry(|| self.build_request(reqwest::Method::GET, &url), None);
        if status == reqwest::StatusCode::NOT_FOUND {
            return None;
        }
        if !status.is_success() {
            self.fail_response(status, body);
        }
        Some(Self::parse_json(&body))
    }

    /// POST request with JSON body, returns parsed response.
    pub fn post<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> T {
        let url = format!("{}{path}", self.api_url);
        let (status, resp_body) = self.send_with_retry(
            || self.build_request(reqwest::Method::POST, &url).json(body),
            Some(body),
        );
        if !status.is_success() {
            self.fail_response(status, resp_body);
        }
        Self::parse_json(&resp_body)
    }

    /// GET request, exits only on connection error, returns raw (status, body).
    /// Use for best-effort endpoints (e.g. health checks) where the caller wants
    /// to handle non-2xx responses gracefully instead of aborting.
    pub fn get_raw(&self, path: &str) -> (reqwest::StatusCode, String) {
        let url = format!("{}{path}", self.api_url);
        self.send_with_retry(|| self.build_request(reqwest::Method::GET, &url), None)
    }

    /// GET with a custom Accept header; returns raw bytes instead of decoded text.
    /// Used for binary result formats such as Arrow IPC streams.
    pub fn get_bytes(&self, path: &str, accept: &str) -> (reqwest::StatusCode, Vec<u8>) {
        let url = format!("{}{path}", self.api_url);
        let send = |client: &reqwest::blocking::Client, c: &Self| {
            let req = c
                .build_request(reqwest::Method::GET, &url)
                .header("Accept", accept);
            match util::send_debug_bytes(client, req) {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("error connecting to API: {e}");
                    std::process::exit(1);
                }
            }
        };
        let (status, bytes) = send(&self.client, self);
        if status == reqwest::StatusCode::UNAUTHORIZED && self.refresh_token() {
            return send(&self.client, self);
        }
        (status, bytes)
    }

    /// POST request with JSON body, exits on error, returns raw (status, body).
    pub fn post_raw(&self, path: &str, body: &serde_json::Value) -> (reqwest::StatusCode, String) {
        let url = format!("{}{path}", self.api_url);
        self.send_with_retry(
            || self.build_request(reqwest::Method::POST, &url).json(body),
            Some(body),
        )
    }

    /// DELETE request, exits on connection error, returns raw (status, body).
    pub fn delete_raw(&self, path: &str) -> (reqwest::StatusCode, String) {
        let url = format!("{}{path}", self.api_url);
        self.send_with_retry(|| self.build_request(reqwest::Method::DELETE, &url), None)
    }

    /// PATCH request with JSON body, returns parsed response.
    pub fn patch<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> T {
        let url = format!("{}{path}", self.api_url);
        let (status, resp_body) = self.send_with_retry(
            || self.build_request(reqwest::Method::PATCH, &url).json(body),
            Some(body),
        );
        if !status.is_success() {
            self.fail_response(status, resp_body);
        }
        Self::parse_json(&resp_body)
    }

    /// PUT request with JSON body, returns parsed response.
    pub fn put<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> T {
        let url = format!("{}{path}", self.api_url);
        let (status, resp_body) = self.send_with_retry(
            || self.build_request(reqwest::Method::PUT, &url).json(body),
            Some(body),
        );
        if !status.is_success() {
            self.fail_response(status, resp_body);
        }
        Self::parse_json(&resp_body)
    }

    /// POST with a custom request body (for file uploads). Returns raw status and body.
    ///
    /// Unlike the other methods this does **not** retry on 401: the body is a
    /// one-shot stream that's consumed on send and can't be replayed. A large
    /// upload is exactly the case where the token may expire mid-flight, but
    /// the failure that matters surfaces on the *next* request (e.g. the load
    /// POST), which does retry. See `databases::tables_load`.
    pub fn post_body<R: std::io::Read + Send + 'static>(
        &self,
        path: &str,
        content_type: &str,
        reader: R,
        content_length: Option<u64>,
    ) -> (reqwest::StatusCode, String) {
        let url = format!("{}{path}", self.api_url);
        let mut req = self
            .build_request(reqwest::Method::POST, &url)
            .header("Content-Type", content_type);
        if let Some(len) = content_length {
            req = req.header("Content-Length", len);
        }
        let req = req.body(reqwest::blocking::Body::new(reader));
        // Execute on the upload client (no request timeout) rather than the
        // default 300s client — `build_request`'s originating client is
        // irrelevant once the request is built, since the executing client's
        // timeout is what applies. Body is an opaque stream, so pass `None`
        // for logging; headers (including the masked Authorization) still log.
        let upload_client = build_upload_client();
        match util::send_debug(&upload_client, req, None) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// Best-effort re-resolution of the bearer token, mirroring the auth-source
/// precedence in [`ApiClient::new`] but returning `None` on failure instead
/// of exiting. Used by the 401-retry refresher: at refresh time we're already
/// past startup, so a failure just means "couldn't refresh, surface the
/// original error" rather than a fatal startup diagnostic.
fn resolve_fresh_token(profile_config: &config::ProfileConfig) -> Option<String> {
    let api_url = profile_config.api_url.to_string();
    if std::env::var("HOTDATA_DATABASE_TOKEN").is_ok() {
        crate::database_session::refresh_from_env(&api_url)
    } else if std::env::var("HOTDATA_SANDBOX_TOKEN").is_ok() {
        crate::sandbox_session::refresh_from_env(&api_url)
    } else if crate::sandbox_session::load().is_some() {
        crate::sandbox_session::ensure_access_token(&api_url)
    } else {
        let api_key_fallback = profile_config
            .api_key
            .as_deref()
            .filter(|k| !k.is_empty() && *k != "PLACEHOLDER");
        crate::jwt::ensure_access_token(profile_config, api_key_fallback).ok()
    }
}

/// Decide what error text to print for a failed response. Pulled out as a pure
/// function so the 4xx-to-re-auth-hint logic can be unit-tested without
/// making real HTTP calls or touching `std::process::exit`.
fn format_fail_message(
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
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Probe {
        n: i32,
    }

    #[test]
    fn get_none_if_not_found_returns_none_on_404() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/missing")
            .match_header("Authorization", "Bearer test-key")
            .with_status(404)
            .create();

        let api = ApiClient::test_new(&server.url(), "test-key", None);
        let got: Option<Probe> = api.get_none_if_not_found("/missing");
        assert!(got.is_none());
        mock.assert();
    }

    #[test]
    fn delete_raw_returns_status_and_body() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("DELETE", "/widgets/abc")
            .match_header("Authorization", "Bearer test-key")
            .with_status(204)
            .with_body("")
            .create();

        let api = ApiClient::test_new(&server.url(), "test-key", None);
        let (status, body) = api.delete_raw("/widgets/abc");
        assert_eq!(status.as_u16(), 204);
        assert!(body.is_empty());
        mock.assert();
    }

    #[test]
    fn delete_raw_surfaces_error_body_on_4xx() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("DELETE", "/widgets/missing")
            .with_status(404)
            .with_body(r#"{"error":{"message":"not found"}}"#)
            .create();

        let api = ApiClient::test_new(&server.url(), "test-key", None);
        let (status, body) = api.delete_raw("/widgets/missing");
        assert_eq!(status.as_u16(), 404);
        assert!(body.contains("not found"));
        mock.assert();
    }

    #[test]
    fn get_none_if_not_found_returns_some_on_200() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/ok")
            .match_header("Authorization", "Bearer test-key")
            .match_header("X-Workspace-Id", "ws-1")
            .with_status(200)
            .with_body(r#"{"n":7}"#)
            .create();

        let api = ApiClient::test_new(&server.url(), "test-key", Some("ws-1"));
        let got: Option<Probe> = api.get_none_if_not_found("/ok");
        assert_eq!(got.unwrap().n, 7);
        mock.assert();
    }

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
        // This is the user-reported scenario: the server masks an auth failure
        // behind a 404 with an empty body. The re-auth probe catches it.
        let msg = format_fail_message(
            reqwest::StatusCode::NOT_FOUND,
            "",
            Some(&AuthStatus::Invalid(401)),
        );
        assert!(msg.contains("API key is invalid"), "got: {msg}");
    }

    #[test]
    fn format_fail_message_404_with_valid_key_shows_real_error() {
        // If the auth probe says the key is fine, surface the upstream body.
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
        // 5xx is not a client error — the auth probe is not even run, so
        // `auth_status` is None from the caller and we just surface the body.
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
        // If the probe itself couldn't reach the API, we can't claim the key
        // is invalid — surface the original body instead.
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
        // Caller couldn't load config (None) — still surface the upstream error.
        let body = "plain body";
        let msg = format_fail_message(reqwest::StatusCode::NOT_FOUND, body, None);
        assert!(!msg.contains("API key is invalid"));
        assert_eq!(msg, "plain body");
    }

    #[test]
    fn post_raw_retries_once_with_refreshed_token_after_401() {
        let mut server = mockito::Server::new();
        // First attempt: stale bearer is rejected.
        let stale = server
            .mock("POST", "/load")
            .match_header("Authorization", "Bearer stale-token")
            .with_status(401)
            .with_body("Invalid api key")
            .create();
        // Retry: client must mint a fresh bearer and the server accepts it.
        let fresh = server
            .mock("POST", "/load")
            .match_header("Authorization", "Bearer fresh-token")
            .with_status(200)
            .with_body(r#"{"ok":true}"#)
            .create();

        let api = ApiClient::test_new_with_refresh(
            &server.url(),
            "stale-token",
            None,
            std::sync::Arc::new(|| Some("fresh-token".to_string())),
        );
        let (status, body) = api.post_raw("/load", &serde_json::json!({"upload_id": "u1"}));

        assert_eq!(
            status.as_u16(),
            200,
            "retry should surface the 200, got body: {body}"
        );
        assert!(body.contains("\"ok\":true"));
        stale.assert();
        fresh.assert();
    }

    #[test]
    fn get_retries_once_with_refreshed_token_after_401() {
        let mut server = mockito::Server::new();
        let stale = server
            .mock("GET", "/ok")
            .match_header("Authorization", "Bearer stale-token")
            .with_status(401)
            .with_body("Invalid api key")
            .create();
        let fresh = server
            .mock("GET", "/ok")
            .match_header("Authorization", "Bearer fresh-token")
            .with_status(200)
            .with_body(r#"{"n":7}"#)
            .create();

        let api = ApiClient::test_new_with_refresh(
            &server.url(),
            "stale-token",
            None,
            std::sync::Arc::new(|| Some("fresh-token".to_string())),
        );
        let got: Probe = api.get("/ok");
        assert_eq!(got.n, 7);
        stale.assert();
        fresh.assert();
    }

    #[test]
    fn does_not_retry_on_non_401() {
        // A 500 is not an auth problem — the client must not refresh or retry.
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/load")
            .with_status(500)
            .with_body("boom")
            .expect(1) // exactly one request, no retry
            .create();

        let refreshed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = refreshed.clone();
        let api = ApiClient::test_new_with_refresh(
            &server.url(),
            "stale-token",
            None,
            std::sync::Arc::new(move || {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Some("fresh-token".to_string())
            }),
        );
        let (status, _) = api.post_raw("/load", &serde_json::json!({}));
        assert_eq!(status.as_u16(), 500);
        assert!(
            !refreshed.load(std::sync::atomic::Ordering::SeqCst),
            "refresher must not be called on a non-401 response"
        );
        mock.assert();
    }

    #[test]
    fn retries_at_most_once_then_surfaces_401() {
        // Both attempts 401 → give up after a single retry (no infinite loop).
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/load")
            .with_status(401)
            .with_body("Invalid api key")
            .expect(2) // original + one retry, then stop
            .create();

        let api = ApiClient::test_new_with_refresh(
            &server.url(),
            "stale-token",
            None,
            std::sync::Arc::new(|| Some("still-bad-token".to_string())),
        );
        let (status, body) = api.post_raw("/load", &serde_json::json!({}));
        assert_eq!(status.as_u16(), 401);
        assert!(body.contains("Invalid api key"));
        mock.assert();
    }

    #[test]
    fn does_not_retry_when_refresher_cannot_mint() {
        // Refresher returns None (e.g. dead refresh token, no API key) → the
        // original 401 is surfaced unchanged, with no second request.
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/load")
            .with_status(401)
            .with_body("Invalid api key")
            .expect(1)
            .create();

        let api = ApiClient::test_new_with_refresh(
            &server.url(),
            "stale-token",
            None,
            std::sync::Arc::new(|| None),
        );
        let (status, _) = api.post_raw("/load", &serde_json::json!({}));
        assert_eq!(status.as_u16(), 401);
        mock.assert();
    }

    #[test]
    fn post_body_does_not_retry_on_401() {
        // Streaming uploads can't be replayed, so a 401 here is surfaced as-is
        // and the refresher is never consulted.
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/files")
            .with_status(401)
            .with_body("Invalid api key")
            .expect(1)
            .create();

        let refreshed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = refreshed.clone();
        let api = ApiClient::test_new_with_refresh(
            &server.url(),
            "stale-token",
            None,
            std::sync::Arc::new(move || {
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                Some("fresh-token".to_string())
            }),
        );
        let data = b"parquet-bytes".to_vec();
        let (status, _) = api.post_body(
            "/files",
            "application/octet-stream",
            std::io::Cursor::new(data),
            None,
        );
        assert_eq!(status.as_u16(), 401);
        assert!(
            !refreshed.load(std::sync::atomic::Ordering::SeqCst),
            "streaming upload must not trigger a token refresh/retry"
        );
        mock.assert();
    }

    #[test]
    fn format_fail_message_4xx_authenticated_probe_shows_server_message() {
        // Valid key but a genuine client error — upstream message wins.
        let body = r#"{"error":{"message":"workspace_not_found"}}"#;
        let msg = format_fail_message(
            reqwest::StatusCode::NOT_FOUND,
            body,
            Some(&AuthStatus::Authenticated),
        );
        assert_eq!(msg, "workspace_not_found");
    }
}
