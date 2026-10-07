use super::BinaryCommand;
use crate::{
    binary::{
        command::{add_args, command_root, CommandConfig},
        ExitStatus,
    },
    file_system_stdlib,
    site::Site,
    BuildOptions, ScriptErrors,
};
use anyhow::Result;
use clap::{arg, value_parser, ArgMatches};
use std::{
    path::{Component, Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
};

/// Resolve `..` and `.` components in `path` without requiring the path to
/// exist on disk (unlike `std::fs::canonicalize`).
fn lexical_normalize(path: impl AsRef<Path>) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.as_ref().components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

pub struct Command {}
impl BinaryCommand for Command {
    fn name(&self) -> &str {
        "build"
    }
    fn cli(&self, cmd: clap::Command) -> clap::Command {
        add_args(cmd.about("builds an archival site").arg(
            // NOTE: weird long form quoting due to https://github.com/clap-rs/clap/issues/3586
            arg!(-b --"build-dir" <build_dir> "Override the directory to build to (defaults to the manifest's build_dir)")
                .value_parser(value_parser!(PathBuf)),
        ).arg(
            arg!(-s --"skip-failures" "If a page fails to build, continue building other pages rather than erroring early, and skip the failing page.").required(false),
        ), CommandConfig::archival_site())
    }
    fn handler(
        &self,
        args: &ArgMatches,
        _quit: Arc<AtomicBool>,
    ) -> Result<crate::binary::ExitStatus> {
        let root_dir = command_root(args);
        let mut fs = file_system_stdlib::NativeFileSystem::new(&root_dir);
        let upload_prefix = args.get_one::<String>("upload-prefix").map(|s| s.as_str());
        let mut site = Site::load(&fs, upload_prefix)?;
        println!("Building site: {}", site);
        if let (Some(written), Some(absolute)) = (
            site.manifest.site_url.as_deref(),
            site.manifest.absolute_site_url(),
        ) {
            if written != absolute {
                eprintln!(
                    "warning: site_url {written:?} has no scheme, so pages render it as {absolute:?}"
                );
            }
        }
        if let Some(build_dir_arg) = args.get_one::<PathBuf>("build-dir") {
            let cwd = std::env::current_dir().unwrap();
            site.manifest.build_dir = lexical_normalize(cwd.join(build_dir_arg));
        }
        let mut options = BuildOptions::default();
        if args.get_flag("skip-failures") {
            options.skip_failures = true;
        }
        if let Err(e) = site.sync_static_files(&mut fs) {
            match e.downcast_ref::<ScriptErrors>() {
                Some(errors) if options.skip_failures => eprintln!("warning: {errors}"),
                _ => return Err(e),
            }
        }
        site.build(&mut fs, options)?;
        Ok(ExitStatus::Ok)
    }
}
