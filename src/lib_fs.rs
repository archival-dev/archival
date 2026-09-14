use crate::{Archival, ArchivalError, FileSystemAPI};
use anyhow::Result;
use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

/// This API is primarily intended for direct modifications to site files,
/// rather than changing objects, which should always be done via archival events.
impl<F: FileSystemAPI + Clone + Debug> Archival<F> {
    pub fn fs_list_files(
        &self,
        dir: impl AsRef<Path>,
        recursive: bool,
        ignore: Option<Vec<&str>>,
    ) -> Result<Vec<PathBuf>> {
        let ignores = ignore
            .map(|ignore| {
                let mut ignores = ignore::gitignore::GitignoreBuilder::new(Path::new(""));
                for line in ignore {
                    ignores.add_line(None, line)?;
                }
                ignores.build()
            })
            .transpose()?;
        Ok(self
            .fs_mutex
            .with_fs(|fs| fs.list_dir(dir, recursive))?
            .filter(|ent| {
                ignores.as_ref().is_none_or(|ignores| {
                    matches!(
                        ignores.matched(ent.as_path(), ent.is_dir()),
                        ignore::Match::None
                    )
                })
            })
            .collect())
    }
    pub fn fs_read_file(&self, path: impl AsRef<Path>) -> Result<String> {
        self.fs_mutex.with_fs(|fs| {
            Ok(fs
                .read_to_string(path)?
                .ok_or_else(|| ArchivalError::new("file not found"))?)
        })
    }
    pub fn fs_read_bytes(&self, path: impl AsRef<Path>) -> Result<Option<Vec<u8>>> {
        self.fs_mutex.with_fs(|fs| match fs.read(path) {
            // NativeFileSystem reports a missing file as an io error rather than None.
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                Ok(None)
            }
            read => read,
        })
    }
    pub fn fs_write_file(&self, path: impl AsRef<Path>, contents: String) -> Result<()> {
        self.fs_mutex.with_fs(|fs| fs.write_str(path, contents))
    }
    pub fn fs_write_bytes(&self, path: impl AsRef<Path>, contents: Vec<u8>) -> Result<()> {
        self.fs_mutex.with_fs(|fs| fs.write(path, contents))
    }
    pub fn fs_delete_file(&self, path: impl AsRef<Path>) -> Result<()> {
        self.fs_mutex.with_fs(|fs| fs.delete(path))
    }
    pub fn fs_rename_file(&self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<()> {
        self.fs_mutex.with_fs(|fs| fs.rename(from, to))
    }
}

#[cfg(all(test, feature = "stdlib-fs"))]
mod lib_fs_stdlib {
    use assertables::*;
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use tempfile::tempdir;

    use crate::{file_system_stdlib::NativeFileSystem, Archival};

    fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
        fs::create_dir_all(&dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let entry_type = entry.file_type()?;
            if entry_type.is_dir() {
                copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
            } else {
                fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
            }
        }
        Ok(())
    }

    fn archival_for_fixture_site() -> anyhow::Result<Archival<NativeFileSystem>> {
        let tmp = tempdir()?;
        let site_root = tmp.keep().join("site");
        copy_dir_all(Path::new("tests/fixtures/website"), &site_root)?;
        let fs = NativeFileSystem::new(&site_root);
        Archival::new_with_upload_prefix(fs, "test")
    }

    #[test]
    fn list_files() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;

        let recursive_files = archival.fs_list_files("/", true, None)?;
        assert_not_contains!(recursive_files, &PathBuf::from(""));
        assert_not_contains!(recursive_files, &PathBuf::from("/"));
        assert_contains!(
            recursive_files,
            &PathBuf::from("objects/subpage/hello.toml")
        );

        let shallow_files = archival.fs_list_files("public", false, None)?;
        assert_not_contains!(shallow_files, &PathBuf::from("public/style/theme.css"));
        assert_any!(shallow_files.iter(), |p: &PathBuf| p
            .file_name()
            .is_some_and(|f| f == "style"));

        let contents = archival.fs_read_file("objects/subpage/hello.toml")?;
        assert_eq!(contents.trim(), "name = \"hello\"");

        Ok(())
    }

    #[test]
    fn gitignored_list_files() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;

        // Ignore liquid templates, json files, and everything under objects/section/
        let files = archival.fs_list_files(
            "/",
            true,
            Some(vec!["*.liquid", "*.json", "objects/section/*"]),
        )?;

        // *.liquid pattern should exclude all page templates
        assert_not_contains!(files, &PathBuf::from("pages/index.liquid"));
        assert_not_contains!(files, &PathBuf::from("pages/subpage.liquid"));
        assert_not_contains!(files, &PathBuf::from("pages/404.liquid"));

        // *.json pattern should exclude json files
        assert_not_contains!(files, &PathBuf::from("package.json"));

        // objects/section/* should exclude files directly under that directory
        assert_not_contains!(files, &PathBuf::from("objects/section/first.toml"));

        // files not matching any pattern should still be present
        assert_contains!(files, &PathBuf::from("objects/subpage/hello.toml"));
        assert_contains!(files, &PathBuf::from("public/robots.txt"));

        Ok(())
    }

    #[test]
    fn read_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let contents = archival.fs_read_file("objects/subpage/hello.toml")?;
        assert_eq!(contents.trim(), "name = \"hello\"");
        let abs_contents = archival.fs_read_file("/objects/subpage/hello.toml")?;
        assert_eq!(abs_contents, contents);
        Ok(())
    }

    #[test]
    fn write_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let path = Path::new("objects/subpage/fs_api_write_file.toml");
        archival.fs_write_file(path, "name = \"write_file\"\n".to_string())?;
        let written = archival.fs_read_file(path)?;
        assert_eq!(written, "name = \"write_file\"\n");
        Ok(())
    }

    #[test]
    fn write_then_read_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let path = Path::new("objects/subpage/fs_api_write_then_read_file.toml");
        let contents = "name = \"write_then_read_file\"\n".to_string();
        archival.fs_write_file(path, contents.clone())?;
        assert_eq!(archival.fs_read_file(path)?, contents);
        Ok(())
    }

    #[test]
    fn delete_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let existing = Path::new("objects/subpage/hello.toml");
        archival.fs_delete_file(existing)?;
        assert!(archival.fs_read_file(existing).is_err());
        Ok(())
    }

    #[test]
    fn write_file_then_delete_new_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let path = Path::new("objects/subpage/fs_api_delete_new_file.toml");
        archival.fs_write_file(path, "name = \"delete_new_file\"\n".to_string())?;
        archival.fs_delete_file(path)?;
        assert!(archival.fs_read_file(path).is_err());
        Ok(())
    }

    #[test]
    fn rename_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let from = Path::new("objects/subpage/hello.toml");
        let to = Path::new("objects/subpage/hello_renamed.toml");
        archival.fs_rename_file(from, to)?;
        assert!(archival.fs_read_file(from).is_err());
        assert_eq!(archival.fs_read_file(to)?.trim(), "name = \"hello\"");
        Ok(())
    }

    #[test]
    fn write_then_rename_then_read_new_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;

        let original_path = Path::new("objects/subpage/fs_api_rename_source.toml");
        let renamed_path = Path::new("objects/subpage/fs_api_rename_target.toml");
        let contents = "name = \"write_then_rename_then_read_new_file\"\n".to_string();

        archival.fs_write_file(original_path, contents.clone())?;
        archival.fs_rename_file(original_path, renamed_path)?;
        assert!(archival.fs_read_file(original_path).is_err());
        assert_eq!(archival.fs_read_file(renamed_path)?, contents);

        Ok(())
    }

    #[test]
    fn read_bytes_of_binary_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let expected = fs::read("tests/fixtures/website/public/icon.png")?;
        assert_eq!(archival.fs_read_bytes("public/icon.png")?, Some(expected));
        assert!(archival.fs_read_file("public/icon.png").is_err());
        assert_eq!(archival.fs_read_bytes("public/missing.png")?, None);
        Ok(())
    }

    #[test]
    fn write_then_rename_then_read_bytes() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let original_path = Path::new("public/fonts/fs_api_source.woff2");
        let renamed_path = Path::new("public/fonts/fs_api_target.woff2");
        let contents = vec![0x77, 0x4f, 0x46, 0x32, 0xff, 0xfe, 0x00, 0x80];

        archival.fs_write_bytes(original_path, contents.clone())?;
        assert_eq!(
            archival.fs_read_bytes(original_path)?,
            Some(contents.clone())
        );
        assert!(archival.fs_read_file(original_path).is_err());
        archival.fs_rename_file(original_path, renamed_path)?;
        assert_eq!(archival.fs_read_bytes(original_path)?, None);
        assert_eq!(archival.fs_read_bytes(renamed_path)?, Some(contents));

        Ok(())
    }
}

