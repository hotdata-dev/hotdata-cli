//! Scenario: workspaces_list.
//!
//! `hotdata workspaces list -o json` returns the workspaces visible to the
//! seeded credentials and includes the seeded HOTDATA_SDK_TEST_WORKSPACE_ID.
//! Read-only — never creates or deletes workspaces against prod.

mod common;

#[test]
fn workspaces_list() {
    let cli = skip_if_no_creds!();
    let workspace_id = cli.workspace_id().to_string();

    let value = cli.json(&["workspaces", "list", "-o", "json"]);
    let workspaces = value
        .as_array()
        .expect("workspaces list -o json should be a JSON array");

    let ids: Vec<&str> = workspaces
        .iter()
        .filter_map(|w| w.get("public_id").and_then(|v| v.as_str()))
        .collect();
    assert!(
        ids.contains(&workspace_id.as_str()),
        "expected seeded workspace {workspace_id} in list, got {ids:?}"
    );
}
