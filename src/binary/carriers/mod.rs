//! Server side functions ("carriers") run alongside the dev server.
//!
//! A carrier is a directory under `carriers/` in the site root, served at
//! `/carriers/<name>`. It default-exports `(params, body, objects, site)`
//! (or `(params, body, objects)` for a carrier whose package.json names no
//! carrier API version), where `objects` is the site's object tree with
//! `secret` fields included - the whole point of the feature. `site.sql` is a
//! SQLite file in the build directory, which every carrier shares.

pub(crate) mod discovery;
pub(crate) mod host;
pub(crate) mod node;
pub(crate) mod objects;
pub(crate) mod proxy;

use self::{
    discovery::CARRIERS_DIR_NAME,
    host::Sidecar,
    node::NodeInfo,
    objects::CarrierPayload,
    proxy::{CarrierProxy, ProxyState},
};
use anyhow::Result;
use console::style;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex, RwLock},
    thread,
};
use tracing::{debug, warn};
use walkdir::WalkDir;

const DATABASE_FILE_NAME: &str = "site.sqlite";

/// Tracks what each watched carrier file last contained.
///
/// Tools write into the directory being watched without changing anything - an
/// editor saving a file as it was, an install rewriting a lockfile - and
/// reacting to every write would restart the sidecar each time. Comparing
/// contents never drops a real edit, unlike ignoring changes for a while.
#[derive(Default)]
struct SeenContents(BTreeMap<PathBuf, u64>);

impl SeenContents {
    /// True the first time a path is seen, and whenever what it holds differs
    /// from last time.
    ///
    /// A directory is compared by its listing rather than skipped: watchers
    /// report the directory both when a carrier is added or removed - which
    /// must rebuild - and when a file inside it is merely read, which must not.
    fn changed(&mut self, path: &Path) -> bool {
        let fingerprint = match std::fs::symlink_metadata(path) {
            Err(_) => return self.0.remove(path).is_some(),
            Ok(metadata) if metadata.is_dir() => listing(path),
            Ok(_) => std::fs::read(path).map(|bytes| seahash::hash(&bytes)).ok(),
        };
        match fingerprint {
            Some(hash) => self.0.insert(path.to_path_buf(), hash) != Some(hash),
            // Unreadable right now, so assume an edit rather than lose one.
            None => true,
        }
    }

    /// Records what the sidecar was just built from, so the first event for a
    /// path is compared against that rather than counting as new.
    ///
    /// Windows reports a directory when a file inside it is read, and the
    /// sidecar reads every carrier as it imports them - which without a
    /// baseline looks like a change and restarts it mid-request.
    fn prime(&mut self, site_root: &Path) {
        let carriers = site_root.join(CARRIERS_DIR_NAME);
        for entry in WalkDir::new(&carriers)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                name != "node_modules" && !name.starts_with('.')
            })
            .filter_map(|e| e.ok())
        {
            self.changed(entry.path());
        }
    }
}

fn listing(dir: &Path) -> Option<u64> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name())
        .collect();
    names.sort();
    let mut hash = 0u64;
    for name in names {
        hash = seahash::hash(format!("{}{}", hash, name.to_string_lossy()).as_bytes());
    }
    Some(hash)
}

#[derive(Debug, Clone, Default)]
pub struct CarrierOptions {
    /// `--no-carriers`.
    pub disabled: bool,
    /// Extra node flags, e.g. `--inspect`.
    pub node_args: Vec<String>,
    /// A pinned sidecar port, for attaching a debugger.
    pub port: Option<u16>,
}

enum Message {
    /// Carrier code changed (or this is the first run): install, build, restart.
    Rebuild,
    /// The site's objects changed: push them, without restarting.
    Objects(Box<CarrierPayload>),
}

