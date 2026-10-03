//! The node sidecar: spawning it, waiting for it, pushing state to it, and
//! making sure it dies with the dev server.

use super::objects::CarrierPayload;
use anyhow::{anyhow, Result};
use nanoid::nanoid;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tracing::warn;

const HARNESS: &str = include_str!("harness.mjs");
const HARNESS_FILE_NAME: &str = "harness.mjs";
/// The carrier build toolchain, vendored by `update-carriers-toolchain.sh`. The
/// harness imports it from beside itself.
const TOOLCHAIN: &str = include_str!("toolchain.mjs");
const TOOLCHAIN_FILE_NAME: &str = "toolchain.mjs";
const HEALTH_TIMEOUT: Duration = Duration::from_secs(10);
const HEALTH_POLL: Duration = Duration::from_millis(20);

/// What the harness is told to serve. `carriers` maps a name to the directory
/// the carrier is built from.
#[derive(Debug, Clone, Serialize)]
struct State<'a> {
    carriers: &'a BTreeMap<String, PathBuf>,
    #[serde(flatten)]
    payload: &'a CarrierPayload,
}

pub(crate) fn harness_dir(site_root: &Path) -> PathBuf {
    if let Some(override_dir) = std::env::var_os("ARCHIVAL_CARRIERS_DIR") {
        return PathBuf::from(override_dir);
    }
    let key = seahash::hash(
        std::fs::canonicalize(site_root)
            .unwrap_or_else(|_| site_root.to_path_buf())
            .to_string_lossy()
            .as_bytes(),
    );
    std::env::temp_dir()
        .join("archival-carriers")
        .join(format!("{:016x}", key))
}

pub(crate) fn write_harness(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(HARNESS_FILE_NAME);
    std::fs::write(&path, HARNESS)?;
    std::fs::write(dir.join(TOOLCHAIN_FILE_NAME), TOOLCHAIN)?;
    Ok(path)
}

pub(crate) struct Sidecar {
    child: Child,
    /// Held open and never written to. The harness exits when it closes, which
    /// is what stops a node process outliving a dev server that was killed.
    _stdin: ChildStdin,
    port: u16,
    token: String,
    client: reqwest::blocking::Client,
}

impl Sidecar {
    pub fn spawn(harness: &Path, extra_args: &[String], port: Option<u16>) -> Result<Self> {
        let token = nanoid!();
        let mut args = extra_args.to_vec();
        args.push(harness.to_string_lossy().into_owned());
        let mut child = Command::new("node")
            .args(&args)
            .env("ARCHIVAL_CARRIER_TOKEN", &token)
            // Unpinned, the harness binds an ephemeral port and reports it
            // back, so there is no window where something else could take the
            // one we picked.
            .env("ARCHIVAL_CARRIER_PORT", port.unwrap_or(0).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| anyhow!("couldn't start the carrier sidecar: {}", e))?;

        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        pump(stderr);
        let port = read_port(stdout)?;

        Ok(Self {
            child,
            _stdin: stdin,
            port,
            token,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(5))
                .no_proxy()
                // Health polling leaves idle connections the sidecar may close
                // before the next push reuses one, which windows reports as a
                // reset rather than a retryable close.
                .pool_max_idle_per_host(0)
                .build()?,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn wait_until_healthy(&self) -> Result<()> {
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        let mut last = None;
        while Instant::now() < deadline {
            match self.control("health").send() {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => last = Some(response.status().to_string()),
                Err(e) => last = Some(e.to_string()),
            }
            thread::sleep(HEALTH_POLL);
        }
        Err(anyhow!(
            "the carrier sidecar never became healthy{}",
            last.map(|l| format!(": {}", l)).unwrap_or_default()
        ))
    }

    pub fn push_state(
        &self,
        carriers: &BTreeMap<String, PathBuf>,
        payload: &CarrierPayload,
    ) -> Result<()> {
        let response = self
            .control("state")
            .json(&State { carriers, payload })
            .send()?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "the carrier sidecar rejected the site's objects: {}",
                response.status()
            ));
        }
        Ok(())
    }

    fn control(&self, route: &str) -> reqwest::blocking::RequestBuilder {
        self.client
            .post(format!(
                "http://127.0.0.1:{}/__control/{}",
                self.port, route
            ))
            .header("x-archival-token", &self.token)
    }

    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        // Reap it, so a restart doesn't leave a zombie behind.
        let _ = self.child.wait();
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The harness announces its port on the first line of stdout; everything after
/// that is carrier logging.
fn read_port(stdout: impl Read + Send + 'static) -> Result<u16> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let port = serde_json::from_str::<serde_json::Value>(&line)
        .ok()
        .and_then(|v| v["archivalCarrier"]["port"].as_u64())
        .ok_or_else(|| {
            anyhow!(
                "the carrier sidecar didn't report a port (it said: {})",
                line.trim()
            )
        })?;
    pump(reader);
    u16::try_from(port).map_err(|_| anyhow!("the carrier sidecar reported port {}", port))
}

/// Carrier `console.log` is how an author debugs, so it has to reach the
/// terminal rather than sitting in a pipe.
fn pump(stream: impl Read + Send + 'static) {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            match line {
                Ok(line) => println!("[carrier] {}", line),
                Err(e) => {
                    warn!("lost the carrier sidecar's output: {}", e);
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn the_harness_is_written_outside_the_site() {
        let site = TempDir::new().unwrap();
        let dir = harness_dir(site.path());
        assert!(
            !dir.starts_with(site.path()),
            "generated files must not land in the user's repo: {}",
            dir.display()
        );
    }

    #[test]
    fn the_harness_dir_is_stable_per_site() {
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        assert_eq!(harness_dir(a.path()), harness_dir(a.path()));
        assert_ne!(harness_dir(a.path()), harness_dir(b.path()));
    }

    #[test]
    fn the_harness_is_written_verbatim() {
        let dir = TempDir::new().unwrap();
        let path = write_harness(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), HARNESS);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(TOOLCHAIN_FILE_NAME)).unwrap(),
            TOOLCHAIN,
            "the harness imports the toolchain from beside itself"
        );
    }
}
