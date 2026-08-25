#[cfg(feature = "json-schema")]
use crate::json_schema;
use crate::{
    check_compatibility,
    constants::MANIFEST_FILE_NAME,
    liquid_parser::{self, PARTIAL_FILE_NAME_RE},
    manifest::Manifest,
    object::{Object, ObjectEntry, Renderable, RenderedObject, RenderedObjectMap},
    object_definition::{ObjectDefinition, ObjectDefinitions},
    page::{build_context, Page, RenderGlobals, TemplateType},
    read_toml::read_toml,
    tags::layout,
    util::path_to_slash,
    ArchivalError, BuildOptions, FieldConfig, FileSystemAPI, ObjectMap,
};
use anyhow::Result;
use seahash::SeaHasher;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    hash::Hasher,
    path::{Path, PathBuf},
    sync::{
        atomic::{self, AtomicU64},
        RwLock,
    },
};
use thiserror::Error;
use tracing::{debug, error, instrument, trace_span, warn};

#[derive(Error, Debug, Clone)]
pub enum InvalidFileError {
    #[error("missing extension in path ({0})")]
    MissingFileExtension(PathBuf),
    #[error("unrecognized file type '{0}' in path {0}")]
    UnrecognizedType(String, PathBuf),
    #[error("cannot define both {0} and {1}")]
    DuplicateObjectDefinition(String, String),
    #[error("invalid root object {0}: {1}")]
    InvalidRootObject(String, String),
    #[error("unknown object {0}")]
    UnknownObject(String),
}

#[derive(Error, Debug, Clone)]
pub enum BuildError {
    #[error("template file {0} does not exist.")]
    MissingTemplate(String),
    #[error("failed parsing template {0}:\n{1}")]
    TemplateParseError(String, String),
    #[error("failed rendering object {0} to {1} template:\n{2}")]
    TemplateRenderError(String, String, String),
    #[error("page {0} failed rendering:\n{1}")]
    PageRenderError(String, String),
}

/// Parsing liquid is expensive (a fixed cost to compile partials into a
/// parser, plus a per-template parse), so parsers and parsed templates are
/// cached across builds. Both caches are keyed by content hashes so they are
/// self-invalidating: when any partial or layout changes the parser (and with
/// it every cached template, which may embed partials) is rebuilt, and when a
/// template changes only that template is re-parsed.
///
/// The parser must outlive any render of the templates it parsed: it owns the
/// `Language` that `crate::tags::output` holds a `Weak` reference to in order
/// to render liquid found in field values. Dropping the parser and its
/// templates together (as replacing this whole struct does) keeps that true.
struct ParserCache {
    partials_hash: u64,
    parser: std::sync::Arc<liquid::Parser>,
    templates: HashMap<u64, std::sync::Arc<liquid::Template>>,
}

impl std::fmt::Debug for ParserCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParserCache")
            .field("partials_hash", &self.partials_hash)
            .field("templates", &self.templates.len())
            .finish()
    }
}

const TEMPLATE_CACHE_MAX_ENTRIES: usize = 256;

#[derive(Debug, Deserialize, Serialize)]
pub struct Site {
    pub object_definitions: ObjectDefinitions,
    pub field_config: FieldConfig,
    pub manifest: Manifest,

    #[serde(skip)]
    obj_cache: RwLock<HashMap<PathBuf, Object>>,
    #[serde(skip)]
    static_file_cache: RwLock<HashMap<PathBuf, u64>>,
    #[serde(skip)]
    build_cache: RwLock<HashMap<PathBuf, u64>>,
    #[serde(skip)]
    cache_generation: AtomicU64,
    #[serde(skip)]
    parser_cache: RwLock<Option<ParserCache>>,
}

// Site is shared across threads (e.g. the dev server); keep it Send + Sync
// even as cached liquid types are added.
#[allow(dead_code)]
fn _assert_site_send_sync() {
    fn assert<T: Send + Sync>() {}
    assert::<Site>();
}

impl std::fmt::Display for Site {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            r#"
    === Objects:
        {}
    === Manifest: {}
        "#,
            self.object_definitions
                .keys()
                .map(|o| o.as_str().to_string())
                .collect::<Vec<String>>()
                .join("\n        "),
            self.manifest
        )
    }
}

fn get_order(obj: &Object) -> String {
    if let Some(order) = obj.order {
        format!("{:0>10}", order)
    } else {
        obj.filename.to_owned()
    }
}

