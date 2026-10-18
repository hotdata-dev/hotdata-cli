//! Raw-HTTP client for `POST {api_url}/v1/support/issues`.
//!
//! This is a normal API-gateway route (`api_url`, default
//! `https://api.hotdata.dev/v1`), not a webapp/OAuth one — same host every
//! other command hits. No SDK operation exists for it yet, so it rides the
//! hand-rolled `reqwest::blocking` seam like `client::ingest`.

use crate::client::jwt;
use crate::config;
use crate::util;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// Retry a failed POST once after this long — long enough that a transient
/// blip (a dropped connection, a mid-deploy 502) has usually cleared. The
/// idempotency key carried on the request is what makes a retried create safe.
const RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize)]
pub struct SupportIssueRequest {
    pub subject: String,
    pub body: String,
    pub kind: String,
    pub severity: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logs: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SupportIssue {
    pub public_id: String,
    pub status: String,
    pub subject: String,
    pub kind: String,
    pub severity: String,
    pub workspace_public_id: Option<String>,
    pub created_at: String,
}

#[derive(Deserialize)]
struct SupportIssueEnvelope {
    issue: SupportIssue,
}

/// A typed error from the support-issue call, mirroring the shape other raw
/// clients (`client::ingest`) use so callers can pattern-match on it.
#[derive(Debug)]
pub enum SupportError {
    /// Non-2xx response; `body` is the server's (unredacted) response text.
    Http { status: u16, body: String },
    /// Transport/connection failure.
    Connection(String),
    /// 2xx whose body didn't match the expected envelope.
    Decode(String),
    /// Could not resolve a bearer token (`jwt::ensure_access_token` failed).
    Auth(String),
}

impl SupportError {
    /// Worth a single retry: a connection never completed, or the server
    /// itself failed (5xx). A 4xx is the server telling us the request is
    /// wrong — retrying it verbatim would just fail again.
    fn is_retryable(&self) -> bool {
        matches!(self, SupportError::Connection(_))
            || matches!(self, SupportError::Http { status, .. } if *status >= 500)
    }
}

/// Same construction as `client::sdk`'s `probe_runtime_status`: strip the
/// configured `api_url`'s `/v1` suffix via `sdk_base_path`, then add the one
/// `/v1/support/issues` itself expects.
fn url(profile: &config::ProfileConfig) -> String {
    let base = crate::client::sdk::sdk_base_path(&profile.api_url);
    format!("{}/v1/support/issues", base.trim_end_matches('/'))
}

fn send_once(
    client: &reqwest::blocking::Client,
    profile: &config::ProfileConfig,
    token: &str,
    workspace_id: Option<&str>,
    req: &SupportIssueRequest,
) -> Result<(SupportIssue, bool), SupportError> {
    let body = serde_json::to_value(req).expect("SupportIssueRequest serializes");
    let mut builder = client
        .post(url(profile))
        .header("Authorization", format!("Bearer {token}"))
        .header(
            "User-Agent",
            concat!("hotdata-cli/", env!("CARGO_PKG_VERSION")),
        );
    // Same header every other /v1 call carries: the gateway ranks a longer
    // path prefix above a header match, so /v1/support keeps routing here
    // (not to a workspace's runtimedb worker) even with it present.
    if let Some(ws) = workspace_id {
        builder = builder.header("X-Workspace-Id", ws);
    }
    let builder = builder.json(&body);
    let (status, body_text) = util::send_debug(client, builder, Some(&body))
        .map_err(|e| SupportError::Connection(e.to_string()))?;
    if !status.is_success() {
        return Err(SupportError::Http {
            status: status.as_u16(),
            body: body_text,
        });
    }
    // 200 is the idempotent-replay shape; 202 the freshly-queued one — same
    // envelope either way, so only the status tells them apart.
    let replay = status.as_u16() == 200;
    let parsed: SupportIssueEnvelope =
        serde_json::from_str(&body_text).map_err(|e| SupportError::Decode(e.to_string()))?;
    Ok((parsed.issue, replay))
}

/// File a support issue. `workspace_id`, when given, is sent only as the
/// `X-Workspace-Id` header (never in the JSON body). Retries once, after
/// [`RETRY_DELAY`], on a connection error or 5xx — never on a 4xx — reusing
/// the same `idempotency_key` so a retried create can't double-file.
pub fn post_support_issue(
    profile: &config::ProfileConfig,
    workspace_id: Option<&str>,
    req: &SupportIssueRequest,
) -> Result<(SupportIssue, bool), SupportError> {
    post_support_issue_with_delay(profile, workspace_id, req, RETRY_DELAY)
}

/// `pub(crate)` so a cross-module test (`commands::support`) can drive the
/// real retry-once path with `Duration::ZERO` instead of eating the full
/// [`RETRY_DELAY`] every run.
pub(crate) fn post_support_issue_with_delay(
    profile: &config::ProfileConfig,
    workspace_id: Option<&str>,
    req: &SupportIssueRequest,
    retry_delay: Duration,
) -> Result<(SupportIssue, bool), SupportError> {
    // Same trust filter as sdk::Api / client::ingest: an empty or template
    // key must fall through to the session JWT, not ship as a bearer.
    let api_key_fallback = profile
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty() && *k != "PLACEHOLDER");
    let token = jwt::ensure_access_token(profile, api_key_fallback).map_err(SupportError::Auth)?;
    let client = crate::client::raw_http::build_http_client();

