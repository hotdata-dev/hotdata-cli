//! Slim raw-HTTP helper for endpoints with no SDK operation.
//!
//! The SDK seam (`src/sdk.rs`) covers the generated API surface, but a handful
//! of endpoints must stay on hand-rolled `reqwest::blocking`:
//!
//! * the PKCE / OAuth token endpoints (`/o/token/`, `/v1/auth/token`) — owned
//!   by `jwt.rs`, no SDK equivalent for the `authorization_code` grant;
//! * the session-token mints (`/v1/auth/database`, `/v1/auth/sandbox`) — a
//!   distinct grant on distinct endpoints (`database_session.rs` /
//!   `sandbox_session.rs`);
//! * `skill.rs`'s arbitrary-URL markdown fetch.
//!
//! (The streaming `/files` upload moved onto the SDK seam's
//! [`Api::upload_stream`](crate::sdk::Api::upload_stream), which owns its own
//! no-timeout client.)
//!
//! This module owns the timeout-bounded blocking client builder and a thin
//! bearer/header request builder. It does NOT carry the old `ApiClient`'s
//! 401-retry loop: token freshness is now the `CliTokenProvider`'s job
//! (proactive refresh at the 30s leeway).

// Consumers (jwt.rs token mints, session mints, the streaming upload,
// skill.rs) are migrated to this helper incrementally; the allow keeps the
// build warning-free until those call sites land.
#![allow(dead_code)]

use std::time::Duration;

/// Cap on any single (non-upload) HTTP request. Connection create + synchronous
/// schema discovery against a slow remote catalog can take over a minute, so
/// this is generous; 5 minutes bounds the worst case if the server hangs.
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// TCP keepalive cadence. Without this, macOS drops a TCP connection that has
/// been quiet (e.g. while the server does slow synchronous work) and reqwest
/// surfaces it as "error sending request" even though the request completed
/// server-side.
const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// JSON keys whose values are redacted in debug request/response logging.
pub const TOKEN_REDACT_KEYS: &[&str] = &["access_token", "refresh_token", "api_token", "code"];

/// A timeout-bounded blocking client for ordinary raw requests (token mints,
/// session mints, arbitrary GETs).
pub fn build_http_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(HTTP_REQUEST_TIMEOUT)
        .tcp_keepalive(TCP_KEEPALIVE_INTERVAL)
        .build()
        .expect("reqwest blocking client should always build with these defaults")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_client_builds() {
        let _ = build_http_client();
    }

    #[test]
    fn redact_keys_cover_token_fields() {
        // Guards against silently dropping a sensitive key from debug logs.
        assert!(TOKEN_REDACT_KEYS.contains(&"access_token"));
        assert!(TOKEN_REDACT_KEYS.contains(&"refresh_token"));
        assert!(TOKEN_REDACT_KEYS.contains(&"api_token"));
    }
}