impl Site {
    #[instrument(skip(fs))]
    pub fn load(fs: &impl FileSystemAPI, upload_prefix: Option<&str>) -> Result<Site> {
        // Load our manifest (should it exist)
        let manifest_path = Manifest::path_in(Path::new(""), fs)?;
        let mut manifest = if fs.exists(&manifest_path)? {
            let manifest = Manifest::from_file(&manifest_path, fs, upload_prefix)?;
            // When loading a manifest, check its compatibility.
            if let Some(manifest_version) = &manifest.archival_version {
                let (compat, message) = check_compatibility(manifest_version);
                if !compat {
                    return Err(ArchivalError::new(&message).into());
                }
            }
            manifest
        } else {
            Manifest::default(
                Path::new(""),
                upload_prefix.ok_or_else(|| {
                    ArchivalError::new(&format!(
                        "upload_prefix must be manually defined if {} is not present.",
                        MANIFEST_FILE_NAME
                    ))
                })?,
            )
        };
        manifest.resolve_object_definition_file(fs)?;
        let odf = Path::new(&manifest.object_definition_file);
        if !fs.exists(odf)? {
            return Err(ArchivalError::new(&format!(
                "Object definition file {} does not exist",
                fs.root_dir().join(odf).to_string_lossy()
            ))
            .into());
        }

        // Load our object definitions
        #[cfg(feature = "verbose-logging")]
        debug!("loading definition {}", odf.display());
        // Read as a string rather than via read_toml: definitions carry the
        // comments written around them, which the toml value model drops.
        let objects_source = fs.read_to_string(odf)?.ok_or_else(|| {
            ArchivalError::new(&format!(
                "Object definition file {} does not exist",
                fs.root_dir().join(odf).to_string_lossy()
            ))
        })?;
        let objects = ObjectDefinition::from_source(&objects_source, &manifest.editor_types)?;

        Ok(Site {
            field_config: FieldConfig::from_manifest(Some(&manifest), upload_prefix)?,
            manifest,
            object_definitions: objects,
            obj_cache: RwLock::new(HashMap::new()),
            static_file_cache: RwLock::new(HashMap::new()),
            build_cache: RwLock::new(HashMap::new()),
            cache_generation: AtomicU64::new(0),
            parser_cache: RwLock::new(None),
        })
    }

    /// Returns a parser for the site's current partials, reusing the cached
    /// one when no partial or layout has changed since it was built.
    fn get_or_build_parser<T: FileSystemAPI>(
        &self,
        pages_dir: &Path,
        layout_dir: Option<&Path>,
        fs: &T,
    ) -> Result<std::sync::Arc<liquid::Parser>> {
        let _span = trace_span!("get_or_build_parser").entered();
        let (source, partials_hash) =
            liquid_parser::partials_hash(Some(pages_dir), layout_dir, fs)?;
        if let Some(cache) = self.parser_cache.read().unwrap().as_ref() {
            if cache.partials_hash == partials_hash {
                return Ok(cache.parser.clone());
            }
        }
        let parser = std::sync::Arc::new(liquid_parser::build_with_partials(source)?);
        *self.parser_cache.write().unwrap() = Some(ParserCache {
            partials_hash,
            parser: parser.clone(),
            templates: HashMap::new(),
        });
        Ok(parser)
    }

    /// Parses `source` with `parser`, reusing a previously parsed template for
    /// identical source. The cache lives inside ParserCache, so it is cleared
    /// whenever the parser is rebuilt (cached templates may embed partials).
    fn get_or_parse_template(
        &self,
        parser: &liquid::Parser,
        source: &str,
    ) -> Result<std::sync::Arc<liquid::Template>, liquid_core::Error> {
        let mut hasher = SeaHasher::new();
        hasher.write(source.as_bytes());
        let key = hasher.finish();
        if let Some(cache) = self.parser_cache.read().unwrap().as_ref() {
            if let Some(template) = cache.templates.get(&key) {
                return Ok(template.clone());
            }
        }
        let _span = trace_span!("parse_template").entered();
        let template = std::sync::Arc::new(liquid_parser::parse(parser, source)?);
        if let Some(cache) = self.parser_cache.write().unwrap().as_mut() {
            if cache.templates.len() >= TEMPLATE_CACHE_MAX_ENTRIES {
                cache.templates.clear();
            }
            cache.templates.insert(key, template.clone());
        }
        Ok(template)
    }

    /// A counter that increments whenever object content is invalidated
    /// (edited, added, deleted, or externally changed). Callers that cache
    /// state derived from objects can compare generations to skip
    /// recomputation when nothing has changed.
    pub fn objects_generation(&self) -> u64 {
        self.cache_generation.load(atomic::Ordering::Relaxed)
    }

    pub fn build_id(&self) -> u64 {
        let mut hasher = SeaHasher::new();
        let hashes = self.build_cache.read().unwrap();
        if hashes.is_empty() {
            return 0;
        }
        for hash in hashes.values() {
            let hash_slice = hash.to_ne_bytes();
            hasher.write(&hash_slice);
        }
        // Include cache generation to ensure build_id changes after invalidation
        let generation = self.cache_generation.load(atomic::Ordering::Relaxed);
        hasher.write(&generation.to_ne_bytes());
        hasher.finish()
    }

    pub fn schema_prefix(&self) -> String {
        self.manifest.site_url.as_ref().map_or_else(
            || {
                format!(
                    "{}",
                    hash_file(
                        serde_json::to_string(&self.object_definitions)
                            .unwrap_or_default()
                            .as_bytes()
                    )
                )
            },
            |s| s.to_owned(),
        )
    }

    pub fn root_objects(&self, fs: &impl FileSystemAPI) -> HashSet<String> {
        let mut root_objects = HashSet::new();
        let objects_dir = &self.manifest.objects_dir;
        for object_name in self.object_definitions.keys() {
            let root_object_file_path = objects_dir.join(format!("{}.toml", object_name));
            if fs.exists(&root_object_file_path).unwrap() {
                root_objects.insert(object_name.to_string());
            }
        }
        root_objects
    }

