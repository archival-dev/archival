#[cfg(feature = "carriers")]
use crate::binary::carriers::{objects::build_payload, CarrierOptions, CarrierSupervisor};
use crate::BuildOptions;
use crate::{file_system::WatchableFileSystemAPI, file_system_stdlib, server, site::Site};
use anyhow::Result;
use console::{style, Term};
use indicatif::ProgressBar;
use rsa::pkcs8::der::Writer;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, RwLock};
use std::time::{Duration, Instant};
use std::{process::exit, sync::atomic::Ordering, thread};
use tracing::warn;

#[derive(Debug, Default)]
pub enum DevServerMode {
    #[default]
    NoServe,
    NoServeStreamLogs,
    Serve(Option<u16>),
}

#[derive(Debug, Default)]
pub struct UploadsConfig<'a> {
    pub url: Option<&'a str>,
    pub prefix: Option<&'a str>,
}

impl<'a> UploadsConfig<'a> {
    pub fn prefix(prefix: &'a str) -> Self {
        UploadsConfig {
            url: None,
            prefix: Some(prefix),
        }
    }
    pub fn new(url: &'a str, prefix: &'a str) -> Self {
        UploadsConfig {
            url: Some(url),
            prefix: Some(prefix),
        }
    }
}

pub struct DevServerOptions<'a> {
    pub root_dir: PathBuf,
    pub uploads_config: UploadsConfig<'a>,
    pub mode: DevServerMode,
    pub change_sender: Option<mpsc::Sender<Vec<PathBuf>>>,
    pub watch_paths: Option<Vec<String>>,
    pub quit: Arc<AtomicBool>,
    #[cfg(feature = "carriers")]
    pub carriers: CarrierOptions,
    /// Overrides the `SITE_URL` carriers see. Defaults to this server's address.
    #[cfg(feature = "carriers")]
    pub site_url: Option<String>,
}

// External API
pub fn watch(
    root_dir: PathBuf,
    uploads_config: UploadsConfig,
    mode: DevServerMode,
    change_sender: Option<mpsc::Sender<Vec<PathBuf>>>,
    watch_paths: Option<Vec<String>>,
    quit: Arc<AtomicBool>,
) -> Result<crate::binary::ExitStatus> {
    watch_with(DevServerOptions {
        root_dir,
        uploads_config,
        mode,
        change_sender,
        watch_paths,
        quit,
        #[cfg(feature = "carriers")]
        carriers: CarrierOptions {
            disabled: true,
            ..Default::default()
        },
        #[cfg(feature = "carriers")]
        site_url: None,
    })
}

