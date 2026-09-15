use super::{prebuild::run_commands, BinaryCommand};
use crate::{
    binary::command::{add_args, command_root, CommandConfig},
    file_system_stdlib,
    site::Site,
};
use anyhow::Result;
use clap::ArgMatches;
use std::sync::{atomic::AtomicBool, Arc};

pub struct Command {}
impl BinaryCommand for Command {
    fn name(&self) -> &str {
        "postbuild"
    }
    fn cli(&self, cmd: clap::Command) -> clap::Command {
        add_args(
            cmd.about("runs external commands against the build output, if configured."),
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
        Ok(run_commands(&root_dir, &site.manifest.postbuild))
    }
}