#[cfg(test)]
mod lib_fs_memory {
    use assertables::*;
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use crate::{Archival, BuildOptions, FileSystemAPI, MemoryFileSystem};

    fn load_fixture_site_into_memory(
        fs_mem: &mut MemoryFileSystem,
        src_root: &Path,
        rel: &Path,
    ) -> anyhow::Result<()> {
        let abs = src_root.join(rel);
        for entry in fs::read_dir(abs)? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let next_rel = rel.join(entry.file_name());
            if ty.is_dir() {
                load_fixture_site_into_memory(fs_mem, src_root, &next_rel)?;
            } else {
                let bytes = fs::read(entry.path())?;
                fs_mem.write(&next_rel, bytes)?;
            }
        }
        Ok(())
    }

    fn archival_for_fixture_site() -> anyhow::Result<Archival<MemoryFileSystem>> {
        let mut fs_mem = MemoryFileSystem::default();
        load_fixture_site_into_memory(
            &mut fs_mem,
            Path::new("tests/fixtures/website"),
            Path::new(""),
        )?;
        println!("FIXTURE FS: {:#?}", fs_mem);
        Archival::new_with_upload_prefix(fs_mem, "test")
    }

    #[test]
    fn list_files() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;

        let recursive_files = archival.fs_list_files("/", true, None)?;
        assert_contains!(
            recursive_files,
            &PathBuf::from("objects/subpage/hello.toml")
        );

        let shallow_files = archival.fs_list_files("public", false, None)?;
        assert_not_contains!(shallow_files, &PathBuf::from("public/style/theme.css"));
        assert_any!(shallow_files.iter(), |p: &PathBuf| p
            .file_name()
            .is_some_and(|f| f == "style"));

        let contents = archival.fs_read_file("objects/subpage/hello.toml")?;
        assert_eq!(contents.trim(), "name = \"hello\"");

        Ok(())
    }

    #[test]
    fn gitignored_list_files() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;

        // Ignore liquid templates, json files, and everything under objects/section/
        let files = archival.fs_list_files(
            "/",
            true,
            Some(vec!["*.liquid", "*.json", "objects/section/*"]),
        )?;

        // *.liquid pattern should exclude all page templates
        assert_not_contains!(files, &PathBuf::from("pages/index.liquid"));
        assert_not_contains!(files, &PathBuf::from("pages/subpage.liquid"));
        assert_not_contains!(files, &PathBuf::from("pages/404.liquid"));

        // *.json pattern should exclude json files
        assert_not_contains!(files, &PathBuf::from("package.json"));

        // objects/section/* should exclude files directly under that directory
        assert_not_contains!(files, &PathBuf::from("objects/section/first.toml"));

        // files not matching any pattern should still be present
        assert_contains!(files, &PathBuf::from("objects/subpage/hello.toml"));
        assert_contains!(files, &PathBuf::from("public/robots.txt"));

        Ok(())
    }

    #[test]
    fn read_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let contents = archival.fs_read_file("objects/subpage/hello.toml")?;
        assert_eq!(contents.trim(), "name = \"hello\"");
        let abs_contents = archival.fs_read_file("/objects/subpage/hello.toml")?;
        assert_eq!(abs_contents, contents);
        Ok(())
    }

    #[test]
    fn write_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let path = Path::new("objects/subpage/fs_api_write_file.toml");
        archival.fs_write_file(path, "name = \"write_file\"\n".to_string())?;
        let written = archival.fs_read_file(path)?;
        assert_eq!(written, "name = \"write_file\"\n");
        Ok(())
    }

    #[test]
    fn write_then_read_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let path = Path::new("objects/subpage/fs_api_write_then_read_file.toml");
        let contents = "name = \"write_then_read_file\"\n".to_string();
        archival.fs_write_file(path, contents.clone())?;
        assert_eq!(archival.fs_read_file(path)?, contents);
        Ok(())
    }

    #[test]
    fn delete_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let existing = Path::new("objects/subpage/hello.toml");
        archival.fs_delete_file(existing)?;
        assert!(archival.fs_read_file(existing).is_err());
        Ok(())
    }

    #[test]
    fn write_file_then_delete_new_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let path = Path::new("objects/subpage/fs_api_delete_new_file.toml");
        archival.fs_write_file(path, "name = \"delete_new_file\"\n".to_string())?;
        archival.fs_delete_file(path)?;
        assert!(archival.fs_read_file(path).is_err());
        Ok(())
    }

    #[test]
    fn rename_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let from = Path::new("objects/subpage/hello.toml");
        let to = Path::new("objects/subpage/hello_renamed.toml");
        archival.fs_rename_file(from, to)?;
        assert!(archival.fs_read_file(from).is_err());
        assert_eq!(archival.fs_read_file(to)?.trim(), "name = \"hello\"");
        Ok(())
    }

    #[test]
    fn write_then_rename_then_read_new_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;

        let original_path = Path::new("objects/subpage/fs_api_rename_source.toml");
        let renamed_path = Path::new("objects/subpage/fs_api_rename_target.toml");
        let contents = "name = \"write_then_rename_then_read_new_file\"\n".to_string();

        archival.fs_write_file(original_path, contents.clone())?;
        archival.fs_rename_file(original_path, renamed_path)?;
        assert!(archival.fs_read_file(original_path).is_err());
        assert_eq!(archival.fs_read_file(renamed_path)?, contents);

        Ok(())
    }

    #[test]
    fn read_bytes_of_binary_file() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let expected = fs::read("tests/fixtures/website/public/icon.png")?;
        assert_eq!(archival.fs_read_bytes("public/icon.png")?, Some(expected));
        assert!(archival.fs_read_file("public/icon.png").is_err());
        assert_eq!(archival.fs_read_bytes("public/missing.png")?, None);
        Ok(())
    }

    #[test]
    fn write_then_rename_then_read_bytes() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let original_path = Path::new("public/fonts/fs_api_source.woff2");
        let renamed_path = Path::new("public/fonts/fs_api_target.woff2");
        let contents = vec![0x77, 0x4f, 0x46, 0x32, 0xff, 0xfe, 0x00, 0x80];

        archival.fs_write_bytes(original_path, contents.clone())?;
        assert_eq!(
            archival.fs_read_bytes(original_path)?,
            Some(contents.clone())
        );
        assert!(archival.fs_read_file(original_path).is_err());
        archival.fs_rename_file(original_path, renamed_path)?;
        assert_eq!(archival.fs_read_bytes(original_path)?, None);
        assert_eq!(archival.fs_read_bytes(renamed_path)?, Some(contents));

        Ok(())
    }

    #[test]
    fn built_static_file_keeps_written_bytes() -> anyhow::Result<()> {
        let archival = archival_for_fixture_site()?;
        let contents = vec![0x77, 0x4f, 0x46, 0x32, 0xff, 0xfe, 0x00, 0x80];
        archival.fs_write_bytes("public/fonts/fs_api_built.woff2", contents.clone())?;

        archival.build(BuildOptions::default())?;

        let built = archival
            .dist_files()
            .into_iter()
            .find(|file| file.path.ends_with("fonts/fs_api_built.woff2"))
            .expect("the static file is in the build");
        assert_eq!(built.data, contents);
        Ok(())
    }
}