pub fn watch_with(options: DevServerOptions) -> Result<crate::binary::ExitStatus> {
    let DevServerOptions {
        root_dir,
        uploads_config,
        mode,
        change_sender,
        watch_paths,
        quit,
        #[cfg(feature = "carriers")]
            carriers: carrier_options,
        #[cfg(feature = "carriers")]
        site_url,
    } = options;
    let mut term = Term::stdout();
    let is_interactive =
        term.features().is_attended() && !matches!(mode, DevServerMode::NoServeStreamLogs);
    let mut fs = file_system_stdlib::NativeFileSystem::new(&root_dir);
    let mut site = Site::load(&fs, uploads_config.prefix)?;
    if let Some(uploads_url) = uploads_config.url {
        site.modify_manifest(&mut fs, |manifest| {
            manifest.uploads_url = Some(uploads_url.to_string());
        })?;
    }
    site.sync_static_files(&mut fs)?;
    let (tx, rx) = mpsc::channel();
    let initial_build = site.build(&mut fs, BuildOptions::default());
    let mut init_message = format!("Watching site: {}", site);
    let change_queue = Arc::new(RwLock::new(vec![]));
    let queue_changes = change_sender.is_some();
    // This won't leak because the process is ended when we
    // abort anyway
    let watcher_queue = change_queue.clone();
    let mut merged_watch_paths = watch_paths.unwrap_or_default();
    merged_watch_paths.append(&mut site.manifest.watched_paths());
    // Carriers need a running server to be reachable, so with --noserve there
    // is nothing to attach them to.
    #[cfg(feature = "carriers")]
    let carriers = if matches!(mode, DevServerMode::Serve(_)) {
        CarrierSupervisor::new(&root_dir, carrier_options)
    } else {
        None
    };
    // watch_paths is an allowlist, not an exclusion list.
    #[cfg(feature = "carriers")]
    if carriers.is_some() {
        merged_watch_paths.push(crate::binary::carriers::discovery::CARRIERS_DIR_NAME.to_string());
    }
    let kill_watcher = fs.watch(fs.root.to_owned(), merged_watch_paths, move |paths| {
        for path in paths {
            if queue_changes {
                watcher_queue.write().unwrap().push(path.clone());
            }
            if let Err(e) = tx.send(path.clone()) {
                warn!("Failed sending change event to builder: {}", e);
            }
        }
    })?;
    let path = root_dir.join(&site.manifest.build_dir);
    #[cfg(feature = "carriers")]
    let mut carrier_site_url = site_url;
    if let DevServerMode::Serve(port) = mode {
        let mut sb = server::ServerBuilder::new(&path, Some("404.html"));
        if let Some(port) = port {
            sb.port(port);
        }
        #[cfg(feature = "carriers")]
        if let Some(carriers) = &carriers {
            sb.handler(carriers.proxy());
        }
        let server = sb.build();
        init_message += &format!("Serving {}\n", path.display());
        init_message += &format!("See http://{}\n", server.addr());
        init_message += "Hit CTRL-C to stop\n";
        // Locally the site is the dev server, so this is where a carrier's
        // fetch of its own site should land - not the deployed site_url.
        #[cfg(feature = "carriers")]
        {
            carrier_site_url.get_or_insert_with(|| format!("http://{}", server.addr()));
        }
        thread::spawn(move || {
            server.serve().unwrap();
        });
    }
    let quit_clone = quit.clone();
    // exit() runs no destructors, so the sidecar has to be killed here. Its
    // stdin pipe closing on process death is the backstop for everything else.
    #[cfg(feature = "carriers")]
    let carrier_child = carriers.as_ref().map(|c| c.child());
    ctrlc::set_handler(move || {
        quit_clone.store(true, Ordering::SeqCst);
        #[cfg(feature = "carriers")]
        if let Some(child) = &carrier_child {
            if let Some(sidecar) = child.lock().unwrap().as_mut() {
                sidecar.shutdown();
            }
        }
        exit(0);
    })?;
    let mut last_build = Instant::now();
    let mut changed = false;
    // The object definition file (the "objects file" the manifest points to) is
    // parsed into memory once at load time; invalidate_file only clears caches
    // and never refreshes it. Track edits to it so we can reload the site.
    let mut object_definitions_changed = false;
    // Static files are synced at startup, so rebuilds only need to re-sync
    // them when a file inside the static dir actually changed.
    let mut static_files_changed = false;
    #[cfg(feature = "carriers")]
    let mut carriers_changed = false;
    // The first payload is pushed once, whether or not anything rebuilds.
    #[cfg(feature = "carriers")]
    let mut carrier_objects_stale = true;
    term.write(init_message.as_bytes())?;
    if let Err(e) = initial_build {
        let bar = ProgressBar::new_spinner();
        bar.finish_with_message(format!("Initial build failed: {}", e));
    }
    loop {
        // Block (briefly) rather than spinning; the timeout keeps the change
        // batching and quit checks below responsive.
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(path) => {
                let changed_file = path.strip_prefix(&root_dir).unwrap();
                // Carrier files are none of the site's business. Handing them
                // to invalidate_file would make every file an `npm install`
                // writes trigger a full site rebuild.
                //
                // Handled without `continue`, so that a burst of carrier events
                // arriving faster than this loop drains them cannot starve the
                // rebuild below.
                #[allow(unused_mut)]
                let mut claimed = false;
                #[cfg(feature = "carriers")]
                if let Some(carriers) = &carriers {
                    claimed = carriers.claims(changed_file);
                    if claimed && carriers.should_rebuild(&root_dir, changed_file) {
                        carriers_changed = true;
                    }
                }
                if !claimed {
                    if changed_file == site.manifest.object_definition_file {
                        object_definitions_changed = true;
                    }
                    if changed_file.starts_with(&site.manifest.static_dir) {
                        static_files_changed = true;
                    }
                    site.invalidate_file(changed_file);
                    changed = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("Build Channel Disconnected.")
            }
        }
        if quit.load(Ordering::SeqCst) {
            kill_watcher();
            exit(0);
        }
        #[cfg(feature = "carriers")]
        if carriers_changed && Instant::now() - last_build > Duration::from_millis(200) {
            carriers_changed = false;
            if let Some(carriers) = &carriers {
                carriers.rebuild();
            }
        }
        #[cfg(feature = "carriers")]
        if carrier_objects_stale {
            if let Some(carriers) = &carriers {
                let fs = file_system_stdlib::NativeFileSystem::new(&root_dir);
                match build_payload(&site, &fs, carrier_site_url.as_deref().unwrap_or_default()) {
                    Ok(payload) => {
                        carriers.set_objects(payload);
                        carrier_objects_stale = false;
                    }
                    Err(e) => {
                        warn!("couldn't read this site's objects for carriers: {}", e);
                        carrier_objects_stale = false;
                    }
                }
            } else {
                carrier_objects_stale = false;
            }
        }
        // Batch changes every 200ms
        if changed && Instant::now() - last_build > Duration::from_millis(200) {
            last_build = Instant::now();
            changed = false;
            let bar = if is_interactive {
                let bar = ProgressBar::new_spinner();
                bar.enable_steady_tick(Duration::from_millis(100));
                if is_interactive {
                    term.clear_screen()?;
                }
                term.write(init_message.as_bytes())?;
                bar.set_message("Rebuilding...");
                Some(bar)
            } else {
                println!("Rebuilding...");
                None
            };
            let mut fs = file_system_stdlib::NativeFileSystem::new(&root_dir);
            // When the object definition file changed, reload the site so the
            // rebuild picks up the latest definitions. On failure we keep the
            // previously loaded site so the dev server stays up.
            let mut reload_error = None;
            let mut site_reloaded = false;
            if object_definitions_changed {
                object_definitions_changed = false;
                match Site::load(&fs, uploads_config.prefix) {
                    Ok(mut reloaded) => {
                        if let Some(uploads_url) = uploads_config.url {
                            reloaded.manifest.uploads_url = Some(uploads_url.to_string());
                        }
                        site = reloaded;
                        site_reloaded = true;
                    }
                    Err(e) => reload_error = Some(e),
                }
            }
            let output = if let Some(e) = reload_error {
                format!("{} {}", style("Reload failed:").red(), style(e).red())
            } else {
                // Reloading the site clears its static file cache (and may
                // change the static dir), so sync in that case too.
                let sync_result = if static_files_changed || site_reloaded {
                    static_files_changed = false;
                    site.sync_static_files(&mut fs)
                } else {
                    Ok(())
                };
                if let Err(e) = sync_result {
                    format!(
                        "{} {}",
                        style("Static file sync failed:").red(),
                        style(e).red()
                    )
                } else if let Err(e) = site.build(&mut fs, BuildOptions::default()) {
                    format!("{} {}", style("Build failed:").red(), style(e).red())
                } else {
                    #[cfg(feature = "carriers")]
                    {
                        carrier_objects_stale = true;
                    }
                    format!(
                        "{} {:?}",
                        style("Rebuilt in").green(),
                        style(Instant::now() - last_build).green()
                    )
                }
            };
            if let Some(bar) = bar {
                bar.finish_with_message(output);
                if let Some(cs) = &change_sender {
                    let mut guard = change_queue.write().unwrap();
                    let q = std::mem::take(&mut *guard);
                    if let Err(e) = cs.send(q) {
                        warn!("Failed sending change event to host: {}", e);
                    }
                }
            } else {
                println!("{}", output);
            }
        }
    }
}