    #[cfg(feature = "json-schema")]
    pub fn dump_schemas(&self, fs: &mut impl FileSystemAPI) -> Result<()> {
        use crate::ObjectSchemaOptions;

        debug!(
            "dumping schemas for {}",
            self.manifest.object_definition_file.display()
        );
        let _ = fs.remove_dir_all(&self.manifest.schemas_dir);
        fs.create_dir_all(&self.manifest.schemas_dir)?;
        for (name, def) in &self.object_definitions {
            let schema = json_schema::generate_json_schema(
                &format!("{}/{}.schema.json", self.schema_prefix(), name),
                def,
                ObjectSchemaOptions::default(),
            );
            fs.write_str(
                self.manifest
                    .schemas_dir
                    .join(format!("{}.schema.json", name)),
                serde_json::to_string_pretty(&schema).unwrap(),
            )?;
        }
        Ok(())
    }
    #[cfg(feature = "json-schema")]
    pub fn dump_schema(&self, object: &String, fs: &mut impl FileSystemAPI) -> Result<()> {
        use crate::ObjectSchemaOptions;

        debug!("dumping schema for {}", object);
        let def = self
            .object_definitions
            .get(object)
            .ok_or_else(|| InvalidFileError::UnknownObject(object.clone()))?;
        let schema = json_schema::generate_json_schema(
            &format!("{}/{}.schema.json", self.schema_prefix(), object),
            def,
            ObjectSchemaOptions::default(),
        );
        fs.write_str(
            self.manifest
                .schemas_dir
                .join(format!("{}.schema.json", object)),
            serde_json::to_string_pretty(&schema).unwrap(),
        )?;
        Ok(())
    }

    #[instrument(skip(fs))]
    pub fn get_objects<T: FileSystemAPI>(&self, fs: &T) -> Result<ObjectMap> {
        self.get_objects_sorted(
            fs,
            Some(|a: &_, b: &_| get_order(a).partial_cmp(&get_order(b)).unwrap()),
        )
    }
    #[instrument(skip(fs))]
    pub fn get_rendered_objects<T: FileSystemAPI>(&self, fs: &T) -> Result<RenderedObjectMap> {
        let objects = self.get_objects(fs)?;
        Ok(objects.rendered(&self.field_config))
    }

    #[instrument(skip(fs))]
    pub fn get_rendered_object<T: FileSystemAPI>(
        &self,
        object_name: &str,
        filename: Option<&str>,
        fs: &T,
    ) -> Result<RenderedObject> {
        self.get_object(object_name, filename, fs)
            .map(|o| o.rendered(&self.field_config))
    }

    #[instrument(skip(fs, sort))]
    pub fn get_rendered_objects_sorted<T: FileSystemAPI>(
        &self,
        fs: &T,
        sort: Option<impl Fn(&Object, &Object) -> Ordering>,
    ) -> Result<RenderedObjectMap> {
        let objects = self.get_objects_sorted(fs, sort)?;
        Ok(objects.rendered(&self.field_config))
    }

    #[instrument]
    pub fn invalidate_file(&self, file: &Path) {
        #[cfg(feature = "verbose-logging")]
        debug!("invalidate {}", file.display());
        self.obj_cache.write().unwrap().remove(file);
        // Increment cache generation to ensure build_id changes after invalidation.
        // We don't clear build_cache here because it's needed for file cleanup
        // in site.build() - it tracks which output files need to be deleted.
        self.cache_generation
            .fetch_add(1, atomic::Ordering::Relaxed);
    }

    #[instrument(skip(fs, modify))]
    pub fn modify_manifest<T: FileSystemAPI>(
        &mut self,
        fs: &mut T,
        modify: impl FnOnce(&mut Manifest),
    ) -> Result<()> {
        modify(&mut self.manifest);
        let manifest_path = Manifest::path_in(Path::new(""), fs)?;
        fs.write_str(manifest_path, self.manifest.to_toml()?)
    }

    pub fn manifest_content<T: FileSystemAPI>(&self, fs: &T) -> Result<String> {
        fs.read_to_string(Manifest::path_in(Path::new(""), fs)?)
            .map(|m| m.unwrap_or_default())
    }

    #[instrument(skip(fs))]
    pub fn get_object<T: FileSystemAPI>(
        &self,
        object_name: &str,
        filename: Option<&str>,
        fs: &T,
    ) -> Result<Object> {
        let object_def = self
            .object_definitions
            .get(object_name)
            .ok_or_else(|| InvalidFileError::UnknownObject(object_name.to_string()))?;
        let mut cache = self.obj_cache.write().unwrap();
        if let Some(filename) = filename {
            let root_path = self.path_for_object(object_name, None);
            if filename == object_name && fs.exists(&root_path)? {
                return self.object_for_path(&root_path, object_def, &mut cache, fs);
            }
        }
        let path = self.path_for_object(object_name, filename);
        self.object_for_path(&path, object_def, &mut cache, fs)
    }

    fn path_for_object(&self, object_name: &str, filename: Option<&str>) -> PathBuf {
        let objects_dir = &self.manifest.objects_dir;
        if let Some(filename) = filename {
            objects_dir
                .join(object_name)
                .join(format!("{}.toml", filename))
        } else {
            objects_dir.join(format!("{}.toml", object_name))
        }
    }

