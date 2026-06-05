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
//! * the streaming `/files` upload (10 GB+, `--url` source, progress bar, no
//!   request timeout, no 401-retry) — the SDK's `uploads().upload` is
//!   `PathBuf`-only;
//! * `skill.rs`'s arbitrary-URL markdown fetch.
//!
//! This module owns the two blocking client builders (one timeout-bounded, one
//! no-timeout for uploads) and a thin bearer/header request builder. It does
//! NOT carry the old `ApiClient`'s 401-retry loop: token freshness is now the
//! `CliTokenProvider`'s job (proactive refresh at the 30s leeway), and the
//! upload path was always one-shot anyway.

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

/// Client used only for streaming file uploads. Deliberately has **no** request
/// timeout: an upload's duration scales with file size and uplink (a 10 GB
/// parquet takes far longer than `HTTP_REQUEST_TIMEOUT`, which is sized for
/// slow server-side work), so a wall-clock cap would abort healthy-but-slow
/// transfers. TCP keepalive is kept so a genuinely dead peer is still reaped by
/// the OS; a live-but-slow upload runs to completion and the user can Ctrl-C.
pub fn build_upload_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
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
    fn upload_client_builds() {
        let _ = build_upload_client();
    }

    #[test]
    fn redact_keys_cover_token_fields() {
        // Guards against silently dropping a sensitive key from debug logs.
        assert!(TOKEN_REDACT_KEYS.contains(&"access_token"));
        assert!(TOKEN_REDACT_KEYS.contains(&"refresh_token"));
        assert!(TOKEN_REDACT_KEYS.contains(&"api_token"));
    }
}
