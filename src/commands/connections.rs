use crate::client::sdk::{Api, block, none_if_404};

/// Resolve a connection name or ID to a connection ID, returning `Err(message)`
/// when nothing matches.
///
/// If `name_or_id` looks like a raw connection ID (starts with "conn"), tries
/// `GET /connections/{id}` directly first to avoid listing the full workspace.
/// Falls back to listing and matching by name, then to managed-database catalog
/// aliases. Only the "no match" outcome is an `Err`; a transport/API failure
/// during resolution still exits (the API is unreachable — not "this name is
/// wrong"), preserving the auth-aware error from [`ApiError::exit`].
///
/// [`resolve_connection_id`] is the exiting wrapper for callers with no recovery
/// path; the `Result` form lets `databases create --attach` warn on a bad name
/// and continue to the next attachment instead of aborting mid-loop.
pub fn try_resolve_connection_id(api: &Api, name_or_id: &str) -> Result<String, String> {
    if name_or_id.starts_with("conn") {
        // Existence probe: a 404 just means "not a raw id", fall through to the
        // name/catalog lookup; any other error is fatal.
        if none_if_404(block(api.client().connections().get(name_or_id)))
            .unwrap_or_else(|e| e.exit())
            .is_some()
        {
            return Ok(name_or_id.to_string());
        }
    }

    // Before listing connections, check if the active database's catalog or name
    // matches — prefer it over any stale connection entry with the same name.
    if let Some(ws) = api.workspace_id()
        && let Some(active_id) = crate::config::load_current_database("default", ws)
        && let Some(active_db) =
            none_if_404(crate::commands::databases::get_database(api, &active_id))
                .unwrap_or_else(|e| e.exit())
        && (active_db.default_catalog.as_deref() == Some(name_or_id)
            || active_db.name.as_deref() == Some(name_or_id))
    {
        return Ok(active_db.default_connection_id);
    }

    let resp = block(api.client().connections().list()).unwrap_or_else(|e| e.exit());
    if let Some(conn) = resp
        .connections
        .iter()
        .find(|c| c.id == name_or_id || c.name == name_or_id)
    {
        return Ok(conn.id.clone());
    }

    // Fall back to managed databases: treat name_or_id as a catalog alias.
    if let Ok(db) = crate::commands::databases::try_resolve_database(api, name_or_id) {
        return Ok(db.default_connection_id);
    }

    Err(format!("no catalog with id '{name_or_id}'"))
}

/// Resolve a connection name or ID to a connection ID, exiting on failure.
/// Exiting wrapper around [`try_resolve_connection_id`].
pub fn resolve_connection_id(api: &Api, name_or_id: &str) -> String {
    use crossterm::style::Stylize;
    match try_resolve_connection_id(api, name_or_id) {
        Ok(id) => id,
        Err(msg) => {
            eprintln!("{}", format!("error: {msg}").red());
            std::process::exit(1);
        }
    }
}
