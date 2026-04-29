//! JSON composition schema version detection and migration.
//!
//! This module is intentionally quiet today: there are no historical schema
//! versions to migrate from yet. Keeping the loader boundary here makes future
//! file-format changes explicit and testable instead of spreading compatibility
//! logic through the parser.

use anyhow::{Result, bail};
use serde_json::Value;

/// The schema version produced by the current Lumen JSON writer/generator.
pub const CURRENT_SCHEMA_VERSION: &str = "0.1.0";

/// Preferred field for saved composition documents.
pub const SCHEMA_VERSION_FIELD: &str = "lumenSchemaVersion";

/// Legacy/alternate field accepted while the public schema format settles.
pub const LEGACY_SCHEMA_VERSION_FIELD: &str = "schemaVersion";

/// Return the declared schema version for a composition document.
///
/// Missing versions are treated as current because all existing compositions
/// predate explicit schema versioning.
pub fn detect_schema_version(root: &Value) -> Result<&str> {
    let Some(obj) = root.as_object() else {
        bail!("root must be an object");
    };

    if let Some(version) = obj.get(SCHEMA_VERSION_FIELD) {
        return version
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("`{SCHEMA_VERSION_FIELD}` must be a string"));
    }

    if let Some(version) = obj.get(LEGACY_SCHEMA_VERSION_FIELD) {
        return version
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("`{LEGACY_SCHEMA_VERSION_FIELD}` must be a string"));
    }

    Ok(CURRENT_SCHEMA_VERSION)
}

/// Migrate a composition JSON value to the current parser contract.
pub fn migrate_to_current(root: &Value) -> Result<Value> {
    let version = detect_schema_version(root)?;

    match version {
        CURRENT_SCHEMA_VERSION => Ok(root.clone()),
        other => bail!("unsupported Lumen schema version `{other}`"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CURRENT_SCHEMA_VERSION, LEGACY_SCHEMA_VERSION_FIELD, SCHEMA_VERSION_FIELD,
        detect_schema_version, migrate_to_current,
    };

    #[test]
    fn missing_schema_version_is_current() {
        let doc = json!({});
        assert_eq!(detect_schema_version(&doc).unwrap(), CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn detects_preferred_schema_version_field() {
        let doc = json!({ SCHEMA_VERSION_FIELD: "0.1.0" });
        assert_eq!(detect_schema_version(&doc).unwrap(), "0.1.0");
    }

    #[test]
    fn detects_legacy_schema_version_field() {
        let doc = json!({ LEGACY_SCHEMA_VERSION_FIELD: "0.1.0" });
        assert_eq!(detect_schema_version(&doc).unwrap(), "0.1.0");
    }

    #[test]
    fn rejects_future_or_unknown_versions_until_a_migration_exists() {
        let doc = json!({ SCHEMA_VERSION_FIELD: "99.0.0" });
        let err = migrate_to_current(&doc).unwrap_err().to_string();
        assert!(err.contains("unsupported Lumen schema version `99.0.0`"));
    }
}
