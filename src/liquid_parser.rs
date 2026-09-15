use crate::liquid_rewrite::rewrite_template;
use crate::named_lookup::NamedLookup;
use crate::tags::include::IncludeTag;
use crate::tags::output::{OutputContext, OutputTag};
use crate::tags::render::RenderTag;
use crate::{page::TemplateType, tags::layout::LayoutTag, util::path_to_slash, FileSystemAPI};
use anyhow::Result;
use liquid_core::model::ObjectView;
use liquid_core::partials::{EagerCompiler, PartialCompiler, PartialSource};
use liquid_core::runtime::{self, PartialStore, Renderable, RuntimeBuilder};
use liquid_core::Language;
use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::{Arc, OnceLock};
use std::{borrow::Cow, collections::HashMap, path::Path};
#[cfg(feature = "verbose-logging")]
use tracing::debug;
use tracing::{error, warn};

pub static PARTIAL_FILE_NAME_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^_(.+)\.liquid").unwrap());

#[derive(Default, Debug, Clone)]
pub(crate) struct ArchivalPartialSource {
    partials: HashMap<String, String>,
}

/// The name a template file is included by: its path relative to the directory
/// it was found in, without the extension, always `/`-separated because
/// templates refer to it by name (`{% include "dir/partial" %}`). Returns
/// `None` for files that are not includable partials.
///
/// Everything in the layout dir is includable, so there the underscore prefix
/// is optional and only stripped; in the pages dir it is what distinguishes a
/// partial from a page.
fn partial_name(file: &Path, underscore_required: bool) -> Option<String> {
    let file_name = file.file_name()?.to_str()?;
    let (name, _t) = TemplateType::parse_path(file_name)?;
    let name = match name.strip_prefix('_') {
        Some(stripped) => stripped,
        None if underscore_required => return None,
        None => name,
    };
    if name.is_empty() {
        return None;
    }
    Some(match file.parent() {
        Some(parent_dir) => path_to_slash(parent_dir.join(name)),
        None => name.to_string(),
    })
}

impl ArchivalPartialSource {
    pub fn new(
        pages_path: Option<&Path>,
        layout_path: Option<&Path>,
        fs: &impl FileSystemAPI,
    ) -> Result<Self> {
        let mut partials = HashMap::new();
        let mut add = |dir: &Path, underscore_required: bool| -> Result<()> {
            for file in fs.walk_dir(dir, false)? {
                let Some(name) = partial_name(&file, underscore_required) else {
                    continue;
                };
                #[cfg(feature = "verbose-logging")]
                debug!("adding partial {} ({})", name, file.display());
                match fs.read_to_string(dir.join(&file))? {
                    Some(contents) => {
                        if partials.insert(name.clone(), contents).is_some() {
                            warn!("partial {} was defined more than once", name);
                        }
                    }
                    None => error!("Failed reading partial {}", file.display()),
                }
            }
            Ok(())
        };
        if let Some(path) = layout_path {
            add(path, false)?;
        }
        if let Some(path) = pages_path {
            add(path, true)?;
        }
        Ok(Self { partials })
    }
}

impl ArchivalPartialSource {
    /// A stable hash of all partial names and contents. Used to decide whether
    /// a cached parser (whose compiled partials embed these sources) is still
    /// valid.
    pub fn source_hash(&self) -> u64 {
        use std::hash::Hasher;
        let mut keys: Vec<&String> = self.partials.keys().collect();
        keys.sort();
        let mut hasher = seahash::SeaHasher::new();
        for key in keys {
            hasher.write(key.as_bytes());
            hasher.write(&[0]);
            hasher.write(self.partials[key].as_bytes());
            hasher.write(&[0]);
        }
        hasher.finish()
    }
}

impl PartialSource for ArchivalPartialSource {
    fn contains(&self, name: &str) -> bool {
        self.partials.contains_key(name)
    }

    fn names(&self) -> Vec<&str> {
        let mut names = vec![];
        for k in self.partials.keys() {
            names.push(&k[..]);
        }
        names
    }

    /// Partials and layouts are rewritten on the way to the compiler so that
    /// field values they output are rendered in place (see
    /// `crate::liquid_rewrite`). Rewriting happens here rather than in `new()`
    /// because every build constructs an `ArchivalPartialSource` just to hash
    /// it, and only compiles on a cache miss.
    fn try_get<'a>(&'a self, name: &str) -> Option<Cow<'a, str>> {
        self.partials.get(name).map(|p| rewrite_template(p))
    }
}

// Builds cache parsers via Site (see partials_hash/build_with_partials); this
// one-shot constructor is kept for callers (and tests) that need a standalone
// parser.
#[allow(dead_code)]
pub fn get(
    pages_path: Option<&Path>,
    layout_path: Option<&Path>,
    fs: &impl FileSystemAPI,
) -> Result<Parser> {
    let source = ArchivalPartialSource::new(pages_path, layout_path, fs)?;
    build_with_partials(source)
}

