//! Locating node.

use anyhow::Result;
use semver::Version;
use std::process::Command;
use thiserror::Error;

/// The first Node with both `module.stripTypeScriptTypes`, which builds a
/// TypeScript carrier, and an unflagged `node:sqlite`, which `site.sql` runs on.
const MINIMUM: Version = Version::new(22, 13, 0);

#[derive(Error, Debug)]
pub(crate) enum NodeError {
    #[error("`node` is not on PATH. Carriers need Node >= {MINIMUM}.")]
    NotFound,
    #[error("`node --version` printed something unparseable: {0}")]
    UnknownVersion(String),
    #[error("carriers need Node >= {MINIMUM}, but `node` is {0}.")]
    TooOld(Version),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeInfo {
    pub version: Version,
}

impl NodeInfo {
    pub fn detect() -> Result<Self> {
        let output = Command::new("node")
            .arg("--version")
            .output()
            .map_err(|_| NodeError::NotFound)?;
        let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Self::from_version(&printed)
    }

    fn from_version(printed: &str) -> Result<Self> {
        let version = printed
            .strip_prefix('v')
            .unwrap_or(printed)
            .parse::<Version>()
            .map_err(|_| NodeError::UnknownVersion(printed.to_string()))?;
        if version < MINIMUM {
            return Err(NodeError::TooOld(version).into());
        }
        Ok(Self { version })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_older_than_the_minimum_is_refused() {
        assert!(NodeInfo::from_version("v18.19.0").is_err());
        assert!(NodeInfo::from_version("v22.12.0").is_err());
        assert!(NodeInfo::from_version("not a version").is_err());
        assert_eq!(
            NodeInfo::from_version("v22.13.0").unwrap().version,
            Version::new(22, 13, 0)
        );
    }
}
