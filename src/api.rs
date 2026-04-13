use crate::config;
use crate::util;
use crossterm::style::Stylize;
use serde::de::DeserializeOwned;

pub struct ApiClient {
    client: reqwest::blocking::Client,
    api_key: String,
    pub api_url: String,
    workspace_id: Option<String>,
    session_id: Option<String>,
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
                eprintln!("error: not authenticated. Run 'hotdata auth' to log in.");
                std::process::exit(1);
            }
        };

        Self {
            client: reqwest::blocking::Client::new(),
            api_key,
            api_url: profile_config.api_url.to_string(),
            workspace_id: workspace_id.map(String::from),
            session_id: std::env::var("HOTDATA_SESSION").ok().or_else(|| {
                if crate::sessions::find_session_run_ancestor().is_some() {
                    eprintln!("error: session has been lost -- restart the process");
                    std::process::exit(1);
                }
                profile_config.session
            }),
        }
    }

    fn debug_headers(&self) -> Vec<(&str, String)> {
        let masked = if self.api_key.len() > 4 {
            format!("Bearer ...{}", &self.api_key[self.api_key.len()-4..])
        } else {
            "Bearer ***".to_string()
        };
        let mut headers = vec![("Authorization", masked)];
        if let Some(ref ws) = self.workspace_id {
            headers.push(("X-Workspace-Id", ws.clone()));
        }
        if let Some(ref sid) = self.session_id {
            headers.push(("X-Session-Id", sid.clone()));
        }
        headers
    }

    fn log_request(&self, method: &str, url: &str, body: Option<&serde_json::Value>) {
        let headers = self.debug_headers();
        let header_refs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        util::debug_request(method, url, &header_refs, body);
    }

    fn build_request(&self, method: reqwest::Method, url: &str) -> reqwest::blocking::RequestBuilder {
        let mut req = self.client.request(method, url)
            .header("Authorization", format!("Bearer {}", self.api_key));
        if let Some(ref ws) = self.workspace_id {
            req = req.header("X-Workspace-Id", ws);
        }
        if let Some(ref sid) = self.session_id {
            req = req.header("X-Session-Id", sid);
        }
        req
    }

    /// GET request with query parameters, returns parsed response.
    /// Parameters with `None` values are omitted.
    pub fn get_with_params<T: DeserializeOwned>(&self, path: &str, params: &[(&str, Option<String>)]) -> T {
        let filtered: Vec<(&str, &String)> = params.iter()
            .filter_map(|(k, v)| v.as_ref().map(|val| (*k, val)))
            .collect();
        let url = format!("{}{path}", self.api_url);
        self.log_request("GET", &url, None);

        let resp = match self.build_request(reqwest::Method::GET, &url).query(&filtered).send() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        let (status, body) = util::debug_response(resp);
        if !status.is_success() {
            eprintln!("{}", util::api_error(body).red());
            std::process::exit(1);
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
            eprintln!("{}", util::api_error(body).red());
            std::process::exit(1);
        }

        match serde_json::from_str(&body) {
            Ok(v) => v,
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

        let resp = match self.build_request(reqwest::Method::POST, &url)
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
            eprintln!("{}", util::api_error(resp_body).red());
            std::process::exit(1);
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

        let resp = match self.build_request(reqwest::Method::POST, &url)
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

    /// POST request with no body (e.g. execute endpoints), returns parsed response.
    pub fn post_empty<T: DeserializeOwned>(&self, path: &str) -> T {
        let url = format!("{}{path}", self.api_url);
        self.log_request("POST", &url, None);

        let resp = match self.build_request(reqwest::Method::POST, &url).send() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error connecting to API: {e}");
                std::process::exit(1);
            }
        };

        let (status, resp_body) = util::debug_response(resp);
        if !status.is_success() {
            eprintln!("{}", util::api_error(resp_body).red());
            std::process::exit(1);
        }

        match serde_json::from_str(&resp_body) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error parsing response: {e}");
                std::process::exit(1);
            }
        }
    }

    /// PUT request with JSON body, returns parsed response.
    pub fn put<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> T {
        let url = format!("{}{path}", self.api_url);
        self.log_request("PUT", &url, Some(body));

        let resp = match self.build_request(reqwest::Method::PUT, &url)
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
            eprintln!("{}", util::api_error(resp_body).red());
            std::process::exit(1);
        }

        match serde_json::from_str(&resp_body) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error parsing response: {e}");
                std::process::exit(1);
            }
        }
    }


    /// PATCH request with JSON body, returns parsed response.
    pub fn patch<T: DeserializeOwned>(&self, path: &str, body: &serde_json::Value) -> T {
        let url = format!("{}{path}", self.api_url);
        self.log_request("PATCH", &url, Some(body));

        let resp = match self.build_request(reqwest::Method::PATCH, &url)
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
            eprintln!("{}", util::api_error(resp_body).red());
            std::process::exit(1);
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

        let mut req = self.build_request(reqwest::Method::POST, &url)
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