/// Reads all partial/layout sources and returns their combined hash. Reading
/// and hashing sources is much cheaper than compiling them, so builds use this
/// to decide whether a cached parser can be reused.
pub(crate) fn partials_hash(
    pages_path: Option<&Path>,
    layout_path: Option<&Path>,
    fs: &impl FileSystemAPI,
) -> Result<(ArchivalPartialSource, u64)> {
    let source = ArchivalPartialSource::new(pages_path, layout_path, fs)?;
    let hash = source.source_hash();
    Ok((source, hash))
}

pub(crate) fn build_with_partials(source: ArchivalPartialSource) -> Result<Parser> {
    Ok(build_with_output_context(source)?.0)
}

/// Stands in for `liquid::Parser`, whose templates can only be rendered with a
/// runtime liquid builds, so they could not resolve objects by name (see
/// `crate::named_lookup`).
#[derive(Clone)]
pub struct Parser {
    language: Arc<Language>,
    partials: Arc<dyn PartialStore + Send + Sync>,
}

impl Parser {
    pub fn parse(&self, text: &str) -> std::result::Result<Template, liquid_core::Error> {
        Ok(Template {
            template: runtime::Template::new(liquid_core::parser::parse(text, &self.language)?),
            partials: Arc::clone(&self.partials),
        })
    }
}

pub struct Template {
    template: runtime::Template,
    partials: Arc<dyn PartialStore + Send + Sync>,
}

impl Template {
    pub fn render(
        &self,
        globals: &dyn ObjectView,
    ) -> std::result::Result<String, liquid_core::Error> {
        let runtime = RuntimeBuilder::new()
            .set_globals(globals)
            .set_partials(self.partials.as_ref())
            .build();
        let mut output = Vec::new();
        self.template
            .render_to(&mut output, &NamedLookup::new(&runtime))?;
        String::from_utf8(output).map_err(|e| liquid_core::Error::with_msg(e.to_string()))
    }
}

/// Also returns the parser's [`OutputContext`], whose lifetime is tied to the
/// parser: it holds a `Weak` reference to the parser's `Language`, and its
/// cache of parsed field values is only valid for that `Language`.
pub(crate) fn build_with_output_context(
    source: ArchivalPartialSource,
) -> Result<(Parser, Arc<OutputContext>)> {
    let ctx = Arc::new(OutputContext::default());
    let captured = Arc::new(OnceLock::new());
    let partials = CapturingCompiler {
        inner: EagerCompiler::new(source),
        ctx: Arc::clone(&ctx),
        captured: Arc::clone(&captured),
    };
    liquid::ParserBuilder::with_stdlib()
        .tag(LayoutTag)
        .tag(IncludeTag)
        .tag(RenderTag)
        .tag(OutputTag::new(Arc::clone(&ctx)))
        .partials(partials)
        .build()?;
    let parser = captured
        .get()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("internal: liquid partials were not compiled"))?;
    Ok((parser, ctx))
}

/// Parses a template, rewriting its output statements first so that liquid
/// inside the values it renders is evaluated in place. Every liquid template
/// archival parses should go through here.
pub(crate) fn parse(
    parser: &Parser,
    source: &str,
) -> std::result::Result<Template, liquid_core::Error> {
    parser.parse(&rewrite_template(source))
}

/// A pass-through partial compiler that captures the `Arc<Language>` that
/// `ParserBuilder::build` hands to it, and the partials it compiles. That is
/// the only way to get hold of either: the output tag needs the language to
/// parse liquid found inside values, and [`Parser`] needs both.
struct CapturingCompiler<C: PartialCompiler> {
    inner: C,
    ctx: Arc<OutputContext>,
    captured: Arc<OnceLock<Parser>>,
}

impl<C: PartialCompiler> PartialCompiler for CapturingCompiler<C> {
    fn compile(
        self,
        language: Arc<Language>,
    ) -> liquid_core::Result<Box<dyn PartialStore + Send + Sync>> {
        self.ctx.set_language(&language);
        let partials: Arc<dyn PartialStore + Send + Sync> =
            self.inner.compile(Arc::clone(&language))?.into();
        let _ = self.captured.set(Parser {
            language,
            partials: Arc::clone(&partials),
        });
        Ok(Box::new(SharedPartials(partials)))
    }

    fn source(&self) -> &dyn PartialSource {
        self.inner.source()
    }
}

#[derive(Debug)]
struct SharedPartials(Arc<dyn PartialStore + Send + Sync>);

impl PartialStore for SharedPartials {
    fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    fn names(&self) -> Vec<&str> {
        self.0.names()
    }

    fn try_get(&self, name: &str) -> Option<Arc<dyn Renderable>> {
        self.0.try_get(name)
    }

    fn get(&self, name: &str) -> liquid_core::Result<Arc<dyn Renderable>> {
        self.0.get(name)
    }
}
