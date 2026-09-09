#[cfg(feature = "binary")]
mod binary_tests {
    use std::{
        fs,
        io::Read,
        path::Path,
        process::{Command, Stdio},
        sync, thread,
        time::{Duration, Instant},
    };

    use archival::binary::command::ExitStatus;
    use nanoid::nanoid;
    use tracing_test::traced_test;
    use walkdir::WalkDir;

    fn get_args(args: Vec<&str>) -> impl Iterator<Item = String> {
        let mut a = vec!["archival".to_string()];
        for arg in args {
            a.push(arg.to_string())
        }
        a.into_iter()
    }

    #[test]
    #[traced_test]
    fn build_basics() {
        _ = fs::remove_dir_all("tests/fixtures/website/dist");
        assert!(Path::new("tests/fixtures/website").exists());
        println!(
            "current dir: {}",
            std::env::current_dir().unwrap().display()
        );
        println!(
            r"files: \n{}",
            WalkDir::new(Path::new("tests/fixtures/website"))
                .follow_links(true)
                .into_iter()
                .filter_map(|e| e.ok())
                .map(|de| de.into_path().to_string_lossy().to_string())
                .collect::<Vec<String>>()
                .join("\n")
        );
        archival::binary::binary(
            get_args(vec![
                "build",
                "tests/fixtures/website",
                "--upload-prefix",
                "test",
            ]),
            None,
        )
        .unwrap();
        // The section body contains `{{ section.name }}`, which only resolves
        // if liquid in field values is rendered in place, where the loop
        // variable `section` exists.
        let index = fs::read_to_string("tests/fixtures/website/dist/index.html").unwrap();
        assert!(
            index.contains("about Some Content"),
            "liquid in a field value did not see the enclosing loop variable:\n{index}"
        );
    }

    #[test]
    #[traced_test]
    fn run_watcher() -> anyhow::Result<()> {
        // TODO: spawn a thread and send sigint
        // archival::binary(get_args(vec!["run", "tests/fixtures/website"]))?;
        Ok(())
    }

    #[test]
    #[traced_test]
    fn compatiblity_ok() -> anyhow::Result<()> {
        assert!(matches!(
            archival::binary::binary(get_args(vec!["compat", "0.12.0"]), None)?,
            ExitStatus::Ok
        ));
        Ok(())
    }

    #[test]
    #[traced_test]
    fn compatiblity_not_ok() -> anyhow::Result<()> {
        assert!(matches!(
            archival::binary::binary(get_args(vec!["compat", "0.1.1"]), None)?,
            ExitStatus::Error
        ));
        Ok(())
    }
    #[test]
    #[traced_test]
    fn types_writes_a_declaration_file() -> anyhow::Result<()> {
        let out = "tests/fixtures/website/objects.d.ts";
        _ = fs::remove_file(out);
        assert!(matches!(
            archival::binary::binary(
                get_args(vec![
                    "types",
                    "-o",
                    "objects.d.ts",
                    "tests/fixtures/website"
                ]),
                None
            )?,
            ExitStatus::Ok
        ));
        let defs = fs::read_to_string(out)?;
        assert!(
            defs.contains("export interface ArchivalObjects {"),
            "{}",
            defs
        );
        assert!(defs.contains("section: SectionObject[];"), "{}", defs);
        assert!(defs.contains("body: string | null;"), "{}", defs);
        // Comments in archival_objects.toml reach the generated types as JSDoc.
        assert!(
            defs.contains("/** A block of content on the home page. */"),
            "{}",
            defs
        );
        assert!(
            defs.contains("  /** The section's heading. */\n  name: string | null;"),
            "{}",
            defs
        );
        // A file header separated by a blank line is not a description.
        assert!(
            !defs.contains("This file is used to define"),
            "leading file comment leaked into the types: {}",
            defs
        );
        _ = fs::remove_file(out);
        Ok(())
    }