    #[instrument(skip(fs, sort))]
    pub fn get_objects_sorted<T: FileSystemAPI>(
        &self,
        fs: &T,
        sort: Option<impl Fn(&Object, &Object) -> Ordering>,
    ) -> Result<ObjectMap> {
        let mut all_objects: ObjectMap = ObjectMap::new();
        let objects_dir = &self.manifest.objects_dir;
        for (object_name, object_def) in self.object_definitions.iter() {
            let object_files_path = objects_dir.join(object_name);
            let object_file_path = objects_dir.join(format!("{}.toml", object_name));
            let mut cache = self.obj_cache.write().unwrap();
            if fs.is_dir(&object_files_path)? {
                if fs.exists(&object_file_path)? {
                    return Err(InvalidFileError::DuplicateObjectDefinition(
                        object_files_path.display().to_string(),
                        object_file_path.display().to_string(),
                    )
                    .into());
                }
                let mut objects: Vec<Object> = Vec::new();
                for file in fs.walk_dir(&object_files_path, false)? {
                    let path = object_files_path.join(&file);
                    match self.object_for_path(&path, object_def, &mut cache, fs) {
                        Ok(obj) => {
                            objects.push(obj);
                        }
                        Err(err) => {
                            error!("Invalid file {:?}: {}", path, err);
                        }
                    }
                }
                // Sort objects by order key
                if let Some(sort) = &sort {
                    trace_span!("sort objects");
                    objects.sort_by(sort);
                }
                all_objects.insert(object_name.clone(), ObjectEntry::from_vec(objects));
            } else if fs.exists(&object_file_path)? {
                match self.object_for_path(&object_file_path, object_def, &mut cache, fs) {
                    Ok(obj) => {
                        all_objects.insert(object_name.clone(), ObjectEntry::from_object(obj));
                    }
                    Err(error) => {
                        // This error is unrecoverable because if we have a root
                        // file, we cannot create an empty list for this type
                        // since it would violate our "list or root" rule.
                        return Err(InvalidFileError::InvalidRootObject(
                            object_file_path.display().to_string(),
                            error.to_string(),
                        )
                        .into());
                    }
                }
            } else {
                all_objects.insert(object_name.clone(), ObjectEntry::empty_list());
            }
        }
        Ok(all_objects)
    }

    #[instrument(skip(object_def, cache, fs))]
    fn object_for_path<T: FileSystemAPI>(
        &self,
        path: &Path,
        object_def: &ObjectDefinition,
        cache: &mut HashMap<PathBuf, Object>,
        fs: &T,
    ) -> Result<Object> {
        let ext = path
            .extension()
            .ok_or_else(|| InvalidFileError::MissingFileExtension(path.to_path_buf()))?;
        if ext != "toml" {
            return Err(InvalidFileError::UnrecognizedType(
                ext.to_string_lossy().to_string(),
                path.to_path_buf(),
            )
            .into());
        }
        if let Some(o) = cache.get(path) {
            Ok(o.clone())
        } else {
            #[cfg(feature = "verbose-logging")]
            debug!("parsing {}", path.display());
            let obj_table = read_toml(path, fs)?;
            let o = Object::from_table(
                object_def,
                Path::new(path.with_extension("").file_name().unwrap()),
                &obj_table,
                &self.manifest.editor_types,
                // Skip validation when populating the cache, we may have new
                // objects with invalid unset keys
                true,
            )?;
            cache.insert(path.to_path_buf(), o.clone());
            Ok(o)
        }
    }

    #[instrument(skip(fs))]
    pub fn sync_static_files<T: FileSystemAPI>(&self, fs: &mut T) -> Result<()> {
        let Manifest {
            static_dir,
            build_dir,
            ..
        } = &self.manifest;
        if !fs.exists(build_dir)? {
            fs.create_dir_all(build_dir)?;
        }
        let mut hashes = self.static_file_cache.write().unwrap();
        let last_dist_paths: Vec<PathBuf> = hashes.keys().cloned().collect();
        let mut copied_paths: HashSet<PathBuf> = HashSet::new();
        // Copy static files
        #[cfg(feature = "verbose-logging")]
        debug!("copying files from {}", static_dir.display());
        if fs.exists(static_dir)? {
            for file in fs.walk_dir(static_dir, false)? {
                let from = static_dir.join(&file);
                if let Some(content) = fs.read(&from)? {
                    let current_hash = hash_file(&content);
                    copied_paths.insert(file.clone());
                    // If there is an existing hash and it matches the current
                    // file, leave it there.
                    if let Some(existing_hash) = hashes.get(&file) {
                        if *existing_hash == current_hash {
                            continue;
                        }
                    }
                    // Otherwise, copy the file and store the latest hash.
                    let dest = build_dir.join(&file);
                    if let Some(dirname) = dest.parent() {
                        if dirname != build_dir {
                            fs.create_dir_all(dirname)?;
                        }
                    }
                    fs.write(&dest, content)?;
                    hashes.insert(file, current_hash);
                }
            }
            // Remove any files in dest that are no longer in static
            for path in last_dist_paths {
                if !copied_paths.contains(&path) {
                    fs.delete(&path)?;
                    hashes.remove(&path);
                }
            }
        } else {
            debug!("static dir {} does not exist.", static_dir.display());
        }
        Ok(())
    }

