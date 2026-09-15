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
use std::sync::{atomic::AtomicBool, Arc};

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
        for s in site.manifest.prebuild {
            let cmd_parts: Vec<&str> = s.split_whitespace().collect();
            if !cmd_parts.is_empty() {
                println!("runnning {} {}", cmd_parts[0], cmd_parts[1..].join(" "));
                let status = std::process::Command::new(cmd_parts[0])
                    .args(&cmd_parts[1..])
                    .current_dir(&root_dir)
                    .spawn()
                    .unwrap_or_else(|_| panic!("spawn failed: {}", s))
                    .wait();
                match status {
                    Ok(status) => {
                        if !status.success() {
                            return Ok(ExitStatus::Error);
                        }
                    }
                    Err(e) => {
                        println!("error: {}", e);
                        return Ok(ExitStatus::Error);
                    }
                }
            }
        }
        Ok(ExitStatus::Ok)
    }
}
