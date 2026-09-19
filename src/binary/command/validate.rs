//! `archival validate` - check a site without building it.
//!
//! Exists because the alternatives all lie in one direction or another: `build`
//! parses object values with validation skipped and, with `--skip-failures`,
//! exits 0 having printed its failures to stderr; `objects` logs a bad object
//! file and steps over it. Anything automating archival - an editor, a CI job,
//! a model being scored - needs one answer that is complete, exact and shaped
//! for a program rather than for a terminal.

use super::BinaryCommand;
use crate::binary::command::{add_args, command_root, CommandConfig};
use crate::binary::ExitStatus;
use crate::diagnostic::Severity;
use crate::{file_system_stdlib, validate};
use anyhow::Result;
use clap::{Arg, ArgAction, ArgMatches};
use std::sync::{atomic::AtomicBool, Arc};

pub struct Command {}
impl BinaryCommand for Command {
    fn name(&self) -> &str {
        "validate"
    }
    fn cli(&self, cmd: clap::Command) -> clap::Command {
        add_args(
            cmd.about("checks an archival site and reports what is wrong")
                .arg(
                    Arg::new("json")
                        .long("json")
                        .help("write the report as json on stdout")
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("quiet")
                        .long("quiet")
                        .short('q')
                        .help("report errors only, leaving out warnings")
                        .action(ArgAction::SetTrue),
                ),
            CommandConfig::no_build(),
        )
    }
    fn handler(&self, args: &ArgMatches, _quit: Arc<AtomicBool>) -> Result<ExitStatus> {
        let root_dir = command_root(args);
        let fs = file_system_stdlib::NativeFileSystem::new(&root_dir);
        let mut report = validate::site(&fs, &root_dir);
        if args.get_flag("quiet") {
            report
                .diagnostics
                .retain(|one| matches!(one.severity, Severity::Error));
        }

        if args.get_flag("json") {
            // stdout carries the document and nothing else, so a caller can
            // parse it without stripping anything first.
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            for one in &report.diagnostics {
                let label = match one.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                };
                println!("{label}: {one}");
            }
            if report.diagnostics.is_empty() {
                println!("no problems found");
            }
        }

        match report.has_errors() {
            true => Ok(ExitStatus::Error),
            false => Ok(ExitStatus::Ok),
        }
    }
}