    #[instrument(skip(fs))]
    pub fn build<T: FileSystemAPI>(&self, fs: &mut T, options: BuildOptions) -> Result<()> {
        let Manifest {
            objects_dir,
            layout_dir,
            pages_dir,
            build_dir,
            site_url,
            ..
        } = &self.manifest;

        let globals = RenderGlobals {
            site_url: site_url.as_ref().map(|v| v.into()).unwrap_or_default(),
        };

        let mut built_hashes = HashMap::new();

        // Validate paths
        if !fs.exists(objects_dir)? {
            return Err(ArchivalError::new(&format!(
                "Objects dir {} does not exist",
                fs.root_dir().join(objects_dir).to_string_lossy()
            ))
            .into());
        }
        if !fs.exists(pages_dir)? {
            return Err(ArchivalError::new(&format!(
                "Pages dir {} does not exist",
                fs.root_dir().join(pages_dir).to_string_lossy()
            ))
            .into());
        }
        if !fs.exists(build_dir)? {
            fs.create_dir_all(build_dir)?;
        }

        let all_objects = self.get_objects(fs)?;

        // for (n, os) in &all_objects {
        //     debug!("{}", n);
        //     for o in os {
        //         debug!("{}", o.filename);
        //         for (k, v) in &o.values {
        //             debug!("{}: {:?}", k, v);
        //         }
        //     }
        // }

        let liquid_parser = self.get_or_build_parser(
            pages_dir,
            if fs.exists(layout_dir)? {
                Some(layout_dir)
            } else {
                None
            },
            fs,
        )?;

        // Build the shared render context once; it is identical for every
        // page and converting objects to liquid values (including rendering
        // markdown) is the expensive part of a render.
        let base_context = build_context(
            &all_objects,
            &self.object_definitions,
            &self.field_config,
            &globals,
        );

        // Render template pages
        for (name, object_def) in self.object_definitions.iter() {
            if let Some(template) = &object_def.template {
                let template_path = pages_dir.join(format!("{}.liquid", template));
                #[cfg(feature = "verbose-logging")]
                debug!("rendering template objects for {}", template_path.display());
                if !fs.exists(&template_path)? {
                    let err =
                        BuildError::MissingTemplate(template_path.display().to_string()).into();
                    if options.skip_failures {
                        warn!("skipping error: {err}");
                        eprintln!("skipping error: {err}");
                        continue;
                    } else {
                        return Err(err);
                    }
                }
                let template_r = fs.read_to_string(&template_path);
                if let Err(e) = &template_r {
                    warn!("failed rendering {}: {e}", template_path.display());
                    eprintln!("failed rendering {}: {e}", template_path.display());
                    if options.skip_failures {
                        continue;
                    }
                }
                let template_str = template_r?;
                if let Some(template_str) = template_str {
                    // Parse each template once and share it across all of the
                    // objects rendered with it (and across builds, via the
                    // template cache).
                    let parsed_template =
                        match self.get_or_parse_template(&liquid_parser, &template_str) {
                            Ok(t) => t,
                            Err(e) => {
                                let err = BuildError::TemplateParseError(
                                    template_path.display().to_string(),
                                    e.to_string(),
                                )
                                .into();
                                if options.skip_failures {
                                    warn!("skipping error: {err}");
                                    eprintln!("skipping error: {err}");
                                    continue;
                                } else {
                                    return Err(err);
                                }
                            }
                        };
                    if let Some(t_objects) = all_objects.get(name) {
                        for object in t_objects.into_iter() {
                            #[cfg(feature = "verbose-logging")]
                            debug!("rendering {}", object.filename);
                            let result = Self::render_template_page(
                                object,
                                object_def,
                                &parsed_template,
                                &template_path,
                                build_dir,
                                &self.field_config,
                                &base_context,
                                fs,
                                &liquid_parser,
                                &self.build_cache,
                            )
                            .map_err(|error| {
                                BuildError::TemplateRenderError(
                                    object.filename.to_string(),
                                    template.to_string(),
                                    error.to_string(),
                                )
                            });
                            if let Err(e) = &result {
                                warn!("failed rendering {}: {e}", template_path.display());
                                eprintln!("failed rendering {}: {e}", template_path.display());
                                if options.skip_failures {
                                    continue;
                                }
                            }
                            let (path, hash) = result?;
                            built_hashes.insert(path, hash);
                        }
                    }
                }
            }
        }

        // Render regular pages
        #[cfg(feature = "verbose-logging")]
        debug!("building pages in {}", pages_dir.display());
        let template_pages: HashSet<&str> = self
            .object_definitions
            .values()
            .flat_map(|object| object.template.as_deref())
            .collect();
        for rel_path in fs.walk_dir(pages_dir, false)? {
            let file_path = pages_dir.join(&rel_path);
            if let Some(name) = rel_path.file_name() {
                let file_name = name.to_string_lossy();
                if let Some((page_name, page_type)) = TemplateType::parse_path(&file_name) {
                    // Template names come from object definitions, which always
                    // use `/`, so compare against a slash-separated path.
                    let template_path_str = path_to_slash(rel_path.with_extension(""));
                    if template_pages.contains(&template_path_str[..])
                        || PARTIAL_FILE_NAME_RE.is_match(&file_name)
                    {
                        // template pages are not rendered as pages
                        continue;
                    }
                    #[cfg(feature = "verbose-logging")]
                    debug!(
                        "rendering {} ({})",
                        file_path.display(),
                        page_type.extension()
                    );
                    let result = self
                        .render_page(
                            &rel_path,
                            &file_path,
                            page_name,
                            page_type,
                            build_dir,
                            &base_context,
                            fs,
                            &liquid_parser,
                        )
                        .map_err(|error| {
                            BuildError::PageRenderError(page_name.to_string(), error.to_string())
                        });
                    if let Err(e) = &result {
                        warn!("{e}");
                        eprintln!("{e}");
                        if options.skip_failures {
                            continue;
                        }
                    }
                    let (path, hash) = result?;
                    if let Some(path) = path {
                        built_hashes.insert(path, hash);
                    }
                }
            }
        }

        let mut current_cache = self.build_cache.write().unwrap();
        for key in current_cache.keys() {
            if !built_hashes.contains_key(key) {
                fs.delete(key)?;
            }
        }
        *current_cache = built_hashes;
        Ok(())
    }

