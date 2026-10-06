//! End to end coverage for carriers in `archival run`.
//!
//! Everything here needs node, so the whole module skips when it is missing or
//! too old rather than failing a machine that simply doesn't have it.

#![cfg(feature = "carriers")]

mod carrier_tests {
    use nanoid::nanoid;
    use std::{
        fs,
        io::{BufRead, BufReader},
        net::TcpListener,
        path::{Path, PathBuf},
        process::{Child, Command, Stdio},
        sync::{Arc, Mutex},
        thread,
        time::{Duration, Instant},
    };

    const FIXTURE: &str = "tests/fixtures/carriers-site";
    const READY_TIMEOUT: Duration = Duration::from_secs(60);

    /// Below the Node `archival run` accepts for carriers, or with no node at
    /// all, there is nothing to test.
    fn node_ok() -> bool {
        let Ok(output) = Command::new("node").arg("--version").output() else {
            println!("skipping: `node` is not on PATH");
            return false;
        };
        let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let version: Vec<u64> = printed
            .trim_start_matches('v')
            .split('.')
            .filter_map(|part| part.parse().ok())
            .collect();
        if version.as_slice() < [22, 13].as_slice() {
            println!("skipping: node {} is older than 22.13", printed);
            return false;
        }
        true
    }

    /// Turns "nothing ever answered" into a diagnosis: the binary under test is
    /// shared, and anything that rebuilds it without this feature makes every
    /// carrier route a plain 404.
    fn assert_binary_has_carriers() {
        let help = Command::new(env!("CARGO_BIN_EXE_archival"))
            .args(["run", "--help"])
            .output()
            .expect("archival --help runs");
        let help = String::from_utf8_lossy(&help.stdout);
        assert!(
            help.contains("--no-carriers"),
            "{} was built without the carriers feature",
            env!("CARGO_BIN_EXE_archival")
        );
    }

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
        fs::create_dir_all(&dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let to = dst.as_ref().join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir_all(entry.path(), to)?;
            } else {
                fs::copy(entry.path(), to)?;
            }
        }
        Ok(())
    }

    /// A copy of the fixture site, so a test never writes into the checked-in one.
    fn site_copy() -> PathBuf {
        assert!(Path::new(FIXTURE).exists(), "missing fixture {}", FIXTURE);
        let _ = fs::create_dir("tests/fixtures/tmp");
        let path = PathBuf::from(format!("tests/fixtures/tmp/{}", nanoid!()));
        copy_dir_all(FIXTURE, &path).unwrap();
        path
    }

    struct DevServer {
        child: Child,
        port: u16,
        root: PathBuf,
        output: Arc<Mutex<Vec<String>>>,
    }

    impl DevServer {
        fn start(root: PathBuf) -> Self {
            let server = Self::spawn(root);
            server.wait_until_ready();
            server
        }

        fn spawn(root: PathBuf) -> Self {
            assert_binary_has_carriers();
            let port = free_port();
            let mut child = Command::new(env!("CARGO_BIN_EXE_archival"))
                .args(["run", &root.to_string_lossy(), "--port", &port.to_string()])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("archival run starts");
            let output = Arc::new(Mutex::new(vec![]));
            // Otherwise a full pipe buffer stalls the dev server mid-test.
            for stream in [
                Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
                Box::new(child.stderr.take().unwrap()),
            ] {
                let collected = output.clone();
                thread::spawn(move || {
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        println!("[archival] {}", line);
                        collected.lock().unwrap().push(line);
                    }
                });
            }
            Self {
                child,
                port,
                root,
                output,
            }
        }

        fn wait_until_listening(&self) {
            let deadline = Instant::now() + READY_TIMEOUT;
            while Self::client().get(self.url("/")).send().is_err() {
                assert!(Instant::now() < deadline, "the dev server never listened");
                thread::sleep(Duration::from_millis(100));
            }
        }

        /// How many times the sidecar has come up. More than one without an
        /// edit means something restarted it for no reason.
        fn starts(&self) -> usize {
            self.output
                .lock()
                .unwrap()
                .iter()
                .filter(|line| line.contains("Carriers ready"))
                .count()
        }

        fn url(&self, path: &str) -> String {
            format!("http://localhost:{}{}", self.port, path)
        }

        fn client() -> reqwest::blocking::Client {
            reqwest::blocking::Client::builder()
                // A carrier's "redirect:" return is a 302 the caller must see.
                .redirect(reqwest::redirect::Policy::none())
                // Reusing a keep-alive connection the server has since closed
                // surfaces as a reset on windows rather than a clean retry.
                .pool_max_idle_per_host(0)
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap()
        }

        fn get(&self, path: &str) -> reqwest::blocking::Response {
            Self::client().get(self.url(path)).send().unwrap()
        }

        /// A carrier that failed answers with the reason in plain text, so a
        /// bare `.json()` on it reports a decode error rather than the problem.
        fn json(response: reqwest::blocking::Response) -> serde_json::Value {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            serde_json::from_str(&body)
                .unwrap_or_else(|_| panic!("expected json, got [{}] {}", status, body))
        }

        fn wait_until_ready(&self) {
            let deadline = Instant::now() + READY_TIMEOUT;
            let mut last = String::new();
            while Instant::now() < deadline {
                match Self::client().get(self.url("/carriers/echo")).send() {
                    Ok(response) if response.status().is_success() => return,
                    Ok(response) => last = response.status().to_string(),
                    Err(e) => last = e.to_string(),
                }
                thread::sleep(Duration::from_millis(100));
            }
            panic!("carriers never came up: {}", last);
        }

        /// Waits for a carrier response to change, since a rebuild is batched
        /// and a restart is not instant.
        fn get_until(&self, path: &str, contains: &str) -> String {
            let deadline = Instant::now() + READY_TIMEOUT;
            let mut last = String::new();
            while Instant::now() < deadline {
                if let Ok(response) = Self::client().get(self.url(path)).send() {
                    last = response.text().unwrap_or_default();
                    if last.contains(contains) {
                        return last;
                    }
                }
                thread::sleep(Duration::from_millis(100));
            }
            panic!("{} never contained {}: {}", path, contains, last);
        }
    }

    impl Drop for DevServer {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn carriers_answer_requests_the_way_a_deployed_one_does() {
        if !node_ok() {
            return;
        }
        let server = DevServer::start(site_copy());
        let client = DevServer::client();

        let response = server.get("/carriers/echo?name=tormenta");
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()["content-type"],
            "application/json",
            "an object return is sent as json"
        );
        let body = DevServer::json(response);
        assert_eq!(body["name"], "tormenta");
        assert_eq!(body["body"], serde_json::Value::Null, "a GET has no body");
        assert_eq!(body["artist"], "Tormenta Rey", "objects reach the carrier");
        assert_eq!(body["site"], server.url(""), "SITE_URL is the dev server");

        let body = DevServer::json(
            client
                .post(server.url("/carriers/echo"))
                .json(&serde_json::json!({ "hello": "world" }))
                .send()
                .unwrap(),
        );
        assert_eq!(body["body"]["hello"], "world", "json bodies are parsed");

        let body = DevServer::json(
            client
                .post(server.url("/carriers/echo"))
                .form(&[("a", "1"), ("b", "2")])
                .send()
                .unwrap(),
        );
        assert_eq!(body["body"]["a"], "1", "form fields arrive as strings");

        let response = server.get("/carriers/redir");
        assert_eq!(response.status(), 302, "a `redirect:` return is a 302");
        assert_eq!(response.headers()["location"], "/login");

        let response = server.get("/carriers/boom");
        assert_eq!(response.status(), 500);
        assert!(
            response
                .text()
                .unwrap()
                .starts_with("Carrier threw an error:"),
            "a throw is reported, not swallowed"
        );

        assert_eq!(server.get("/carriers/nope").status(), 404);
        assert_eq!(
            server.get("/carriers/..%2f__control/state").status(),
            404,
            "nothing can address the sidecar's control plane"
        );

        assert!(
            server
                .get("/index.html")
                .text()
                .unwrap()
                .contains("menu.pdf"),
            "the static server still serves the built site"
        );
    }

    #[test]
    fn a_carrier_is_called_the_way_its_carrier_api_version_says() {
        if !node_ok() {
            return;
        }
        let server = DevServer::start(site_copy());
        let body = DevServer::json(server.get("/carriers/two"));
        assert_eq!(body["artist"], "Tormenta Rey", "objects reach the carrier");
        assert_eq!(body["objects"], false, "objects are the site's own");
        assert_eq!(body["url"], server.url(""), "the site is its own argument");
        assert_eq!(
            body["site"],
            serde_json::json!(["url", "uploads", "email", "sql", "activitypub"])
        );
    }

    #[test]
    fn a_carrier_is_built_the_way_a_deploy_builds_it() {
        if !node_ok() {
            return;
        }
        let server = DevServer::start(site_copy());

        let body = DevServer::json(server.get("/carriers/linked?name=tormenta"));
        assert_eq!(
            body["greeting"], "hello tormenta!",
            "its own files are linked, named with an extension or without"
        );
        assert_eq!(body["count"], 2, "a json file is imported as its value");

        let response = server.get("/carriers/commonjs");
        assert_eq!(response.status(), 500);
        let message = response.text().unwrap();
        assert!(
            message.contains("could not be built") && message.contains("CommonJS"),
            "a commonjs carrier is refused, as a deploy refuses it: {}",
            message
        );

        let response = server.get("/carriers/builtin");
        assert_eq!(response.status(), 500);
        let message = response.text().unwrap();
        assert!(
            message.contains("node:fs") && message.contains("Node built-in"),
            "a node built-in is refused by name: {}",
            message
        );

        assert_eq!(
            server.get("/carriers/echo").status(),
            200,
            "a carrier that does not build leaves the others running"
        );
    }

    #[test]
    fn a_carrier_reads_a_secret_the_built_site_never_sees() {
        if !node_ok() {
            return;
        }
        let server = DevServer::start(site_copy());
        // A TypeScript carrier, so this also covers the toolchain's type stripping.
        let response = server.get("/carriers/typed");
        let status = response.status();
        let body = response.text().unwrap_or_default();
        assert_eq!(
            (status.as_u16(), body.as_str()),
            (200, "hunter2"),
            "carriers run on the server and read the real secret"
        );

        let built = fs::read_to_string(server.root.join("dist/index.html")).unwrap();
        assert!(
            !built.contains("hunter2"),
            "the same field must not reach the built site: {}",
            built
        );
    }

    #[test]
    fn editing_an_object_reaches_carriers_without_a_restart() {
        if !node_ok() {
            return;
        }
        let server = DevServer::start(site_copy());
        let object = server.root.join("objects/artist/tormenta-rey.toml");
        fs::write(&object, "name = \"Renamed Artist\"\n").unwrap();
        let body = server.get_until("/carriers/echo", "Renamed Artist");
        assert!(body.contains("Renamed Artist"), "{}", body);
    }

    #[test]
    fn reading_carriers_does_not_restart_the_sidecar() {
        if !node_ok() {
            return;
        }
        // The sidecar reads each carrier as it imports it, and windows reports
        // that read as a change to the carrier's directory. Acting on it
        // restarts the sidecar out from under the request that caused it.
        let server = DevServer::start(site_copy());
        // Carriers that answer without reaching anything outside the process.
        for path in ["/carriers/echo", "/carriers/typed", "/carriers/redir"] {
            let response = server.get(path);
            assert!(
                !response.status().is_server_error(),
                "{} failed: [{}] {}",
                path,
                response.status(),
                response.text().unwrap_or_default()
            );
        }
        // Long enough for a change to have been batched and acted on.
        thread::sleep(Duration::from_millis(500));
        assert_eq!(
            server.starts(),
            1,
            "nothing was edited, so the sidecar should have started once"
        );
    }

    #[test]
    fn a_reload_does_not_break_requests_in_flight() {
        if !node_ok() {
            return;
        }
        // Restarting the sidecar swaps the port every carrier request is sent
        // to. A request that arrives mid-restart must wait for the replacement
        // rather than be sent to the process that just died.
        let server = DevServer::start(site_copy());
        let entry = server.root.join("carriers/echo/index.js");
        for round in 0..3 {
            fs::write(
                &entry,
                format!("export default () => ({{ round: {} }});\n", round),
            )
            .unwrap();
            for _ in 0..10 {
                let response = server.get("/carriers/echo");
                assert!(
                    !response.status().is_server_error(),
                    "a reload must not fail a request: [{}] {}",
                    response.status(),
                    response.text().unwrap_or_default()
                );
                thread::sleep(Duration::from_millis(25));
            }
        }
    }

    #[test]
    fn editing_a_carrier_reloads_it() {
        if !node_ok() {
            return;
        }
        let server = DevServer::start(site_copy());
        let entry = server.root.join("carriers/echo/index.js");
        fs::write(&entry, "export default () => ({ reloaded: true });\n").unwrap();
        let body = server.get_until("/carriers/echo", "reloaded");
        assert!(body.contains("\"reloaded\":true"), "{}", body);
    }

    #[test]
    fn a_carrier_added_to_a_site_without_carriers_is_served() {
        if !node_ok() {
            return;
        }
        let root = site_copy();
        fs::remove_dir_all(root.join("carriers")).unwrap();
        let server = DevServer::spawn(root);
        server.wait_until_listening();
        assert_eq!(server.get("/carriers/late").status(), 404);
        assert_eq!(server.starts(), 0, "no carriers means no sidecar");

        let carrier = server.root.join("carriers/late");
        fs::create_dir_all(&carrier).unwrap();
        fs::write(
            carrier.join("index.ts"),
            "export default (): { late: boolean } => ({ late: true });\n",
        )
        .unwrap();
        server.get_until("/carriers/late", "\"late\":true");
    }

    #[test]
    fn site_sql_keeps_its_rows_in_the_build_directory_across_restarts() {
        if !node_ok() {
            return;
        }
        let server = DevServer::start(site_copy());
        let client = DevServer::client();
        let visit = || {
            DevServer::json(
                client
                    .post(server.url("/carriers/tally"))
                    .json(&serde_json::json!({}))
                    .send()
                    .unwrap(),
            )
        };
        assert_eq!(visit()["count"], 1);
        assert_eq!(visit()["count"], 2);
        assert!(server.root.join("dist/site.sqlite").is_file());

        let entry = server.root.join("carriers/tally/index.js");
        let source = fs::read_to_string(&entry).unwrap();
        fs::write(
            &entry,
            source.replace("return { count };", "return { count, restarted: true };"),
        )
        .unwrap();
        let body: serde_json::Value =
            serde_json::from_str(&server.get_until("/carriers/tally", "restarted")).unwrap();
        assert_eq!(body["count"], 2, "a new sidecar opens the same database");
    }

    #[test]
    fn uploads_are_readable_over_http() {
        if !node_ok() {
            return;
        }
        // Stands in for the uploads CDN a deployed carrier reads through R2.
        let uploads = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let uploads_url = format!("http://{}", uploads.server_addr());
        thread::spawn(move || {
            for request in uploads.incoming_requests() {
                let found = request.url().contains("menu.pdf");
                let response = if found {
                    tiny_http::Response::from_string("a menu").with_status_code(200)
                } else {
                    tiny_http::Response::from_string("nope").with_status_code(404)
                };
                let _ = request.respond(response);
            }
        });

        let root = site_copy();
        let manifest = root.join("archival.toml");
        let existing = fs::read_to_string(&manifest).unwrap();
        fs::write(
            &manifest,
            format!("{}uploads_url = \"{}\"\n", existing, uploads_url),
        )
        .unwrap();

        let server = DevServer::start(root);
        let body = DevServer::json(server.get("/carriers/upload"));
        assert_eq!(
            body["list"][0]["filename"], "menu.pdf",
            "list() is derived from the files objects point at"
        );
        assert_eq!(
            body["byValue"], "carriers-test/0000000000000000000000000000000000000000000000000000000000000001/menu.pdf",
            "a file value off objects carries its own sha"
        );
        assert_eq!(
            body["contents"], "a menu",
            "the upload's bytes are readable"
        );
    }
}
