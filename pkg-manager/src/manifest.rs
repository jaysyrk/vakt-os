use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    pub dependencies: Vec<String>,
    pub signature: String,
}

impl PackageManifest {
    pub fn parse(json: &str) -> anyhow::Result<Self> {
        let manifest: PackageManifest = serde_json::from_str(json)?;
        Ok(manifest)
    }
}