    #[instrument(skip(template, base_context, fs, liquid_parser))]
    #[allow(clippy::too_many_arguments)]
    fn render_template_page<T: FileSystemAPI>(
        object: &Object,
        object_def: &ObjectDefinition,
        template: &liquid::Template,
        template_path: &PathBuf,
        build_dir: &PathBuf,
        field_config: &FieldConfig,
        base_context: &liquid::Object,
        fs: &mut T,
        liquid_parser: &liquid::Parser,
        build_cache: &RwLock<HashMap<PathBuf, u64>>,
    ) -> Result<(PathBuf, u64)> {
        let page = Page::new_with_parsed_template(
            object.filename.clone(),
            object_def,
            object,
            template,
            TemplateType::parse_path(&template_path.display().to_string())
                .unwrap_or_default()
                .1,
            template_path,
        );
        let render_o = page.render(liquid_parser, base_context, field_config);
        if render_o.is_err() {
            warn!("failed rendering {}", object.filename);
        }
        let rendered = layout::post_process(render_o?);
        let render_name = format!("{}.{}", object.filename, page.extension());
        let t_dir = build_dir.join(&object_def.name);
        fs.create_dir_all(&t_dir)?;
        let build_path = t_dir.join(render_name);
        let hash = hash_file(rendered.as_bytes());
        let should_write = if let Some(prev_hash) = build_cache.read().unwrap().get(&build_path) {
            hash != *prev_hash
        } else {
            true
        };
        if should_write {
            #[cfg(feature = "verbose-logging")]
            debug!("write template {}", build_path.display());
            fs.write_str(&build_path, rendered)?;
        } else {
            #[cfg(feature = "verbose-logging")]
            debug!("template no-op {}", build_path.display());
        }
        Ok((build_path, hash))
    }

    #[instrument(skip(self, base_context, fs, liquid_parser))]
    #[allow(clippy::too_many_arguments)]
    fn render_page<T: FileSystemAPI>(
        &self,
        rel_path: &PathBuf,
        file_path: &PathBuf,
        page_name: &str,
        page_type: TemplateType,
        build_dir: &PathBuf,
        base_context: &liquid::Object,
        fs: &mut T,
        liquid_parser: &liquid::Parser,
    ) -> Result<(Option<PathBuf>, u64)> {
        let field_config = &self.field_config;
        let build_cache = &self.build_cache;
        if let Some(template_str) = fs.read_to_string(file_path)? {
            let template = self.get_or_parse_template(liquid_parser, &template_str)?;
            let page = Page::new_with_parsed_content(
                page_name.to_string(),
                &template,
                TemplateType::Default,
                file_path,
            );
            let render_o = page.render(liquid_parser, base_context, field_config);
            if render_o.is_err() {
                warn!("failed rendering {}", file_path.display());
            }
            let rendered = layout::post_process(render_o?);
            let mut render_dir = build_dir.to_path_buf();
            if let Some(parent_dir) = rel_path.parent() {
                render_dir = render_dir.join(parent_dir);
                fs.create_dir_all(&render_dir)?;
            }
            let render_path = render_dir.join(format!("{}.{}", page_name, page_type.extension()));
            let hash = hash_file(rendered.as_bytes());
            let should_write =
                if let Some(prev_hash) = build_cache.read().unwrap().get(&render_path) {
                    hash != *prev_hash
                } else {
                    true
                };
            if should_write {
                #[cfg(feature = "verbose-logging")]
                debug!("write page {}", render_path.display());
                fs.write_str(&render_path, rendered)?;
            } else {
                #[cfg(feature = "verbose-logging")]
                debug!("page no-op {}", render_path.display());
            }
            Ok((Some(render_path), hash))
        } else {
            warn!("page not found: {}", file_path.display());
            Ok((None, hash_file(&[])))
        }
    }
}

