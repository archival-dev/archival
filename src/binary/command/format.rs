use super::BinaryCommand;
use crate::{
    binary::{
        command::{add_args, command_root, CommandConfig},
        ExitStatus,
    },
    file_system_stdlib,
    manifest::Manifest,
    toml_comments::preserve_comments,
    Archival, ArchivalError, FileSystemAPI,
};
use anyhow::Result;
use clap::ArgMatches;
use std::path::Path;
use std::sync::{atomic::AtomicBool, Arc};

impl Archival<file_system_stdlib::NativeFileSystem> {
    fn format_objects(&self) -> Result<()> {
        self.fs_mutex.with_fs(|fs| {
            let all_objects = self.site.get_objects(fs)?;
            let definitions = &self.site.object_definitions;
            for (obj_type, objects) in &all_objects {
                for object in objects {
                    let path = self.object_path_impl(obj_type, &object.filename, fs)?;
                    let def = definitions.get(obj_type).ok_or_else(|| {
                        ArchivalError::new(&format!("missing object definition: {obj_type}"))
                    })?;
                    let formatted = object.to_toml(def)?;
                    let contents = match fs.read_to_string(&path)? {
                        Some(original) => preserve_comments(&original, &formatted),
                        None => formatted,
                    };
                    fs.write_str(&path, contents)?;
                }
            }
            Ok(())
        })
    }
    fn format_manifest(&self) -> Result<()> {
        self.fs_mutex.with_fs(|fs| {
            let manifest_path = Manifest::path_in(Path::new(""), fs)?;
            let formatted = self.site.manifest.to_toml()?;
            let contents = match fs.read_to_string(&manifest_path)? {
                Some(original) => preserve_comments(&original, &formatted),
                None => formatted,
            };
            fs.write_str(manifest_path, contents)
        })
    }
}

pub struct Command {}
impl BinaryCommand for Command {
    fn name(&self) -> &str {
        "format"
    }
    fn cli(&self, cmd: clap::Command) -> clap::Command {
        add_args(
            cmd.about("formats toml files in an archival site"),
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
        let archival = Archival::new(fs)?;
        archival.format_objects()?;
        archival.format_manifest()?;
        Ok(ExitStatus::Ok)
    }
}
