//! Build-time identity of the application.

/// Product name (tentative; see design decision 11).
pub const APP_NAME: &str = "DocuLens";
/// Crate/package version (kept in sync with `src-tauri/tauri.conf.json` by CI).
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Schema version of `settings.json`.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
/// Schema version of `<workspace>/.doculens/workspace.json`.
pub const WORKSPACE_SCHEMA_VERSION: u32 = 1;
/// Name of the hidden application directory inside a workspace.
pub const WORKSPACE_DIR_NAME: &str = ".doculens";
