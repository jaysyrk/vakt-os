use serde::{Deserialize, Serialize};

/// The `<name>.json` a repository publishes beside each `<name>.zrp`.
///
/// `dependencies` carries the edges of the package graph; it is `#[serde(default)]`
/// so a manifest written before the field existed still parses, and an empty
/// list means "depends on nothing" rather than "unknown".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Hex-encoded ed25519 signature over the SHA-256 digest of the archive.
    pub signature: String,
}

impl PackageManifest {
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        let manifest: PackageManifest = serde_json::from_str(json)?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependencies_are_parsed() {
        let manifest = PackageManifest::parse(
            r#"{"name":"vakt-audit","version":"1.0.0","description":"auditor",
                "dependencies":["libvakt","libcore"],"signature":"ab"}"#,
        )
        .unwrap();
        assert_eq!(manifest.dependencies, vec!["libvakt", "libcore"]);
    }

    /// Manifests published before the field existed must still install.
    #[test]
    fn a_manifest_without_dependencies_parses_as_having_none() {
        let manifest = PackageManifest::parse(
            r#"{"name":"old","version":"0.1.0","description":"","signature":"ab"}"#,
        )
        .unwrap();
        assert!(manifest.dependencies.is_empty());
    }

    #[test]
    fn a_manifest_missing_its_signature_is_rejected() {
        assert!(PackageManifest::parse(r#"{"name":"x","version":"1","description":""}"#).is_err());
    }
}