pub(crate) struct CarrierSupervisor {
    to_worker: mpsc::Sender<Message>,
    state: Arc<RwLock<ProxyState>>,
    sidecar: Arc<Mutex<Option<Sidecar>>>,
    seen: Arc<Mutex<SeenContents>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl CarrierSupervisor {
    /// `None` when carriers were turned off. A site with no carriers yet still
    /// gets one, idle until the first carrier is added.
    pub fn new(site_root: &Path, build_dir: &Path, options: CarrierOptions) -> Option<Self> {
        if options.disabled {
            return None;
        }
        Some(Self::start(
            site_root.to_path_buf(),
            build_dir.join(DATABASE_FILE_NAME),
            options,
        ))
    }

    fn start(site_root: PathBuf, database: PathBuf, options: CarrierOptions) -> Self {
        let state = Arc::new(RwLock::new(ProxyState::Starting));
        let sidecar = Arc::new(Mutex::new(None));
        let mut seen = SeenContents::default();
        // Ahead of the dev server's watcher, which would otherwise see the worker's discovery reads as edits.
        seen.prime(&site_root);
        let seen = Arc::new(Mutex::new(seen));
        let (to_worker, inbox) = mpsc::channel();
        let worker = Worker {
            site_root,
            database,
            options,
            state: state.clone(),
            sidecar: sidecar.clone(),
            seen: seen.clone(),
            roots: BTreeMap::new(),
            payload: None,
        };
        let handle = thread::spawn(move || worker.run(inbox));
        // Everything up to here is cheap; installing and spawning node happens
        // on the worker so the dev server can start serving immediately.
        let supervisor = Self {
            to_worker,
            state,
            sidecar,
            seen,
            worker: Some(handle),
        };
        supervisor.send(Message::Rebuild);
        supervisor
    }

    pub fn is_idle(&self) -> bool {
        matches!(*self.state.read().unwrap(), ProxyState::Idle)
    }

    pub fn proxy(&self) -> Arc<CarrierProxy> {
        Arc::new(CarrierProxy::new(self.state.clone()))
    }

    /// The sidecar, so a ctrl-C handler can kill it before `exit`.
    pub fn child(&self) -> Arc<Mutex<Option<Sidecar>>> {
        self.sidecar.clone()
    }

    /// Whether a changed path is a carrier's concern. Paths under `carriers/`
    /// are always claimed - they must never reach the site's cache invalidation,
    /// or an `npm install` would trigger a full rebuild per written file.
    pub fn claims(&self, changed: &Path) -> bool {
        changed.starts_with(CARRIERS_DIR_NAME)
    }

    /// Whether a claimed path should actually cause a restart. `changed` is
    /// relative to the site root.
    pub fn should_rebuild(&self, root: &Path, changed: &Path) -> bool {
        let ignored = changed.components().any(|c| {
            let name = c.as_os_str().to_string_lossy();
            name == "node_modules" || name.starts_with('.')
        });
        let rebuild = !ignored && self.seen.lock().unwrap().changed(&root.join(changed));
        debug!(
            "carrier change {} -> rebuild={}",
            changed.display(),
            rebuild
        );
        rebuild
    }

    pub fn rebuild(&self) {
        self.send(Message::Rebuild);
    }

    pub fn set_objects(&self, payload: CarrierPayload) {
        self.send(Message::Objects(Box::new(payload)));
    }

    fn send(&self, message: Message) {
        if self.to_worker.send(message).is_err() {
            warn!("the carrier worker is gone; carriers will not update");
        }
    }

    pub fn shutdown(&mut self) {
        if let Ok(mut sidecar) = self.sidecar.lock() {
            if let Some(sidecar) = sidecar.as_mut() {
                sidecar.shutdown();
            }
        }
    }
}

impl Drop for CarrierSupervisor {
    fn drop(&mut self) {
        self.shutdown();
        // Dropping the sender ends the worker's loop.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Worker {
    site_root: PathBuf,
    database: PathBuf,
    options: CarrierOptions,
    state: Arc<RwLock<ProxyState>>,
    sidecar: Arc<Mutex<Option<Sidecar>>>,
    seen: Arc<Mutex<SeenContents>>,
    /// Each carrier's directory, which the sidecar builds it from.
    roots: BTreeMap<String, PathBuf>,
    payload: Option<CarrierPayload>,
}

impl Worker {
    fn run(mut self, inbox: mpsc::Receiver<Message>) {
        while let Ok(message) = inbox.recv() {
            match message {
                Message::Rebuild => {
                    if let Err(e) = self.rebuild() {
                        self.fail(e);
                    }
                }
                Message::Objects(payload) => {
                    self.payload = Some(*payload);
                    if let Err(e) = self.push_state() {
                        warn!("couldn't send the site's objects to a carrier: {}", e);
                    }
                }
            }
        }
    }

    fn fail(&self, error: anyhow::Error) {
        let message = format!("{}", error);
        println!(
            "{} {}",
            style("Carriers failed:").red(),
            style(&message).red()
        );
        *self.state.write().unwrap() = ProxyState::Failed(message);
        *self.sidecar.lock().unwrap() = None;
    }

