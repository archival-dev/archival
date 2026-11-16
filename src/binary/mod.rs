#[cfg(feature = "carriers")]
pub(crate) mod carriers;
pub mod command;
pub mod config;
pub mod dev_server;

/// The JSON file of what a local run sells, which `site.payments` answers from
/// and pages render under `archival.payments`.
pub(crate) const PAYMENTS_FILE_ENV: &str = "ARCHIVAL_PAYMENTS";

use self::command::{ExitStatus, COMMANDS};
use anyhow::Result;
use clap::{command, Command};
pub use config::ArchivalConfig;
use std::{
    env,
    sync::{atomic::AtomicBool, Arc},
};

pub fn binary(
    args: impl Iterator<Item = String>,
    quit: Option<Arc<AtomicBool>>,
) -> Result<ExitStatus> {
    let mut cmd: Command = command!().arg_required_else_help(true);
    for command in COMMANDS {
        cmd = cmd.subcommand(command.cli(Command::new(command.name())));
    }
    let matches = cmd.get_matches_from(args);
    let quit = if let Some(quit) = quit {
        quit
    } else {
        Arc::new(AtomicBool::default())
    };
    COMMANDS
        .iter()
        .find_map(|c| {
            matches
                .subcommand_matches(c.name())
                .map(|args| c.handler(args, quit.clone()))
        })
        .unwrap_or_else(|| panic!("No command provided."))
}
