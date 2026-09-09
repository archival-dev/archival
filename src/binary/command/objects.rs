use super::BinaryCommand;
use crate::{
    binary::{
        command::{add_args, command_root, CommandConfig},
        ExitStatus,
    },
    file_system_stdlib,
    object::{
        context_value::{ContextObject, ContextValue},
        ObjectEntry,
    },
    page::debug_context,
    site::Site,
};
use anyhow::Result;
use clap::ArgMatches;
use std::sync::{atomic::AtomicBool, Arc};

pub struct Command {}
impl BinaryCommand for Command {
    fn name(&self) -> &str {
        "objects"
    }
    fn cli(&self, cmd: clap::Command) -> clap::Command {
        add_args(
            cmd.about("lists the objects in this site"),
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
        let mut objects = ContextObject::new();
        let definitions = &site.object_definitions;
        for (name, obj_entry) in site.get_objects(&fs)? {
            let definition = definitions
                .get(&name)
                .unwrap_or_else(|| panic!("missing object definition {}", name));
            let values = match obj_entry {
                ObjectEntry::List(l) => ContextValue::array(
                    l.iter()
                        .map(|o| o.liquid_object(definition, &site.field_config).into()),
                ),
                ObjectEntry::Object(o) => o.liquid_object(definition, &site.field_config).into(),
            };
            objects.insert(liquid::model::KString::from_string(name.clone()), values);
        }
        let mut context = ContextObject::new();
        context.insert("objects".into(), ContextValue::Object(objects));
        println!("{}", debug_context(&context, 0));
        // let page = Page::new(
        //     "objects-template",
        //     "",
        //     TemplateType::Default,
        //     &"",
        // );
        // let render_o = page.render(&liquid_parser, &all_objects);
        Ok(ExitStatus::Ok)
        // match compat {
        //     true => Ok(ExitStatus::Ok),
        //     false => Ok(ExitStatus::Error),
        // }
    }
}
