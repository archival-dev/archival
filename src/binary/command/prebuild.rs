use super::BinaryCommand;
use crate::{
    binary::{
        command::{add_args, command_root, CommandConfig},
        ExitStatus,
    },
    file_system_stdlib,
    site::Site,
};
use anyhow::Result;
use clap::ArgMatches;
use std::{
    path::Path,
    sync::{atomic::AtomicBool, Arc},
};

/// Runs each manifest hook command in `root_dir`, in order, stopping at the first failure.
pub(super) fn run_commands(root_dir: &Path, commands: &[String]) -> ExitStatus {
    for s in commands {
        let cmd_parts: Vec<&str> = s.split_whitespace().collect();
        if cmd_parts.is_empty() {
            continue;
        }
        println!("running {}", cmd_parts.join(" "));
        let status = std::process::Command::new(cmd_parts[0])
            .args(&cmd_parts[1..])
            .current_dir(root_dir)
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => {
                println!("{} failed: {}", s, status);
                return ExitStatus::Error;
            }
            Err(e) => {
                println!("error running {}: {}", s, e);
                return ExitStatus::Error;
            }
        }
    }
    ExitStatus::Ok
}

pub struct Command {}
impl BinaryCommand for Command {
    fn name(&self) -> &str {
        "prebuild"
    }
    fn cli(&self, cmd: clap::Command) -> clap::Command {
        add_args(
            cmd.about("runs external build commands, if configured."),
            CommandConfig::no_build(),
        )
    }
    fn handler(
        &self,
        args: &ArgMatches,
        _quit: Arc<AtomicBool>,
    ) -> Result<crate::binary::ExitStatus> {
        let root_dir = command_root(args);
        let fs = file_system_stdlib::NativeFileSystem::new(&root_dir);
        let site = Site::load(&fs, Some(""))?;
        Ok(run_commands(&root_dir, &site.manifest.prebuild))
    }
}
