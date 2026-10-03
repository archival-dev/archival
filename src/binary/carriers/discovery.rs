//! Finding the carriers in a site and naming each one's entry file.
//!
//! A carrier is built by the toolchain, which resolves the entry itself; the
//! resolution here mirrors it for `archival carriers list`.

use anyhow::Result;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) const CARRIERS_DIR_NAME: &str = "carriers";

const TS_EXTENSIONS: &[&str] = &["ts", "mts"];
const JS_EXTENSIONS: &[&str] = &["js", "mjs"];

fn main_extensions() -> impl Iterator<Item = &'static str> {
    TS_EXTENSIONS.iter().chain(JS_EXTENSIONS).copied()
}

/// A carrier directory name doubles as a URL path segment and as a lookup key
/// in the sidecar's registry, so it may not contain anything that could walk
/// out of either.
pub(crate) fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageJson {
    pub main: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Carrier {
    pub name: String,
    pub root: PathBuf,
    pub package: Option<PackageJson>,
}

impl Carrier {
    pub fn entry(&self) -> Option<PathBuf> {
        resolve_entry(
            &self.root,
            self.package.as_ref().and_then(|p| p.main.as_deref()),
        )
    }

    pub fn expected_entry(&self) -> String {
        match self.package.as_ref().and_then(|p| p.main.as_deref()) {
            Some(main) => format!("the \"main\" declared in package.json ({})", main),
            None => main_extensions()
                .map(|ext| format!("index.{}", ext))
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// Every carrier in a site, sorted by name. An entry that isn't a directory, or
/// whose name can't be a URL segment, is skipped.
pub(crate) fn discover(site_root: &Path) -> Result<Vec<Carrier>> {
    let dir = site_root.join(CARRIERS_DIR_NAME);
    if !dir.is_dir() {
        return Ok(vec![]);
    }
    let mut carriers = vec![];
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_valid_name(&name) {
            tracing::warn!("skipping carrier directory with unusable name: {}", name);
            continue;
        }
        let root = entry.path();
        carriers.push(Carrier {
            package: read_package(&root),
            name,
            root,
        });
    }
    carriers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(carriers)
}

fn read_package(root: &Path) -> Option<PackageJson> {
    let source = fs::read_to_string(root.join("package.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&source).ok()?;
    Some(PackageJson {
        main: parsed
            .get("main")
            .and_then(|m| m.as_str())
            .map(str::to_string),
    })
}

pub(crate) fn resolve_entry(root: &Path, pkg_main: Option<&str>) -> Option<PathBuf> {
    let mut candidates = vec![];
    if let Some(main) = pkg_main {
        candidates.push(main.to_string());
        // `npm init` defaults main to index.js, so TypeScript carriers routinely
        // point at a file that was never emitted. Try the same name with every
        // supported extension before giving up.
        let base = match main.rfind('.') {
            Some(dot) => &main[..dot],
            None => main,
        };
        candidates.extend(main_extensions().map(|ext| format!("{}.{}", base, ext)));
    } else {
        candidates.extend(main_extensions().map(|ext| format!("index.{}", ext)));
    }
    candidates
        .into_iter()
        .map(|candidate| root.join(candidate))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{create_dir_all, write};
    use tempfile::TempDir;

    fn carrier_dir(files: &[(&str, &str)]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for (path, contents) in files {
            let path = dir.path().join(path);
            create_dir_all(path.parent().unwrap()).unwrap();
            write(path, contents).unwrap();
        }
        dir
    }

    #[test]
    fn index_is_found_in_extension_order() {
        let dir = carrier_dir(&[("index.js", ""), ("index.ts", "")]);
        assert_eq!(
            resolve_entry(dir.path(), None),
            Some(dir.path().join("index.ts")),
            "typescript is preferred, matching the toolchain"
        );
    }

    #[test]
    fn package_main_wins_over_index() {
        let dir = carrier_dir(&[("index.js", ""), ("carrier.js", "")]);
        assert_eq!(
            resolve_entry(dir.path(), Some("carrier.js")),
            Some(dir.path().join("carrier.js"))
        );
    }

    #[test]
    fn a_main_that_was_never_emitted_falls_back_to_its_other_extensions() {
        // `npm init` writes main: "index.js" into a TypeScript carrier.
        let dir = carrier_dir(&[("index.ts", "")]);
        assert_eq!(
            resolve_entry(dir.path(), Some("index.js")),
            Some(dir.path().join("index.ts"))
        );
    }

    #[test]
    fn nothing_resolves_when_no_candidate_exists() {
        let dir = carrier_dir(&[("readme.md", "")]);
        assert_eq!(resolve_entry(dir.path(), None), None);
    }

    #[test]
    fn names_that_could_escape_their_route_are_rejected() {
        assert!(is_valid_name("submit-form"));
        assert!(is_valid_name("a_b.c"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("."));
        assert!(!is_valid_name(".."));
        assert!(!is_valid_name(".hidden"));
        assert!(!is_valid_name("a/b"));
        assert!(!is_valid_name("a\\b"));
        assert!(!is_valid_name("-leading-dash"));
    }

    #[test]
    fn discovery_reads_each_carrier_and_its_main() {
        let dir = carrier_dir(&[
            ("carriers/echo/index.js", ""),
            ("carriers/named/package.json", r#"{"main":"src/main.ts"}"#),
            ("carriers/.hidden/index.js", ""),
            ("carriers/loose-file.js", ""),
        ]);
        let carriers = discover(dir.path()).unwrap();
        assert_eq!(
            carriers.iter().map(|c| &c.name).collect::<Vec<_>>(),
            vec!["echo", "named"],
            "dotted dirs and loose files are not carriers"
        );
        assert!(carriers[0].package.is_none());
        assert_eq!(
            carriers[1].package,
            Some(PackageJson {
                main: Some("src/main.ts".to_string()),
            })
        );
    }

    #[test]
    fn a_site_with_no_carriers_dir_has_no_carriers() {
        let dir = TempDir::new().unwrap();
        assert!(discover(dir.path()).unwrap().is_empty());
    }
}
