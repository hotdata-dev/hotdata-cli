use crate::client::sdk::Api;
use crate::util::human_bytes;
use serde::{Deserialize, Serialize};

/// CLI output shape for `usage`, mapped from the `/v1/usage`
/// (`WorkspaceUsageResponse`) body.
///
/// `since` is the start of the reporting window. `query_count` and
/// `bytes_scanned` accrue per query in real time; `storage_bytes` is a periodic
/// snapshot taken at `storage_captured_at`, so it lags writes (uploads) by up to
/// one capture interval and is not real-time.
#[derive(Deserialize, Serialize)]
struct Usage {
    since: String,
    query_count: i64,
    bytes_scanned: i64,
    storage_bytes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    storage_captured_at: Option<String>,
}

/// `hotdata usage` — workspace usage for the current billing window (or since a
/// caller-supplied timestamp).
pub fn usage(workspace_id: &str, since: Option<&str>, format: &str) {
    let api = Api::new(Some(workspace_id));
    let query: Vec<(&str, String)> = since
        .map(|s| vec![("since", s.to_string())])
        .unwrap_or_default();
    let spinner = crate::util::spinner("Loading usage…");
    let result = api.get_json("/usage", &query);
    spinner.finish_and_clear();
    let u: Usage = result.unwrap_or_else(|e| e.exit());

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&u).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&u).unwrap()),
        _ => {
            let rows = vec![
                vec!["since".to_string(), u.since.clone()],
                vec!["query_count".to_string(), u.query_count.to_string()],
                vec!["bytes_scanned".to_string(), human_bytes(u.bytes_scanned)],
                vec!["storage_bytes".to_string(), human_bytes(u.storage_bytes)],
                vec![
                    "storage_captured_at".to_string(),
                    u.storage_captured_at
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ],
            ];
            crate::output::table::print(&["METRIC", "VALUE"], &rows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_deserializes_real_response_shape() {
        // Mirrors a live `/v1/usage` body; storage_captured_at may be null.
        let body = r#"{"since":"2026-06-01T00:00:00Z","bytes_scanned":19814572,
            "query_count":184,"storage_bytes":98209424,
            "storage_captured_at":"2026-06-20T00:55:19Z"}"#;
        let u: Usage = serde_json::from_str(body).unwrap();
        assert_eq!(u.query_count, 184);
        assert_eq!(u.bytes_scanned, 19814572);
        assert_eq!(u.storage_bytes, 98209424);
        assert_eq!(u.since, "2026-06-01T00:00:00Z");

        // Round-trips back out for -o json/yaml consumers.
        let out = serde_json::to_string(&u).unwrap();
        assert!(out.contains("\"query_count\":184"));
    }

    #[test]
    fn usage_tolerates_null_storage_captured_at() {
        let body = r#"{"since":"2026-06-01T00:00:00Z","bytes_scanned":0,
            "query_count":0,"storage_bytes":0,"storage_captured_at":null}"#;
        let u: Usage = serde_json::from_str(body).unwrap();
        assert!(u.storage_captured_at.is_none());
    }
}
