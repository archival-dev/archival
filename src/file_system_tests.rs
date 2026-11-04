mod tests {
    use anyhow::Result;
    use std::path::{Path, PathBuf};

    use crate::{file_system::unpack_zip, test_utils::as_path_str, FileSystemAPI};

    pub fn write_and_read_files(mut fs: impl FileSystemAPI) -> Result<()> {
        let dir = Path::new("some/deep/path");
        let file = dir.join("myfile.txt");
        let content = "foo bar baz".to_string();
        fs.create_dir_all(dir)?;
        println!("write: {}", file.as_path().display());
        fs.write_str(file.as_path(), content.to_owned())?;
        let files = fs.walk_dir(Path::new(""), false)?.collect::<Vec<PathBuf>>();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], file);
        let content_o = fs.read_to_string(file.as_path())?;
        assert!(content_o.is_some());
        assert_eq!(content, content_o.unwrap());
        fs.delete(file.as_path())?;
        let files = fs.walk_dir(Path::new(""), false)?.collect::<Vec<PathBuf>>();
        println!("files: {:?}", files);
        assert_eq!(files.len(), 0);
        let file = dir.join("myfile.bin");
        let content: Vec<u8> = vec![0, 3, 4];
        fs.write(file.as_path(), content.clone())?;
        let content_o = fs.read(file.as_path())?;
        assert_eq!(content_o.unwrap(), content);
        Ok(())
    }

    pub fn missing_files_read_as_none(mut fs: impl FileSystemAPI) -> Result<()> {
        let missing = Path::new("some/deep/path/absent.txt");
        assert!(fs.read(missing)?.is_none());
        assert!(fs.read_to_string(missing)?.is_none());
        fs.create_dir_all(Path::new("some/deep/path"))?;
        assert!(fs.read(missing)?.is_none());
        assert!(fs.read_to_string(missing)?.is_none());
        Ok(())
    }

    pub fn absolute_paths(mut fs: impl FileSystemAPI) -> Result<()> {
        // Write via relative path, read back via absolute path (leading /)
        let rel = Path::new("dir/file.txt");
        let abs = Path::new("/dir/file.txt");
        let content = "hello".to_string();
        fs.write_str(rel, content.clone())?;

        assert!(fs.exists(abs)?, "exists should find absolute path");
        assert!(fs.exists(rel)?, "exists should find relative path");

        let read_abs = fs.read_to_string(abs)?;
        assert_eq!(read_abs.unwrap(), content, "read via absolute path");

        let read_rel = fs.read_to_string(rel)?;
        assert_eq!(read_rel.unwrap(), content, "read via relative path");

        assert!(fs.is_dir(Path::new("/dir"))?, "is_dir via absolute path");
        assert!(fs.is_dir(Path::new("dir"))?, "is_dir via relative path");

        // Write via absolute path, read back via relative path
        let abs2 = Path::new("/dir/file2.txt");
        let rel2 = Path::new("dir/file2.txt");
        let content2 = "world".to_string();
        fs.write_str(abs2, content2.clone())?;
        let read_rel2 = fs.read_to_string(rel2)?;
        assert_eq!(
            read_rel2.unwrap(),
            content2,
            "read relative after abs write"
        );

        // rename via absolute path
        fs.rename(abs, Path::new("/dir/renamed.txt"))?;
        assert!(!fs.exists(rel)?, "old path gone after rename");
        assert!(
            fs.exists(Path::new("dir/renamed.txt"))?,
            "new relative path present after rename"
        );

        // delete via absolute path
        fs.delete(abs2)?;
        assert!(!fs.exists(rel2)?, "relative path gone after abs delete");

        // walk_dir / list_dir normalise the root as well
        fs.write_str(Path::new("/a/b.txt"), "x".to_string())?;
        let walked: Vec<PathBuf> = fs.walk_dir(Path::new("/"), false)?.collect();
        let walked_rel: Vec<PathBuf> = fs.walk_dir(Path::new(""), false)?.collect();
        assert_eq!(
            walked.len(),
            walked_rel.len(),
            "walk_dir('/') and walk_dir('') return same count"
        );

        // remove_dir_all via absolute path
        fs.remove_dir_all(Path::new("/dir"))?;
        assert!(
            !fs.exists(Path::new("dir/renamed.txt"))?,
            "file gone after remove_dir_all via abs path"
        );

        Ok(())
    }

    pub fn unzip_to_fs(mut fs: impl FileSystemAPI) -> Result<()> {
        let zip = include_bytes!("../tests/fixtures/archival-website.zip");
        unpack_zip(zip.to_vec(), &mut fs)?;
        // This fixture is a site using the legacy file names.
        let ob_def = fs.read_to_string(Path::new("objects.toml"))?;
        assert!(ob_def.is_some());
        let files = fs
            .walk_dir(Path::new(""), false)?
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<String>>();
        println!("files: {:?}", files);
        assert!(files.contains(&as_path_str("layout/theme.liquid")));
        assert!(!files.contains(&"layout".to_string()));
        Ok(())
    }
}

macro_rules! gen_test {
    ($test_fn: ident, $fs_impl:expr) => {
        #[test]
        fn $test_fn() -> anyhow::Result<()> {
            tests::$test_fn($fs_impl)?;
            Ok(())
        }
    };
}

mod memory {
    use crate::file_system_memory;

    use super::tests;
    fn get_fs() -> file_system_memory::MemoryFileSystem {
        file_system_memory::MemoryFileSystem::default()
    }
    gen_test!(write_and_read_files, get_fs());
    gen_test!(unzip_to_fs, get_fs());
    gen_test!(absolute_paths, get_fs());
    gen_test!(missing_files_read_as_none, get_fs());
}

#[cfg(feature = "stdlib-fs")]
mod stdlib {
    use rand::{distributions::Alphanumeric, Rng};
    use std::{fs, path::Path};

    use crate::file_system_stdlib;

    use super::tests;
    fn get_fs() -> file_system_stdlib::NativeFileSystem {
        let rand_dir: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(14)
            .map(char::from)
            .collect();
        let dir = format!("./target/file-system-tests/{}", rand_dir);
        fs::create_dir_all(&dir).unwrap();
        println!("DIR: {}", dir);
        file_system_stdlib::NativeFileSystem::new(Path::new(&dir))
    }
    gen_test!(write_and_read_files, get_fs());
    gen_test!(unzip_to_fs, get_fs());
    gen_test!(absolute_paths, get_fs());
    gen_test!(missing_files_read_as_none, get_fs());
}