    static SUBPAGE_CONTENT: &str = r#"
        name="HI"
        "#;
    #[test]
    #[traced_test]
    fn run_removes_files_when_objects_deleted() {
        assert!(Path::new("tests/fixtures/website").exists());
        _ = fs::create_dir("tests/fixtures/tmp");
        let site_path = format!("tests/fixtures/tmp/{}", nanoid!());
        copy_dir_all("tests/fixtures/website", &site_path).unwrap();

        println!(
            "current dir: {}",
            std::env::current_dir().unwrap().display()
        );
        println!(
            r"files: \n{}",
            WalkDir::new(&site_path)
                .follow_links(true)
                .into_iter()
                .filter_map(|e| e.ok())
                .map(|de| de.into_path().to_string_lossy().to_string())
                .collect::<Vec<String>>()
                .join("\n")
        );
        // The already-built binary, not `cargo run`: a nested cargo invocation
        // rebuilds this path with whatever features it defaults to, which other
        // test binaries then run.
        let mut run_cmd = Command::new(env!("CARGO_BIN_EXE_archival"))
            .args(["run", &site_path, "--upload-prefix", "test"])
            .current_dir(std::env::current_dir().unwrap())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stream = run_cmd.stdout.take().unwrap();
        let (sender, receiver) = sync::mpsc::channel();
        thread::spawn(move || loop {
            let mut buf = [0];
            match stream.read(&mut buf) {
                Err(err) => {
                    panic!("{}] Error reading from stream: {}", line!(), err);
                }
                Ok(len) => {
                    if len > 0 {
                        sender.send(buf).expect("send failed");
                    }
                }
            }
        });
        let s = run_until(&receiver, "Serving", Duration::from_millis(60_000));
        println!("-------- initial build: {}", String::from_utf8_lossy(&s));
        // First build complete. Now add a file and make sure it's built
        let spid = nanoid!();
        let new_section_path = format!("{}/objects/subpage/{}.toml", site_path, spid);
        println!("LOOKING FOR NEW PAGE...");
        fs::write(&new_section_path, SUBPAGE_CONTENT).unwrap();
        let s = run_until(&receiver, "Rebuilt", Duration::from_millis(2000));
        println!("-------- new page: {}", String::from_utf8_lossy(&s));
        let new_section_page_path = format!("{}/dist/subpage/{}.html", site_path, spid);
        let found_page = fs::exists(&new_section_page_path).unwrap();
        assert!(found_page);
        // Now remove the object and verify that the page was also removed;
        fs::remove_file(&new_section_path).unwrap();
        let s = run_until(&receiver, "Rebuilt", Duration::from_millis(2000));
        println!("-------- deleted: {}", String::from_utf8_lossy(&s));
        let found_page = fs::exists(&new_section_page_path).unwrap();
        assert!(!found_page);
        run_cmd.kill().unwrap();
        run_cmd.wait().unwrap();
        // _ = fs::remove_dir_all(site_path);
    }

    fn run_until(
        receiver: &sync::mpsc::Receiver<[u8; 1]>,
        until_seen: &str,
        timeout: Duration,
    ) -> Vec<u8> {
        let start = Instant::now();
        let mut current_buf = vec![];
        loop {
            if Instant::now() - start > timeout {
                panic!("timed out waiting for {}", until_seen);
            }
            match receiver.try_recv() {
                Ok(bytes) => {
                    current_buf.append(&mut bytes.to_vec());
                    let str = String::from_utf8_lossy(&current_buf);
                    if str.contains(until_seen) {
                        break;
                    }
                }
                Err(sync::mpsc::TryRecvError::Empty) => {}
                Err(sync::mpsc::TryRecvError::Disconnected) => panic!("Channel disconnected"),
            }
        }
        current_buf
    }

    /// Copies a site's source. `dist` is skipped because `build_basics` rebuilds the
    /// shared fixture's copy of it concurrently, and walking a tree another test is
    /// deleting from fails on whichever file it removes first.
    fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
        fs::create_dir_all(&dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            if entry.file_name() == "dist" {
                continue;
            }
            let ty = entry.file_type()?;
            if ty.is_dir() {
                copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
            } else {
                fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
            }
        }
        Ok(())
    }
}