    fn rebuild(&mut self) -> Result<()> {
        // The sidecar builds a carrier the first time it is asked for, so one
        // that does not build answers its own requests with why and leaves the
        // others running.
        self.roots = discovery::discover(&self.site_root)?
            .into_iter()
            .map(|carrier| (carrier.name, carrier.root))
            .collect();
        if self.roots.is_empty() {
            self.seen.lock().unwrap().prime(&self.site_root);
            *self.state.write().unwrap() = ProxyState::Idle;
            *self.sidecar.lock().unwrap() = None;
            return Ok(());
        }
        let node = NodeInfo::detect()?;
        debug!("carriers run on node {}", node.version);
        let harness = host::write_harness(&host::harness_dir(&self.site_root))?;
        // Before the sidecar reads a single carrier, so that its own reads are
        // measured against what it is about to run.
        self.seen.lock().unwrap().prime(&self.site_root);
        // Node's module cache cannot be invalidated, so changed code means a
        // new process. Booting one costs about as much as an import.
        //
        // Flagged before the old one dies, so a request arriving mid-restart
        // waits for the replacement instead of being sent to a dead port.
        *self.state.write().unwrap() = ProxyState::Starting;
        *self.sidecar.lock().unwrap() = None;
        let sidecar = Sidecar::spawn(&harness, &self.options.node_args, self.options.port)?;
        sidecar.wait_until_healthy()?;
        *self.sidecar.lock().unwrap() = Some(sidecar);
        self.push_state()?;
        *self.state.write().unwrap() = ProxyState::Ready {
            port: self
                .sidecar
                .lock()
                .unwrap()
                .as_ref()
                .expect("just stored")
                .port(),
        };
        println!(
            "{} {}",
            style("Carriers ready:").green(),
            style(
                self.roots
                    .keys()
                    .map(|n| format!("/carriers/{}", n))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .green()
        );
        Ok(())
    }

    fn push_state(&self) -> Result<()> {
        let sidecar = self.sidecar.lock().unwrap();
        let (Some(sidecar), Some(payload)) = (sidecar.as_ref(), &self.payload) else {
            // Either nothing is running yet, or the first build hasn't produced
            // objects. Whichever comes second pushes.
            return Ok(());
        };
        sidecar.push_state(&self.roots, &self.database, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::write;
    use tempfile::TempDir;

    #[test]
    fn a_new_file_counts_as_changed_once() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("index.js");
        write(&file, "a").unwrap();
        let mut seen = SeenContents::default();
        assert!(seen.changed(&file));
        assert!(!seen.changed(&file), "an unchanged file must not rebuild");
    }

    #[test]
    fn identical_contents_do_not_rebuild() {
        // This is what stops a carrier's own `build` script from looping: it
        // rewrites the same bytes, and the second pass is a no-op.
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("index.js");
        write(&file, "a").unwrap();
        let mut seen = SeenContents::default();
        assert!(seen.changed(&file));
        write(&file, "a").unwrap();
        assert!(!seen.changed(&file));
        write(&file, "b").unwrap();
        assert!(seen.changed(&file), "a real edit always rebuilds");
    }

    #[test]
    fn priming_makes_a_read_of_an_untouched_carrier_a_non_event() {
        // Windows reports carriers/<name> when the sidecar imports the file
        // inside it. Without a baseline that first report looks like a change
        // and restarts the sidecar out from under the request that caused it.
        let site = TempDir::new().unwrap();
        let carrier = site.path().join("carriers/echo");
        std::fs::create_dir_all(&carrier).unwrap();
        write(carrier.join("index.js"), "a").unwrap();

        let mut seen = SeenContents::default();
        seen.prime(site.path());
        assert!(!seen.changed(&site.path().join("carriers")));
        assert!(!seen.changed(&carrier));
        assert!(!seen.changed(&carrier.join("index.js")));

        write(carrier.join("index.js"), "b").unwrap();
        assert!(
            seen.changed(&carrier.join("index.js")),
            "a real edit still rebuilds"
        );
    }

    #[test]
    fn priming_ignores_dependencies() {
        let site = TempDir::new().unwrap();
        let modules = site.path().join("carriers/echo/node_modules/dep");
        std::fs::create_dir_all(&modules).unwrap();
        write(modules.join("index.js"), "a").unwrap();

        let mut seen = SeenContents::default();
        seen.prime(site.path());
        // Not primed, so not tracked - the change loop drops these before it
        // ever asks.
        assert!(seen.changed(&modules.join("index.js")));
    }

    #[test]
    fn a_directory_changes_only_when_its_listing_does() {
        // Windows reports the directory when a file inside it is merely read,
        // so treating every directory event as a change restarts the sidecar
        // out from under the request that caused the read.
        let dir = TempDir::new().unwrap();
        write(dir.path().join("index.js"), "a").unwrap();
        let mut seen = SeenContents::default();
        assert!(seen.changed(dir.path()), "first sight is a change");
        assert!(!seen.changed(dir.path()), "reading it is not");

        // Editing a file inside it is that file's event, not the directory's.
        write(dir.path().join("index.js"), "b").unwrap();
        assert!(!seen.changed(dir.path()));

        // Adding or removing a carrier is a change to the listing.
        write(dir.path().join("other.js"), "c").unwrap();
        assert!(seen.changed(dir.path()));
        std::fs::remove_file(dir.path().join("other.js")).unwrap();
        assert!(seen.changed(dir.path()));
    }

    #[test]
    #[cfg(unix)]
    fn an_unreadable_file_counts_as_changed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("index.js");
        write(&file, "a").unwrap();
        let mut seen = SeenContents::default();
        assert!(seen.changed(&file));

        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_mode(0o000);
        std::fs::set_permissions(&file, permissions.clone()).unwrap();
        // Root reads anything, so only assert where the mode applies.
        if std::fs::read(&file).is_err() {
            assert!(seen.changed(&file));
        }
        permissions.set_mode(0o644);
        std::fs::set_permissions(&file, permissions).unwrap();
    }

    #[test]
    fn a_deleted_file_counts_as_changed_once() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("index.js");
        write(&file, "a").unwrap();
        let mut seen = SeenContents::default();
        assert!(seen.changed(&file));
        std::fs::remove_file(&file).unwrap();
        assert!(seen.changed(&file));
        assert!(!seen.changed(&file), "still gone is not a change");
    }
}
