//! API client and supporting transport/credential infrastructure.
//!
//! `sdk` is the HTTP client for the Hotdata API; `jwt` and `database_session`
//! handle credential minting and scoped database tokens.

pub mod database_session;
pub mod jwt;
pub mod raw_http;
pub mod sdk;