    match send_once(&client, profile, &token, workspace_id, req) {
        Ok(ok) => Ok(ok),
        Err(e) if e.is_retryable() => {
            std::thread::sleep(retry_delay);
            send_once(&client, profile, &token, workspace_id, req)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiUrl, ProfileConfig, test_helpers::with_temp_config_dir};

    /// A profile with an api_key set, so every test here resolves a bearer
    /// with zero network calls (no session mint/refresh to mock separately).
    fn mock_profile(url: &str) -> ProfileConfig {
        ProfileConfig {
            api_key: Some("hd_test_key".to_string()),
            api_url: ApiUrl(Some(url.to_string())),
            ..Default::default()
        }
    }

    fn req(idempotency_key: &str) -> SupportIssueRequest {
        SupportIssueRequest {
            subject: "Query timing out".into(),
            body: "Queries against my workspace have been hanging for an hour.".into(),
            kind: "bug".into(),
            severity: "high".into(),
            context: BTreeMap::from([("cli_version".to_string(), "0.31.0".to_string())]),
            logs: None,
            idempotency_key: idempotency_key.to_string(),
        }
    }

    #[test]
    fn happy_path_202_is_not_a_replay() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/support/issues")
            .match_header("Authorization", "Bearer hd_test_key")
            .match_header("content-type", "application/json")
            .match_header("X-Workspace-Id", "work_abc")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "subject": "Query timing out",
                "kind": "bug",
                "severity": "high",
                "idempotency_key": "fixed-key-1",
            })))
            .with_status(202)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"issue":{"public_id":"supp_1","status":"queued","subject":"Query timing out","kind":"bug","severity":"high","workspace_public_id":"work_abc","created_at":"2026-09-05T00:00:00Z"}}"#,
            )
            .create();

        let profile = mock_profile(&server.url());
        let (issue, replay) = post_support_issue_with_delay(
            &profile,
            Some("work_abc"),
            &req("fixed-key-1"),
            Duration::ZERO,
        )
        .unwrap();
        m.assert();
        assert_eq!(issue.public_id, "supp_1");
        assert_eq!(issue.status, "queued");
        assert!(!replay);
    }

    #[test]
    fn no_workspace_sends_no_x_workspace_id_header() {
        // The JSON body never carries a workspace field at all — see
        // `SupportIssueRequest`, which has no such field to omit — so the
        // only thing left to assert is the header.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/support/issues")
            .match_header("X-Workspace-Id", mockito::Matcher::Missing)
            .with_status(202)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"issue":{"public_id":"supp_none","status":"queued","subject":"s","kind":"bug","severity":"high","workspace_public_id":null,"created_at":"2026-09-05T00:00:00Z"}}"#,
            )
            .create();

        let profile = mock_profile(&server.url());
        post_support_issue_with_delay(&profile, None, &req("k"), Duration::ZERO).unwrap();
        m.assert();
    }

    #[test]
    fn replay_200_is_reported_as_replay() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/support/issues")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"issue":{"public_id":"supp_2","status":"queued","subject":"s","kind":"bug","severity":"high","workspace_public_id":null,"created_at":"2026-09-05T00:00:00Z"}}"#,
            )
            .create();

        let profile = mock_profile(&server.url());
        let (issue, replay) =
            post_support_issue_with_delay(&profile, None, &req("fixed-key-2"), Duration::ZERO)
                .unwrap();
        m.assert();
        assert_eq!(issue.public_id, "supp_2");
        assert!(replay);
    }

    #[test]
    fn server_500_then_202_retries_once_with_same_key() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let key_matcher =
            || mockito::Matcher::PartialJson(serde_json::json!({"idempotency_key": "fixed-key-3"}));
        // Registered first: consumed by request #1 (default expectation is
        // satisfied after a single hit, so request #2 falls through to the
        // mock below).
        let fail = server
            .mock("POST", "/v1/support/issues")
            .match_body(key_matcher())
            .with_status(500)
            .expect(1)
            .create();
        let ok = server
            .mock("POST", "/v1/support/issues")
            .match_body(key_matcher())
            .with_status(202)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"issue":{"public_id":"supp_3","status":"queued","subject":"s","kind":"bug","severity":"high","workspace_public_id":null,"created_at":"2026-09-05T00:00:00Z"}}"#,
            )
            .expect(1)
            .create();

        let profile = mock_profile(&server.url());
        let (issue, replay) =
            post_support_issue_with_delay(&profile, None, &req("fixed-key-3"), Duration::ZERO)
                .unwrap();
        fail.assert();
        ok.assert();
        assert_eq!(issue.public_id, "supp_3");
        assert!(!replay);
    }

    #[test]
    fn connection_error_retries_once_then_gives_up() {
        // Nothing listens on port 1 for either attempt — both fail at the
        // transport level, and the caller sees a Connection error, not a hang.
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = mock_profile("http://127.0.0.1:1");
        let err =
            post_support_issue_with_delay(&profile, None, &req("k"), Duration::ZERO).unwrap_err();
        assert!(matches!(err, SupportError::Connection(_)));
    }

    #[test]
    fn client_error_is_not_retried() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/support/issues")
            .with_status(422)
            .with_body(r#"{"error":"subject_required"}"#)
            // Exactly one hit expected — a second request here fails the test.
            .expect(1)
            .create();

        let profile = mock_profile(&server.url());
        let err =
            post_support_issue_with_delay(&profile, None, &req("k"), Duration::ZERO).unwrap_err();
        m.assert();
        assert!(matches!(err, SupportError::Http { status: 422, .. }));
    }

    #[test]
    fn url_strips_the_configured_v1_suffix_and_adds_it_back_once() {
        // DEFAULT_API_URL carries a /v1 suffix; sdk_base_path strips it so
        // this doesn't add up to /v1/v1/support/issues.
        let profile = ProfileConfig {
            api_url: ApiUrl(Some("https://api.hotdata.dev/v1".to_string())),
            ..Default::default()
        };
        assert_eq!(url(&profile), "https://api.hotdata.dev/v1/support/issues");
    }
}
