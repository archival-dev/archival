use anyhow::Result;
use std::{
    io::{Cursor, Read, Seek},
    path::{Path, PathBuf},
};
#[cfg(feature = "verbose-logging")]
use tracing::debug;

pub trait FileSystemAPI: Send + Sync + Default {
    /// A point-in-time copy of this filesystem, or `None` when the backend cannot make
    /// one. `Clone` is not that guarantee: a filesystem backed by a real directory
    /// clones into another handle on the same live tree, so a build computed against it
    /// would still be reading what it is writing.
    fn snapshot(&self) -> Option<Self>
    where
        Self: Sized;
    fn root_dir(&self) -> &Path;
    fn exists(&self, path: impl AsRef<Path>) -> Result<bool>;
    fn is_dir(&self, path: impl AsRef<Path>) -> Result<bool>;
    fn remove_dir_all(&mut self, path: impl AsRef<Path>) -> Result<()>;
    fn create_dir_all(&mut self, path: impl AsRef<Path>) -> Result<()>;
    fn read(&self, path: impl AsRef<Path>) -> Result<Option<Vec<u8>>>;
    fn read_to_string(&self, path: impl AsRef<Path>) -> Result<Option<String>>;
    fn delete(&mut self, path: impl AsRef<Path>) -> Result<()>;
    fn write(&mut self, path: impl AsRef<Path>, contents: Vec<u8>) -> Result<()>;
    fn rename(&mut self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()>;
    fn write_str(&mut self, path: impl AsRef<Path>, contents: String) -> Result<()>;
    fn list_dir(
        &self,
        path: impl AsRef<Path>,
        recursive: bool,
    ) -> Result<Box<dyn Iterator<Item = PathBuf>>>;
    fn walk_dir(
        &self,
        path: impl AsRef<Path>,
        include_dirs: bool,
    ) -> Result<Box<dyn Iterator<Item = PathBuf>>>;
}

// Orphan rules prevent this blanket impl so it must be implemented for each
// struct individually
// impl<T> std::fmt::Display for T
// where
//     T: FileSystemAPI,
// {
//     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//         match self.walk_dir("", false) {
//             Ok(paths) => {
//                 write!(
//                     f,
//                     "{}:\n\t{}",
//                     self.root_dir().display(),
//                     paths
//                         .map(|p| p.display().to_string())
//                         .collect::<Vec<_>>()
//                         .join("\n\t")
//                 )
//             }
//             Err(e) => write!(f, "{}: {}", self.root_dir().display(), e),
//         }
//     }
// }

#[cfg(feature = "binary")]
pub trait WatchableFileSystemAPI {
    fn watch(
        &self,
        root: PathBuf,
        watch_paths: Vec<String>,
        changed: impl Fn(Vec<PathBuf>) + Send + Sync + 'static,
    ) -> Result<Box<dyn FnOnce() + '_>>;
}

fn has_toplevel<S: Read + Seek>(
    archive: &mut zip::ZipArchive<S>,
) -> Result<bool, zip::result::ZipError> {
    let mut toplevel_dir: Option<PathBuf> = None;
    if archive.len() < 2 {
        return Ok(false);
    }

    for i in 0..archive.len() {
        let file = archive.by_index(i)?.mangled_name();
        if let Some(toplevel_dir) = &toplevel_dir {
            if !file.starts_with(toplevel_dir) {
                return Ok(false);
            }
        } else {
            // First iteration
            let comp: PathBuf = file.components().take(1).collect();
            toplevel_dir = Some(comp);
        }
    }
    Ok(true)
}

pub fn unpack_zip(zipball: Vec<u8>, fs: &mut impl FileSystemAPI) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zipball))?;

    let do_strip_toplevel = has_toplevel(&mut archive)?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let mut relative_path = file.mangled_name();

        if do_strip_toplevel {
            let base = relative_path
                .components()
                .take(1)
                .fold(PathBuf::new(), |mut p, c| {
                    p.push(c);
                    p
                });
            relative_path = relative_path.strip_prefix(&base)?.to_path_buf()
        }

        if relative_path.to_string_lossy().is_empty() {
            // Top-level directory
            continue;
        }

        let mut outpath = PathBuf::new();
        outpath.push(relative_path);

        #[cfg(feature = "verbose-logging")]
        debug!("create {}", outpath.display());

        if file.name().ends_with('/') {
            fs.create_dir_all(&outpath)?;
        } else {
            if let Some(p) = outpath.parent() {
                if !fs.exists(p)? {
                    #[cfg(feature = "verbose-logging")]
                    debug!("create {}", p.display());
                    fs.create_dir_all(p)?;
                }
            }
            let mut buffer = vec![];
            file.read_to_end(&mut buffer)?;
            #[cfg(feature = "verbose-logging")]
            debug!("writing file: {}", outpath.display());
            fs.write(&outpath, buffer)?;
        }
    }
    Ok(())
}
