use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    hash::Hash,
    ops::Deref,
    path::{Path, PathBuf, MAIN_SEPARATOR},
};
#[cfg(feature = "verbose-logging")]
use tracing::debug;
use tracing::error;

use crate::{util::path_to_native, ArchivalError};

use super::FileSystemAPI;

#[derive(Debug, Clone, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    path: PathBuf,
    is_file: bool,
}

impl PartialOrd for DirEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DirEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.path.cmp(&other.path)
    }
}

impl PartialEq for DirEntry {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl Hash for DirEntry {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.path.hash(state);
    }
}

impl DirEntry {
    fn new(path: impl AsRef<Path>, is_file: bool) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            is_file,
        }
    }
    fn copy(&self) -> Self {
        Self {
            path: self.path.to_owned(),
            is_file: self.is_file,
        }
    }
}

impl Deref for DirEntry {
    type Target = PathBuf;
    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileGraphNode {
    path: PathBuf,
    pub(crate) files: BTreeSet<DirEntry>,
}

impl FileGraphNode {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            files: BTreeSet::new(),
        }
    }
    pub fn key(path: impl AsRef<Path>) -> String {
        path.as_ref().to_string_lossy().to_lowercase()
    }
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
    pub fn add(&mut self, path: impl AsRef<Path>, is_file: bool) {
        self.files.insert(DirEntry::new(path, is_file));
    }
    pub fn remove(&mut self, path: impl AsRef<Path>) {
        // Since we impl Ord (which is what BTreeSet uses for comparison),
        // is_file doesn't matter for comparison
        self.files.remove(&DirEntry::new(path, false));
    }
    pub fn copy(&self) -> Self {
        Self {
            path: self.path.to_path_buf(),
            files: self.files.iter().map(|r| r.copy()).collect(),
        }
    }
}

#[derive(Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryFileSystem {
    // Contents and directory nodes are both Arc'd so cloning the filesystem copies
    // pointers rather than data: editors clone it per edit to derive reversible
    // changes, and a build snapshots it. Every write replaces the whole Arc, so a
    // clone never observes a later mutation.
    fs: BTreeMap<String, std::sync::Arc<Vec<u8>>>,
    tree: BTreeMap<String, std::sync::Arc<FileGraphNode>>,
}
impl Debug for MemoryFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct DisplayMemoryFileSystem<'a> {
            files: Vec<&'a String>,
            tree: &'a BTreeMap<String, std::sync::Arc<FileGraphNode>>,
        }
        let Self { fs, tree } = self;

        Debug::fmt(
            &DisplayMemoryFileSystem {
                files: fs.keys().collect(),
                tree,
            },
            f,
        )
    }
}

impl MemoryFileSystem {
    fn normalize(&self, rel: impl AsRef<Path>) -> PathBuf {
        // Paths reach this filesystem both as joined `PathBuf`s (native
        // separators) and as literal strings (`/`). Keys are compared and
        // counted as strings, so they have to agree on one separator: use the
        // platform's, which keeps this filesystem's paths interchangeable with
        // the native one's.
        let rel_path = path_to_native(rel.as_ref());
        let raw = rel_path.to_string_lossy();
        let trimmed = raw.trim_start_matches(MAIN_SEPARATOR);

        // In wasm targets, absolute-path detection can differ from host behavior.
        // Normalize by string shape so both "/" and "" map to the root key.
        if trimmed.is_empty() {
            PathBuf::new()
        } else {
            PathBuf::from(trimmed)
        }
    }
}

