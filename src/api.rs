use crate::auth;
use crate::config;
use crate::util;
use crossterm::style::Stylize;
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub struct ApiClient {
    client: reqwest::blocking::Client,
    api_key: String,
    pub api_url: String,
    workspace_id: Option<String>,
    sandbox_id: Option<String>,
}

impl ApiClient {
    /// Create a new API client. Loads config, validates auth.
    /// Pass `workspace_id` for endpoints that require it, or `None` for workspace-less endpoints.
    pub fn new(workspace_id: Option<&str>) -> Self {
        let profile_config = match config::load("default") {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };

        let api_key = match &profile_config.api_key {
            Some(key) if key != "PLACEHOLDER" => key.clone(),
            _ => {
                eprintln!(
                    "error: not authenticated. Run 'hotdata auth login' (or 'hotdata auth') to log in."
                );
                std::process::exit(1);
            }
        };

        Self {
            client: reqwest::blocking::Client::new(),
            api_key,
            api_url: profile_config.api_url.to_string(),
            workspace_id: workspace_id.map(String::from),
            sandbox_id: std::env::var("HOTDATA_SANDBOX").ok().or_else(|| {
                if crate::sandbox::find_sandbox_run_ancestor().is_some() {
                    eprintln!("error: sandbox has been lost -- restart the process");
                    std::process::exit(1);
                }
                profile_config.sandbox
            }),
        }
    }

    /// Test-only client (no config load). Used with a local mock HTTP server.
    #[cfg(test)]
    pub(crate) fn test_new(api_url: &str, api_key: &str, workspace_id: Option<&str>) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            api_key: api_key.to_string(),
            api_url: api_url.to_string(),
            workspace_id: workspace_id.map(String::from),
            sandbox_id: None,
        }
    }

    fn debug_headers(&self) -> Vec<(&str, String)> {
        let masked = if self.api_key.len() > 4 {
            format!("Bearer ...{}", &self.api_key[self.api_key.len() - 4..])
        } else {
            "Bearer ***".to_string()
        };
        let mut headers = vec![("Authorization", masked)];
        if let Some(ref ws) = self.workspace_id {
            headers.push(("X-Workspace-Id", ws.clone()));
        }
        if let Some(ref sid) = self.sandbox_id {
            // Send both headers during the session→sandbox migration window.
            headers.push(("X-Session-Id", sid.clone()));
            headers.push(("X-Sandbox-Id", sid.clone()));
        }
        headers
    }

    fn log_request(&self, method: &str, url: &str, body: Option<&serde_json::Value>) {
        let headers = self.debug_headers();
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        util::debug_request(method, url, &header_refs, body);
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
        let mut req = self
            .client
            .request(method, url)
            .header("Authorization", format!("Bearer {}", self.api_key));
        if let Some(ref ws) = self.workspace_id {
            req = req.header("X-Workspace-Id", ws);
        }
        if let Some(ref sid) = self.sandbox_id {
            // Send both headers during the session→sandbox migration window.
            req = req.header("X-Session-Id", sid);
            req = req.header("X-Sandbox-Id", sid);
        }
        req
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
        self.log_request("GET", &url, None);

        let resp = match self
            .build_request(reqwest::Method::GET, &url)
            .query(&filtered)
            .send()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        let (status, body) = util::debug_response(resp);
        if !status.is_success() {
            self.fail_response(status, body);
        }

        match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error parsing response: {e}");
                std::process::exit(1);
            }
        }
    }

    /// GET request, returns parsed response.
    pub fn get<T: DeserializeOwned>(&self, path: &str) -> T {
        let url = format!("{}{path}", self.api_url);
        self.log_request("GET", &url, None);

        let resp = match self.build_request(reqwest::Method::GET, &url).send() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        let (status, body) = util::debug_response(resp);
        if !status.is_success() {
            self.fail_response(status, body);
        }

        match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error parsing response: {e}");
                std::process::exit(1);
            }
        }
    }

    /// GET request; returns `None` on HTTP 404. Other status codes use the same handling as
    /// [`Self::get`]. Used when probing many paths where a missing resource is normal.
    pub fn get_none_if_not_found<T: DeserializeOwned>(&self, path: &str) -> Option<T> {
        let url = format!("{}{path}", self.api_url);
        self.log_request("GET", &url, None);

        let resp = match self.build_request(reqwest::Method::GET, &url).send() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        let (status, body) = util::debug_response(resp);
        if status == reqwest::StatusCode::NOT_FOUND {
            return None;
        }
        if !status.is_success() {
            self.fail_response(status, body);
        }

        match serde_json::from_str(&body) {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("error parsing response: {e}");
                std::process::exit(1);
            }
        }
    }

    /// POST request with JSON body, returns parsed response.
    pub fn post<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> T {
        let url = format!("{}{path}", self.api_url);
        self.log_request("POST", &url, Some(body));

        let resp = match self
            .build_request(reqwest::Method::POST, &url)
            .json(body)
            .send()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        let (status, resp_body) = util::debug_response(resp);
        if !status.is_success() {
            self.fail_response(status, resp_body);
        }

        match serde_json::from_str(&resp_body) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error parsing response: {e}");
                std::process::exit(1);
            }
        }
    }

    /// GET request, exits only on connection error, returns raw (status, body).
    /// Use for best-effort endpoints (e.g. health checks) where the caller wants
    /// to handle non-2xx responses gracefully instead of aborting.
    pub fn get_raw(&self, path: &str) -> (reqwest::StatusCode, String) {
        let url = format!("{}{path}", self.api_url);
        self.log_request("GET", &url, None);

        let resp = match self.build_request(reqwest::Method::GET, &url).send() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        util::debug_response(resp)
    }

    /// POST request with JSON body, exits on error, returns raw (status, body).
    pub fn post_raw(&self, path: &str, body: &serde_json::Value) -> (reqwest::StatusCode, String) {
        let url = format!("{}{path}", self.api_url);
        self.log_request("POST", &url, Some(body));

        let resp = match self
            .build_request(reqwest::Method::POST, &url)
            .json(body)
            .send()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        util::debug_response(resp)
    }

    /// PATCH request with JSON body, returns parsed response.
    pub fn patch<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> T {
        let url = format!("{}{path}", self.api_url);
        self.log_request("PATCH", &url, Some(body));

        let resp = match self
            .build_request(reqwest::Method::PATCH, &url)
            .json(body)
            .send()
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        let (status, resp_body) = util::debug_response(resp);
        if !status.is_success() {
            self.fail_response(status, resp_body);
        }

        match serde_json::from_str(&resp_body) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error parsing response: {e}");
                std::process::exit(1);
            }
        }
    }

    /// POST with a custom request body (for file uploads). Returns raw status and body.
    pub fn post_body<R: std::io::Read + Send + 'static>(
        &self,
        path: &str,
        content_type: &str,
        reader: R,
        content_length: Option<u64>,
    ) -> (reqwest::StatusCode, String) {
        let url = format!("{}{path}", self.api_url);
        self.log_request("POST", &url, None);

        let mut req = self
            .build_request(reqwest::Method::POST, &url)
            .header("Content-Type", content_type);

        if let Some(len) = content_length {
            req = req.header("Content-Length", len);
        }

        let resp = match req.body(reqwest::blocking::Body::new(reader)).send() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        util::debug_response(resp)
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