fn hash_file(file: &[u8]) -> u64 {
    let mut hasher = SeaHasher::new();
    hasher.write(file);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::OBJECT_DEFINITION_FILE_NAME;
    use crate::fields::ObjectValues;
    use crate::file_system_memory::MemoryFileSystem;
    use crate::util::path_to_slash;

    /// Sites whose templates and partials live in subdirectories name them with
    /// `/`, but the pages that back them are found by walking the filesystem,
    /// which uses the platform separator. This builds such a site end to end so
    /// the two representations stay reconciled (see
    /// https://github.com/archival-dev/archival/issues/16).
    fn nested_template_site() -> Result<MemoryFileSystem> {
        let mut fs = MemoryFileSystem::default();
        fs.write_str(
            Path::new(OBJECT_DEFINITION_FILE_NAME),
            "[post]\ntemplate = \"posts/single\"\nname = \"string\"\n".to_string(),
        )?;
        fs.write_str(
            Path::new("objects/post/a-post.toml"),
            "name = \"A Post\"\n".to_string(),
        )?;
        fs.write_str(
            Path::new("pages/posts/single.liquid"),
            "{% include 'posts/byline' %}\npath: {{post.path}}\n".to_string(),
        )?;
        fs.write_str(
            Path::new("pages/posts/_byline.liquid"),
            "byline: {{post.name}}\n".to_string(),
        )?;
        fs.write_str(Path::new("pages/index.liquid"), "index\n".to_string())?;
        Ok(fs)
    }

    #[test]
    fn builds_templates_and_partials_from_subdirectories() -> Result<()> {
        let mut fs = nested_template_site()?;
        let site = Site::load(&fs, Some("test"))?;
        site.build(&mut fs, BuildOptions::default())?;

        let build_dir = &site.manifest.build_dir;
        let rendered = fs
            .read_to_string(build_dir.join("post").join("a-post.html"))?
            .expect("template object page was not built");
        // The partial is registered under its `/`-joined name, so the include
        // in the template has to find it.
        assert!(
            rendered.contains("byline: A Post"),
            "partial in a subdirectory was not included: {rendered}"
        );
        // `path` is used to build urls, so it never contains a backslash.
        assert!(
            rendered.contains("path: post/a-post"),
            "object path was not rendered as a url path: {rendered}"
        );
        assert!(
            fs.exists(build_dir.join("index.html"))?,
            "page was not built"
        );
        // Templates and partials back other pages; they are not pages
        // themselves.
        assert!(
            !fs.exists(build_dir.join("posts").join("single.html"))?,
            "template in a subdirectory was also rendered as a page"
        );
        assert!(
            !fs.exists(build_dir.join("posts").join("_byline.html"))?,
            "partial in a subdirectory was also rendered as a page"
        );
        Ok(())
    }

    /// Shopify's include syntax (comma-separated arguments, `with` and `for`
    /// clauses) built end to end; see `crate::tags::include`.
    #[test]
    fn builds_includes_with_shopify_syntax() -> Result<()> {
        let mut fs = MemoryFileSystem::default();
        fs.write_str(
            Path::new(OBJECT_DEFINITION_FILE_NAME),
            "[post]\nname = \"string\"\n".to_string(),
        )?;
        fs.write_str(
            Path::new("objects/post/a-post.toml"),
            "name = \"A Post\"\n".to_string(),
        )?;
        fs.write_str(
            Path::new("pages/index.liquid"),
            "{% for post in posts %}{% include 'byline', post: post, tag: 'h1' %}{% endfor %}\n\
             {% include 'byline' for posts as post, tag: 'h2' %}\n\
             {% include 'byline' with posts[0] as post, tag: 'h3' %}\n"
                .to_string(),
        )?;
        fs.write_str(
            Path::new("pages/_byline.liquid"),
            "<{{tag}}>{{post.name}}</{{tag}}>\n".to_string(),
        )?;
        let site = Site::load(&fs, Some("test"))?;
        site.build(&mut fs, BuildOptions::default())?;

        let rendered = fs
            .read_to_string(site.manifest.build_dir.join("index.html"))?
            .expect("page was not built");
        for (tag, syntax) in [
            ("h1", "comma-separated arguments"),
            ("h2", "`for` clause"),
            ("h3", "`with` clause"),
        ] {
            assert!(
                rendered.contains(&format!("<{tag}>A Post</{tag}>")),
                "include with {syntax} did not render: {rendered}"
            );
        }
        Ok(())
    }

    /// `render` accepts the same syntax as `include`, but renders the partial
    /// in an isolated scope; see `crate::tags::render`.
    #[test]
    fn builds_renders_with_shopify_syntax() -> Result<()> {
        let mut fs = MemoryFileSystem::default();
        fs.write_str(
            Path::new(OBJECT_DEFINITION_FILE_NAME),
            "[post]\nname = \"string\"\n".to_string(),
        )?;
        fs.write_str(
            Path::new("objects/post/a-post.toml"),
            "name = \"A Post\"\n".to_string(),
        )?;
        fs.write_str(
            Path::new("pages/index.liquid"),
            "{% for post in posts %}{% render 'bylines/byline', post: post, tag: 'h1' %}{% endfor %}\n\
             {% render 'bylines/byline' for posts as post, tag: 'h2' %}\n\
             {% render 'bylines/byline' with posts[0] as post, tag: 'h3' %}\n\
             {% render 'bylines/global' %}\n"
                .to_string(),
        )?;
        fs.write_str(
            Path::new("pages/bylines/_byline.liquid"),
            "<{{tag}}>{{post.name}}</{{tag}}>\n".to_string(),
        )?;
        // Site objects and archival's own variables are global, so a partial
        // reaches them without being passed anything.
        fs.write_str(
            Path::new("pages/bylines/_global.liquid"),
            "<global>{{posts[0].name}}/{{page}}</global>\n".to_string(),
        )?;
        let site = Site::load(&fs, Some("test"))?;
        site.build(&mut fs, BuildOptions::default())?;

        let rendered = fs
            .read_to_string(site.manifest.build_dir.join("index.html"))?
            .expect("page was not built");
        for (tag, syntax) in [
            ("h1", "comma-separated arguments"),
            ("h2", "`for` clause"),
            ("h3", "`with` clause"),
        ] {
            assert!(
                rendered.contains(&format!("<{tag}>A Post</{tag}>")),
                "render with {syntax} did not render: {rendered}"
            );
        }
        assert!(
            rendered.contains("<global>A Post/index</global>"),
            "globals did not reach the rendered partial: {rendered}"
        );
        Ok(())
    }

    /// The difference between the two tags: `include` shares the caller's
    /// scope, `render` does not.
    #[test]
    fn render_does_not_see_the_callers_locals() -> Result<()> {
        let site_with = |tag: &str| -> Result<MemoryFileSystem> {
            let mut fs = MemoryFileSystem::default();
            fs.write_str(
                Path::new(OBJECT_DEFINITION_FILE_NAME),
                "[post]\nname = \"string\"\n".to_string(),
            )?;
            fs.write_str(
                Path::new("objects/post/a-post.toml"),
                "name = \"A Post\"\n".to_string(),
            )?;
            fs.write_str(
                Path::new("pages/index.liquid"),
                format!("{{% assign local = 'visible' %}}{{% {tag} 'local' %}}\n"),
            )?;
            fs.write_str(Path::new("pages/_local.liquid"), "{{local}}\n".to_string())?;
            Ok(fs)
        };

        let mut fs = site_with("include")?;
        let site = Site::load(&fs, Some("test"))?;
        site.build(&mut fs, BuildOptions::default())?;
        let rendered = fs
            .read_to_string(site.manifest.build_dir.join("index.html"))?
            .expect("page was not built");
        assert!(
            rendered.contains("visible"),
            "include should share the caller's scope: {rendered}"
        );

        let mut fs = site_with("render")?;
        let site = Site::load(&fs, Some("test"))?;
        assert!(
            site.build(&mut fs, BuildOptions::default()).is_err(),
            "render should not see a variable the calling page assigned"
        );
        Ok(())
    }

    /// Nothing in the layout dir is built as a page, so every file in it is
    /// includable, in subdirectories and with or without the underscore that
    /// marks a partial in the pages dir (see
    /// https://github.com/archival-dev/archival/issues/29).
    #[test]
    fn builds_partials_from_the_layout_dir() -> Result<()> {
        let mut fs = MemoryFileSystem::default();
        fs.write_str(
            Path::new(OBJECT_DEFINITION_FILE_NAME),
            "[post]\nname = \"string\"\n".to_string(),
        )?;
        fs.write_str(
            Path::new("objects/post/a-post.toml"),
            "name = \"A Post\"\n".to_string(),
        )?;
        fs.write_str(
            Path::new("pages/index.liquid"),
            "{% layout 'wrappers/theme' %}\n\
             {% include 'header' %}\n\
             {% include 'sidebar' %}\n\
             {% include 'partials/nav' %}\n\
             {% include 'partials/footer', name: posts[0].name %}\n"
                .to_string(),
        )?;
        fs.write_str(
            Path::new("layout/wrappers/theme.liquid"),
            "<theme>{{page_content}}</theme>\n".to_string(),
        )?;
        fs.write_str(
            Path::new("layout/_header.liquid"),
            "<header/>\n".to_string(),
        )?;
        fs.write_str(Path::new("layout/sidebar.liquid"), "<aside/>\n".to_string())?;
        fs.write_str(
            Path::new("layout/partials/_nav.liquid"),
            "<nav/>\n".to_string(),
        )?;
        fs.write_str(
            Path::new("layout/partials/footer.liquid"),
            "<footer>{{name}}</footer>\n".to_string(),
        )?;
        let site = Site::load(&fs, Some("test"))?;
        site.build(&mut fs, BuildOptions::default())?;

        let rendered = fs
            .read_to_string(site.manifest.build_dir.join("index.html"))?
            .expect("page was not built");
        for (expected, description) in [
            ("<theme>", "layout in a subdirectory"),
            ("<header/>", "underscore-prefixed layout partial"),
            ("<aside/>", "layout partial without an underscore"),
            ("<nav/>", "layout partial in a subdirectory"),
            ("<footer>A Post</footer>", "layout partial with arguments"),
        ] {
            assert!(
                rendered.contains(expected),
                "{description} did not render: {rendered}"
            );
        }
        // Layouts back other pages; they are never pages themselves.
        assert!(
            !fs.exists(site.manifest.build_dir.join("sidebar.html"))?,
            "a layout was also rendered as a page"
        );
        Ok(())
    }

    #[test]
    fn object_paths_are_url_paths() {
        let object = Object {
            filename: "a-post".to_string(),
            object_name: "post".to_string(),
            order: None,
            values: ObjectValues::new(),
        };
        assert_eq!(object.url_path(), "post/a-post");
        assert_eq!(path_to_slash(object.path()), "post/a-post");
    }
}
