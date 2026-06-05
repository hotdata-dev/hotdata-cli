//! Scenario: connections_read.
//!
//! Read-only lifecycle on the seeded connection. The CLI exposes `connections
//! list` and `connections <id>` (the latter also runs a health check and folds
//! the result into its output); it has no separate cache-purge surface, so this
//! mirror covers get + list + health. Does not create or delete connections in
//! prod (would require real datastore credentials).

mod common;

#[test]
fn connections_read() {
    let (cli, connection_id) = skip_if_no_connection!();

    // list_connections — the seeded connection must be present.
    let listing = cli.json(&["connections", "list", "-o", "json"]);
    let connections = listing
        .as_array()
        .expect("connections list -o json should be a JSON array");
    let ids: Vec<&str> = connections
        .iter()
        .filter_map(|c| c.get("id").and_then(|v| v.as_str()))
        .collect();
    assert!(
        ids.contains(&connection_id.as_str()),
        "seeded connection {connection_id} not in connections list, got {ids:?}"
    );

    // get_connection (+ health) — the detail view reports the same id and a
    // health block.
    let detail = cli.json(&["connections", &connection_id, "-o", "json"]);
    assert_eq!(
        detail.get("id").and_then(|v| v.as_str()),
        Some(connection_id.as_str()),
        "connections <id> returned the wrong id: {detail}"
    );
    assert!(
        !detail
            .get("source_type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty(),
        "expected a non-empty source_type: {detail}"
    );
    assert!(
        detail.get("health").is_some(),
        "expected a health block in connections <id> output: {detail}"
    );
}