impl FileSystemAPI for MemoryFileSystem {
    /// Contents sit behind `Arc`, so this copies pointers rather than bytes, and a
    /// write always replaces the whole `Arc` - a snapshot never observes a later write.
    fn snapshot(&self) -> Option<Self> {
        Some(self.clone())
    }
    fn root_dir(&self) -> &Path {
        Path::new("<memory>")
    }
    fn exists(&self, path: impl AsRef<Path>) -> Result<bool> {
        let path = self.normalize(path);
        if self.fs.contains_key(&path.to_string_lossy().to_lowercase()) || self.is_dir(&path)? {
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn is_dir(&self, path: impl AsRef<Path>) -> Result<bool> {
        Ok(self
            .tree
            .contains_key(&FileGraphNode::key(self.normalize(path))))
    }
    fn remove_dir_all(&mut self, path: impl AsRef<Path>) -> Result<()> {
        self.remove_from_graph(self.normalize(path));
        Ok(())
    }
    fn create_dir_all(&mut self, _path: impl AsRef<Path>) -> Result<()> {
        // dirs are implicitly created when files are created in them
        Ok(())
    }
    fn read(&self, path: impl AsRef<Path>) -> Result<Option<Vec<u8>>> {
        Ok(self.read_file(self.normalize(path)))
    }
    fn read_to_string(&self, path: impl AsRef<Path>) -> Result<Option<String>> {
        if let Some(file) = self.read_file(self.normalize(path)) {
            Ok(Some(std::str::from_utf8(&file)?.to_string()))
        } else {
            Ok(None)
        }
    }
    fn delete(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = self.normalize(path);
        if self.is_dir(&path)? {
            return Err(ArchivalError::new("use remove_dir_all to delete directories").into());
        }
        self.delete_file(path);
        Ok(())
    }
    fn write(&mut self, path: impl AsRef<Path>, contents: Vec<u8>) -> Result<()> {
        let path = self.normalize(path);
        if self.is_dir(&path)? {
            return Err(ArchivalError::new("cannot write to a folder").into());
        }
        self.write_file(path, contents);
        Ok(())
    }
    fn write_str(&mut self, path: impl AsRef<Path>, contents: String) -> Result<()> {
        self.write(path, contents.as_bytes().to_vec())
    }
    fn rename(&mut self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
        let from = self.normalize(from);
        let to = self.normalize(to);
        if let Some(contents) = self.read_file(&from) {
            self.write(to, contents)?;
            self.delete(&from)?;
            Ok(())
        } else {
            Err(ArchivalError::new("file not found").into())
        }
    }
    fn list_dir(
        &self,
        path: impl AsRef<Path>,
        recursive: bool,
    ) -> Result<Box<dyn Iterator<Item = PathBuf>>> {
        if recursive {
            self.walk_dir(path, true)
        } else {
            Ok(Box::new(
                self.all_children(self.normalize(path), recursive, false)
                    .into_iter(),
            ))
        }
    }
    fn walk_dir(
        &self,
        path: impl AsRef<Path>,
        include_dirs: bool,
    ) -> Result<Box<dyn Iterator<Item = PathBuf>>> {
        let normalized = self.normalize(&path);
        let children = self.all_children(&normalized, true, true);
        let mut all_files: Vec<PathBuf> = vec![];
        for child in children {
            let node = self.get_node(&child);
            all_files.append(
                &mut node
                    .files
                    .into_iter()
                    .filter_map(|de| {
                        if !include_dirs && !de.is_file {
                            return None;
                        }
                        match de.path.strip_prefix(&normalized) {
                            Ok(path) => Some(path.to_owned()),
                            Err(e) => {
                                error!(
                                    "Ignorning invalid path {} ({})",
                                    path.as_ref().display(),
                                    e
                                );
                                None
                            }
                        }
                    })
                    .collect(),
            );
        }
        Ok(Box::new(all_files.into_iter()))
    }
}

impl MemoryFileSystem {
    fn write_file(&mut self, path: impl AsRef<Path>, data: Vec<u8>) {
        #[cfg(feature = "verbose-logging")]
        debug!("write: {}", path.as_ref().display());
        self.fs.insert(
            path.as_ref().to_string_lossy().to_lowercase(),
            std::sync::Arc::new(data),
        );
        self.write_to_graph(path, true);
    }

    fn write_to_graph(&mut self, path: impl AsRef<Path>, is_file: bool) {
        // Traverse up the path and add each file to its parent's node
        let mut last_path: PathBuf = PathBuf::new();
        let mut is_file = is_file;
        for ancestor in path.as_ref().ancestors() {
            let a_path = ancestor.to_path_buf();
            // Skip the actual file path, since only directories are nodes
            if a_path.to_string_lossy() != path.as_ref().to_string_lossy() {
                let mut node = self.get_node(&a_path);
                node.add(&last_path, is_file);
                // After we add the first file, everything else will be directories.
                is_file = false;
                self.tree
                    .insert(FileGraphNode::key(&a_path), std::sync::Arc::new(node));
            }
            a_path.clone_into(&mut last_path);
        }
    }

    fn get_node(&self, path: impl AsRef<Path>) -> FileGraphNode {
        match self.tree.get(&FileGraphNode::key(&path)) {
            Some(n) => n.copy(),
            None => FileGraphNode::new(path),
        }
    }

    fn all_children(
        &self,
        path: impl AsRef<Path>,
        recursive: bool,
        include_self: bool,
    ) -> Vec<PathBuf> {
        // All children of this directory will use keys prefixed with this
        // directory's key, but exclude nested descendants.
        let node_key = FileGraphNode::key(&path);
        let parent_separator_count = node_key.matches(MAIN_SEPARATOR).count();
        let direct_child_depth = if node_key.is_empty() {
            0
        } else {
            parent_separator_count + 1
        };
        self.tree
            .keys()
            .chain(self.fs.keys())
            .filter(|k| {
                (include_self || Path::new(k) != path.as_ref())
                    && k.starts_with(&node_key)
                    && (recursive || k.matches(MAIN_SEPARATOR).count() == direct_child_depth)
            })
            .map(PathBuf::from)
            .collect()
    }

    fn remove_from_graph(&mut self, path: impl AsRef<Path>) {
        // If this is a directory, remove it and its children from the graph
        let node = self.get_node(&path);
        if !node.is_empty() {
            for path in self.all_children(&path, true, true) {
                // delete the node and object
                self.tree.remove(&FileGraphNode::key(&path));
                self.delete_file_only(&path);
            }
        }

        let mut last_path = path.as_ref().to_path_buf();
        for ancestor in path.as_ref().ancestors() {
            let a_path = ancestor.to_path_buf();
            if a_path.to_string_lossy() == path.as_ref().to_string_lossy() {
                // We've handled the leaf above
                continue;
            }
            let mut node = self.get_node(&a_path);
            node.remove(&last_path);
            if node.is_empty() {
                // If this is the last item in a node, remove the node entirely.
                self.tree.remove(&FileGraphNode::key(&a_path));
            } else {
                // If the file has siblings, just write the updated value and
                // stop traversing.
                self.tree
                    .insert(FileGraphNode::key(&a_path), std::sync::Arc::new(node));
                break;
            }
            a_path.clone_into(&mut last_path);
        }
    }

    fn delete_file_only(&mut self, path: impl AsRef<Path>) {
        self.fs
            .remove(&path.as_ref().to_string_lossy().to_lowercase());
    }

    fn delete_file(&mut self, path: impl AsRef<Path>) {
        self.delete_file_only(&path);
        self.remove_from_graph(path);
    }

    fn read_file(&self, path: impl AsRef<Path>) -> Option<Vec<u8>> {
        #[cfg(feature = "verbose-logging")]
        debug!("read {}", path.as_ref().display());
        self.fs
            .get(&path.as_ref().to_string_lossy().to_lowercase())
            .map(|v| v.to_vec())
    }
}

impl std::fmt::Display for MemoryFileSystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.walk_dir("", false) {
            Ok(paths) => {
                write!(
                    f,
                    "{}:\n\t{}",
                    self.root_dir().display(),
                    paths
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n\t")
                )
            }
            Err(e) => write!(f, "{}: {}", self.root_dir().display(), e),
        }
    }
}
