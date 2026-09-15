use anyhow::Result;
use clap::{arg, value_parser, ArgMatches, Command};
use std::{
    env, fs,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, LazyLock},
};
mod build;
#[cfg(feature = "carriers")]
mod carriers;
mod compat;
mod format;
mod import;
mod login;
#[cfg(feature = "lsp")]
mod lsp;
mod manifest;
mod objects;
mod prebuild;
#[cfg(feature = "proto")]
mod proto;
mod run;
#[cfg(feature = "json-schema")]
mod schemas;
mod types;
mod upload;

pub enum ExitStatus {
    Error,
    Ok,
}
impl ExitStatus {
    pub fn code(&self) -> i32 {
        match self {
            ExitStatus::Error => 1,
            ExitStatus::Ok => 0,
        }
    }
}

#[derive(Default, Debug)]
pub struct CommandConfig {
    upload_prefix: bool,
    site_path: bool,
}
impl CommandConfig {
    pub fn archival_site() -> Self {
        CommandConfig {
            upload_prefix: true,
            site_path: true,
        }
    }
    pub fn no_build() -> Self {
        CommandConfig {
            upload_prefix: false,
            site_path: true,
        }
    }
    pub fn upload_only() -> Self {
        CommandConfig {
            upload_prefix: true,
            site_path: false,
        }
    }
}

static CWD: LazyLock<PathBuf> = std::sync::LazyLock::new(|| env::current_dir().unwrap());

pub trait BinaryCommand {
    fn name(&self) -> &str;
    fn cli(&self, cmd: Command) -> Command;
    fn handler(&self, args: &ArgMatches, quit: Arc<AtomicBool>) -> Result<ExitStatus>;
}

pub fn add_args(mut cmd: Command, config: CommandConfig) -> Command {
    if config.upload_prefix {
        cmd = cmd.arg(
                // NOTE: weird long form quoting due to https://github.com/clap-rs/clap/issues/3586
                arg!(-u --"upload-prefix" <upload_prefix> "override the upload prefix. If no archival.toml is present, this is required.")
                    .value_parser(value_parser!(String)),
            );
    }
    if config.site_path {
        cmd = cmd.arg(
                arg!([site_path] "an optional path to the archival site. Otherwise will be auto-detected from cwd.")
                    .required(false)
                    .default_value(CWD.to_str())
                    .value_parser(value_parser!(PathBuf)),
            );
    }
    cmd
}

pub fn command_root(args: &ArgMatches) -> PathBuf {
    let path = match args.try_get_one::<PathBuf>("site_path") {
        Ok(arg) => arg.unwrap(), // defaulted so unwrap is ok
        Err(_) => &CWD.to_path_buf(),
    };
    if *path != *CWD {
        // If not default, the path is relative
        fs::canonicalize(CWD.join(path)).unwrap()
    } else {
        CWD.to_path_buf()
    }
}

pub const COMMANDS: &[&'static dyn BinaryCommand] = &[
    &build::Command {},
    &run::Command {},
    &format::Command {},
    &manifest::Command {},
    &prebuild::Command {},
    &login::Command {},
    &compat::Command {},
    &upload::Command {},
    &import::Command {},
    &objects::Command {},
    &types::Command {},
    #[cfg(feature = "carriers")]
    &carriers::Command {},
    #[cfg(feature = "json-schema")]
    &schemas::Command {},
    #[cfg(feature = "lsp")]
    &lsp::Command {},
    #[cfg(feature = "proto")]
    &proto::Command {},
];
