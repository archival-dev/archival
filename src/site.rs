#[cfg(feature = "json-schema")]
use crate::json_schema;
use crate::{
    check_compatibility,
    constants::MANIFEST_FILE_NAME,
    liquid_parser::{self, PARTIAL_FILE_NAME_RE},
    manifest::Manifest,
    object::{Object, ObjectEntry, Renderable, RenderedObject, RenderedObjectMap},
    object_definition::{ObjectDefinition, ObjectDefinitions},
    page::{
        build_context, ContextDep, ContextReads, ContextSignatures, Page, RenderGlobals,
        TemplateType,
    },
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

/// The filesystem mutations a build resolved to, computed without touching the
/// filesystem so the work can run off the write lock.
///
/// Deletions are named explicitly rather than derived at apply time from "everything
/// the plan did not mention". A plan that visits only part of the site is then applied
/// by the same code as a plan that visits all of it, instead of pruning every output it
/// did not look at.
#[derive(Debug, Default)]
pub struct WritePlan {
    dirs: Vec<PathBuf>,
    writes: Vec<(PathBuf, Vec<u8>, u64)>,
    /// Outputs that rendered to what is already on disk. They keep their cache entry
    /// without carrying their bytes, which is what keeps peak memory at the size of
    /// what actually changed.
    unchanged: Vec<(PathBuf, u64)>,
    deletes: Vec<PathBuf>,
}

/// What one page's last render depended on. A render reads nothing but its own
/// template, the shared partials and the context, so a page whose inputs all sign
/// the same renders identically and need not run again.
#[derive(Debug)]
struct RenderRecord {
    partials: u64,
    template: u64,
    /// The object a template page renders over; zero for a page without one.
    overlay: u64,
    deps: Vec<(ContextDep, u64)>,
    output: u64,
}

impl RenderRecord {
    /// `build_cache` is consulted rather than trusted: this cache says what a render
    /// would produce, and only the build cache says what is actually on disk.
    fn reusable(
        &self,
        partials: u64,
        template: u64,
        overlay: u64,
        signatures: &ContextSignatures,
        build_cache: &RwLock<HashMap<PathBuf, u64>>,
        path: &PathBuf,
    ) -> bool {
        self.partials == partials
            && self.template == template
            && self.overlay == overlay
            && build_cache.read().unwrap().get(path) == Some(&self.output)
            && self
                .deps
                .iter()
                .all(|(dep, signed)| signatures.of(dep) == Some(*signed))
    }
}

fn signed_reads(
    reads: ContextReads,
    signatures: &ContextSignatures,
) -> Option<Vec<(ContextDep, u64)>> {
    reads
        .take()
        .into_iter()
        .map(|dep| signatures.of(&dep).map(|signed| (dep, signed)))
        .collect()
}

fn hash_source(source: &str) -> u64 {
    let mut hasher = SeaHasher::new();
    hasher.write(source.as_bytes());
    hasher.finish()
}

impl WritePlan {
    /// Files this plan accounts for, whether or not it writes them.
    fn visited(&self) -> HashSet<&PathBuf> {
        self.writes
            .iter()
            .map(|(path, _, _)| path)
            .chain(self.unchanged.iter().map(|(path, _)| path))
            .collect()
    }

    fn record(
        &mut self,
        cache: &RwLock<HashMap<PathBuf, u64>>,
        dir: Option<PathBuf>,
        path: PathBuf,
        contents: Vec<u8>,
        hash: u64,
    ) {
        if cache.read().unwrap().get(&path) == Some(&hash) {
            self.unchanged.push((path, hash));
            return;
        }
        if let Some(dir) = dir {
            self.dirs.push(dir);
        }
        self.writes.push((path, contents, hash));
    }

    /// Accounts for a page whose inputs are unchanged, so it was never rendered.
    /// The caller has already established that `cache` holds `hash` for `path`.
    fn record_reused(&mut self, path: PathBuf, hash: u64) {
        self.unchanged.push((path, hash));
    }

    /// Everything previously written that this plan did not account for.
    fn delete_unvisited(&mut self, cache: &RwLock<HashMap<PathBuf, u64>>) {
        let visited = self.visited();
        let stale: Vec<PathBuf> = cache
            .read()
            .unwrap()
            .keys()
            .filter(|key| !visited.contains(key))
            .cloned()
            .collect();
        self.deletes.extend(stale);
    }

    /// Applies the plan, updating `cache` as each write lands. A failure part-way
    /// through therefore leaves the cache describing exactly what reached the
    /// filesystem, rather than claiming content that was never written - which would
    /// make the next build skip those pages for good.
    pub(crate) fn apply<T: FileSystemAPI>(
        self,
        fs: &mut T,
        cache: &RwLock<HashMap<PathBuf, u64>>,
    ) -> Result<()> {
        for dir in &self.dirs {
            fs.create_dir_all(dir)?;
        }
        let mut cache = cache.write().unwrap();
        for (path, contents, hash) in self.writes {
            fs.write(&path, contents)?;
            cache.insert(path, hash);
        }
        for (path, hash) in self.unchanged {
            cache.insert(path, hash);
        }
        for path in self.deletes {
            if fs.exists(&path)? {
                fs.delete(&path)?;
            }
            cache.remove(&path);
        }
        Ok(())
    }
}

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
    #[serde(skip)]
    render_cache: RwLock<HashMap<PathBuf, RenderRecord>>,
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
            render_cache: RwLock::new(HashMap::new()),
        })
    }

    /// Returns a parser for the site's current partials, reusing the cached
    /// one when no partial or layout has changed since it was built.
    fn get_or_build_parser<T: FileSystemAPI>(
        &self,
        pages_dir: &Path,
        layout_dir: Option<&Path>,
        fs: &T,
    ) -> Result<(std::sync::Arc<liquid::Parser>, u64)> {
        let _span = trace_span!("get_or_build_parser").entered();
        let (source, partials_hash) =
            liquid_parser::partials_hash(Some(pages_dir), layout_dir, fs)?;
        if let Some(cache) = self.parser_cache.read().unwrap().as_ref() {
            if cache.partials_hash == partials_hash {
                return Ok((cache.parser.clone(), partials_hash));
            }
        }
        let parser = std::sync::Arc::new(liquid_parser::build_with_partials(source)?);
        *self.parser_cache.write().unwrap() = Some(ParserCache {
            partials_hash,
            parser: parser.clone(),
            templates: HashMap::new(),
        });
        Ok((parser, partials_hash))
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

    pub(crate) fn build_cache(&self) -> &RwLock<HashMap<PathBuf, u64>> {
        &self.build_cache
    }

    pub(crate) fn static_file_cache(&self) -> &RwLock<HashMap<PathBuf, u64>> {
        &self.static_file_cache
    }

    pub fn build_id(&self) -> u64 {
        let mut hasher = SeaHasher::new();
        let hashes = self.build_cache.read().unwrap();
        if hashes.is_empty() {
            return 0;
        }
        // Sorted, and over paths as well as hashes: a HashMap iterates in an order
        // derived from its own random state, so hashing values in map order gives a
        // different id for identical output every time the map is replaced.
        let mut entries: Vec<(&PathBuf, &u64)> = hashes.iter().collect();
        entries.sort_unstable_by_key(|(path, _)| *path);
        for (path, hash) in entries {
            hasher.write(path.as_os_str().as_encoded_bytes());
            hasher.write(&hash.to_ne_bytes());
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
        self.get_objects_at(fs, self.objects_generation())
    }

    /// `get_objects` against a filesystem that was current at `generation`.
    pub(crate) fn get_objects_at<T: FileSystemAPI>(
        &self,
        fs: &T,
        generation: u64,
    ) -> Result<ObjectMap> {
        self.get_objects_sorted_at(
            fs,
            Some(|a: &_, b: &_| get_order(a).partial_cmp(&get_order(b)).unwrap()),
            generation,
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
        // The bump happens under the object cache's own guard: a plan reading a stale
        // snapshot checks the generation while holding that guard before it caches a
        // parse, so the two cannot interleave. build_cache is deliberately untouched -
        // site.build() needs it to know which outputs to remove.
        let mut cache = self.obj_cache.write().unwrap();
        cache.remove(file);
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
        let generation = self.objects_generation();
        let mut cache = self.obj_cache.write().unwrap();
        if let Some(filename) = filename {
            let root_path = self.path_for_object(object_name, None);
            if filename == object_name && fs.exists(&root_path)? {
                return self.object_for_path(&root_path, object_def, &mut cache, fs, generation);
            }
        }
        let path = self.path_for_object(object_name, filename);
        self.object_for_path(&path, object_def, &mut cache, fs, generation)
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
        self.get_objects_sorted_at(fs, sort, self.objects_generation())
    }

    fn get_objects_sorted_at<T: FileSystemAPI>(
        &self,
        fs: &T,
        sort: Option<impl Fn(&Object, &Object) -> Ordering>,
        generation: u64,
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
                    match self.object_for_path(&path, object_def, &mut cache, fs, generation) {
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
                match self.object_for_path(
                    &object_file_path,
                    object_def,
                    &mut cache,
                    fs,
                    generation,
                ) {
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
    /// `generation` is the object generation the `fs` being read was current at. A parse
    /// from an older one is returned but not cached: the invalidation that moved the
    /// generation already dropped this entry, and nothing would drop it again.
    fn object_for_path<T: FileSystemAPI>(
        &self,
        path: &Path,
        object_def: &ObjectDefinition,
        cache: &mut HashMap<PathBuf, Object>,
        fs: &T,
        generation: u64,
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
            if self.cache_generation.load(atomic::Ordering::Relaxed) == generation {
                cache.insert(path.to_path_buf(), o.clone());
            }
            Ok(o)
        }
    }

    #[instrument(skip(fs))]
    pub fn sync_static_files<T: FileSystemAPI>(&self, fs: &mut T) -> Result<()> {
        let plan = self.plan_static_sync(&*fs)?;
        plan.apply(fs, &self.static_file_cache)
    }

    /// Resolves the static copy to the writes and deletes it implies, reading only
    /// `static_dir`.
    ///
    /// The cache is keyed by destination, like `build_cache`: a key relative to
    /// `static_dir` names a build output only once joined onto `build_dir`, and using
    /// one where the other belongs deletes the source a static file shadows.
    pub fn plan_static_sync<T: FileSystemAPI>(&self, fs: &T) -> Result<WritePlan> {
        let Manifest {
            static_dir,
            build_dir,
            ..
        } = &self.manifest;
        let mut plan = WritePlan::default();
        if !fs.exists(build_dir)? {
            plan.dirs.push(build_dir.to_owned());
        }
        #[cfg(feature = "verbose-logging")]
        debug!("copying files from {}", static_dir.display());
        if fs.exists(static_dir)? {
            for file in fs.walk_dir(static_dir, false)? {
                let from = static_dir.join(&file);
                if let Some(content) = fs.read(&from)? {
                    let hash = hash_file(&content);
                    let dest = build_dir.join(&file);
                    let dir = dest
                        .parent()
                        .filter(|parent| *parent != build_dir)
                        .map(|parent| parent.to_path_buf());
                    plan.record(&self.static_file_cache, dir, dest, content, hash);
                }
            }
        } else {
            debug!("static dir {} does not exist.", static_dir.display());
        }
        plan.delete_unvisited(&self.static_file_cache);
        Ok(plan)
    }

    #[instrument(skip(fs))]
    pub fn build<T: FileSystemAPI>(&self, fs: &mut T, options: BuildOptions) -> Result<()> {
        let plan = self.plan_build(&*fs, options, self.objects_generation())?;
        plan.apply(fs, &self.build_cache)
    }

    /// Resolves a build to the filesystem mutations it implies, without performing any
    /// of them. Reads only `objects_dir`, `pages_dir` and `layout_dir`; everything it
    /// produces lands under `build_dir`, which `Manifest::validate_build_dir` keeps
    /// disjoint from all three - so this can run against a snapshot taken before the
    /// write lock was released. `generation` is the object generation that snapshot was
    /// current at; parses from an older one are used but not cached.
    #[instrument(skip(fs))]
    pub fn plan_build<T: FileSystemAPI>(
        &self,
        fs: &T,
        options: BuildOptions,
        generation: u64,
    ) -> Result<WritePlan> {
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

        let mut plan = WritePlan::default();

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
            plan.dirs.push(build_dir.to_owned());
        }

        let all_objects = self.get_objects_at(fs, generation)?;

        let (liquid_parser, partials_hash) = self.get_or_build_parser(
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
        let (base_context, signatures) = build_context(
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
                            let result = self
                                .plan_template_page(
                                    object,
                                    object_def,
                                    &parsed_template,
                                    &template_path,
                                    hash_source(&template_str),
                                    partials_hash,
                                    build_dir,
                                    &base_context,
                                    &signatures,
                                    &liquid_parser,
                                    &mut plan,
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
                            result?;
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
                        .plan_page(
                            &rel_path,
                            &file_path,
                            page_name,
                            page_type,
                            partials_hash,
                            build_dir,
                            &base_context,
                            &signatures,
                            fs,
                            &liquid_parser,
                            &mut plan,
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
                    result?;
                }
            }
        }

        plan.delete_unvisited(&self.build_cache);
        Ok(plan)
    }

    #[instrument(skip(self, template, base_context, signatures, liquid_parser, plan))]
    #[allow(clippy::too_many_arguments)]
    fn plan_template_page(
        &self,
        object: &Object,
        object_def: &ObjectDefinition,
        template: &liquid::Template,
        template_path: &PathBuf,
        template_hash: u64,
        partials_hash: u64,
        build_dir: &Path,
        base_context: &liquid::Object,
        signatures: &ContextSignatures,
        liquid_parser: &liquid::Parser,
        plan: &mut WritePlan,
    ) -> Result<PathBuf> {
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
        let render_name = format!("{}.{}", object.filename, page.extension());
        let t_dir = build_dir.join(&object_def.name);
        let build_path = t_dir.join(render_name);
        let overlay = signatures
            .object(&object_def.name, &object.filename)
            .unwrap_or_default();
        if let Some(record) = self.render_cache.read().unwrap().get(&build_path) {
            if record.reusable(
                partials_hash,
                template_hash,
                overlay,
                signatures,
                &self.build_cache,
                &build_path,
            ) {
                plan.record_reused(build_path.clone(), record.output);
                return Ok(build_path);
            }
        }
        let reads = ContextReads::default();
        let render_o = page.render(liquid_parser, base_context, &self.field_config, &reads);
        if render_o.is_err() {
            warn!("failed rendering {}", object.filename);
        }
        let rendered = layout::post_process(render_o?);
        let hash = hash_file(rendered.as_bytes());
        if let Some(deps) = signed_reads(reads, signatures) {
            self.render_cache.write().unwrap().insert(
                build_path.clone(),
                RenderRecord {
                    partials: partials_hash,
                    template: template_hash,
                    overlay,
                    deps,
                    output: hash,
                },
            );
        }
        plan.record(
            &self.build_cache,
            Some(t_dir),
            build_path.clone(),
            rendered.into_bytes(),
            hash,
        );
        Ok(build_path)
    }

    #[instrument(skip(self, base_context, signatures, fs, liquid_parser, plan))]
    #[allow(clippy::too_many_arguments)]
    fn plan_page<T: FileSystemAPI>(
        &self,
        rel_path: &PathBuf,
        file_path: &PathBuf,
        page_name: &str,
        page_type: TemplateType,
        partials_hash: u64,
        build_dir: &Path,
        base_context: &liquid::Object,
        signatures: &ContextSignatures,
        fs: &T,
        liquid_parser: &liquid::Parser,
        plan: &mut WritePlan,
    ) -> Result<Option<PathBuf>> {
        let field_config = &self.field_config;
        let Some(template_str) = fs.read_to_string(file_path)? else {
            warn!("page not found: {}", file_path.display());
            return Ok(None);
        };
        let mut render_dir = build_dir.to_path_buf();
        let mut nested_dir = None;
        if let Some(parent_dir) = rel_path.parent() {
            render_dir = render_dir.join(parent_dir);
            nested_dir = Some(render_dir.clone());
        }
        let render_path = render_dir.join(format!("{}.{}", page_name, page_type.extension()));
        let template_hash = hash_source(&template_str);
        if let Some(record) = self.render_cache.read().unwrap().get(&render_path) {
            if record.reusable(
                partials_hash,
                template_hash,
                0,
                signatures,
                &self.build_cache,
                &render_path,
            ) {
                plan.record_reused(render_path.clone(), record.output);
                return Ok(Some(render_path));
            }
        }
        let template = self.get_or_parse_template(liquid_parser, &template_str)?;
        let page = Page::new_with_parsed_content(
            page_name.to_string(),
            &template,
            TemplateType::Default,
            file_path,
        );
        let reads = ContextReads::default();
        let render_o = page.render(liquid_parser, base_context, field_config, &reads);
        if render_o.is_err() {
            warn!("failed rendering {}", file_path.display());
        }
        let rendered = layout::post_process(render_o?);
        let hash = hash_file(rendered.as_bytes());
        if let Some(deps) = signed_reads(reads, signatures) {
            self.render_cache.write().unwrap().insert(
                render_path.clone(),
                RenderRecord {
                    partials: partials_hash,
                    template: template_hash,
                    overlay: 0,
                    deps,
                    output: hash,
                },
            );
        }
        plan.record(
            &self.build_cache,
            nested_dir,
            render_path.clone(),
            rendered.into_bytes(),
            hash,
        );
        Ok(Some(render_path))
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

    /// A static file's cache key is relative to `static_dir`, so deleting it has to
    /// be resolved against `build_dir`. A key that also names a real source file is
    /// what makes getting this wrong destructive rather than merely ineffective.
    #[test]
    fn removing_a_static_file_does_not_delete_the_source_it_shadows() -> Result<()> {
        let mut fs = MemoryFileSystem::default();
        fs.write_str(
            Path::new(OBJECT_DEFINITION_FILE_NAME),
            "[post]\nname = \"string\"\n".to_string(),
        )?;
        let source = Path::new("objects/post/a-post.toml");
        fs.write_str(source, "name = \"A Post\"\n".to_string())?;
        // Shadows the source path once `public/` is stripped.
        let shadowing_static = Path::new("public/objects/post/a-post.toml");
        fs.write_str(shadowing_static, "static\n".to_string())?;
        fs.write_str(Path::new("pages/index.liquid"), "index\n".to_string())?;

        let site = Site::load(&fs, Some("test"))?;
        site.sync_static_files(&mut fs)?;
        let copied = site.manifest.build_dir.join("objects/post/a-post.toml");
        assert!(
            fs.exists(&copied)?,
            "static file was not copied into the build"
        );

        fs.delete(shadowing_static)?;
        site.sync_static_files(&mut fs)?;

        assert!(
            fs.exists(source)?,
            "removing a static file deleted the source file it shadowed"
        );
        assert!(
            !fs.exists(&copied)?,
            "removed static file was left behind in the build"
        );
        Ok(())
    }

    /// `build_cache` is replaced wholesale by every build, and a fresh `HashMap`
    /// iterates in an order derived from its own random state - so an id derived from
    /// map order changes even when nothing about the output did.
    #[test]
    fn build_id_is_stable_across_builds_with_identical_output() -> Result<()> {
        // Enough outputs that two fresh HashMaps almost certainly iterate differently;
        // with two or three entries the orders coincide often enough to pass by luck.
        let mut fs = MemoryFileSystem::default();
        fs.write_str(
            Path::new(OBJECT_DEFINITION_FILE_NAME),
            "[post]\ntemplate = \"post\"\nname = \"string\"\n".to_string(),
        )?;
        for i in 0..24 {
            fs.write_str(
                Path::new(&format!("objects/post/post-{i}.toml")),
                format!("name = \"Post {i}\"\n"),
            )?;
        }
        fs.write_str(
            Path::new("pages/post.liquid"),
            "{{ post.name }}\n".to_string(),
        )?;
        fs.write_str(Path::new("pages/index.liquid"), "index\n".to_string())?;
        let site = Site::load(&fs, Some("test"))?;
        site.build(&mut fs, BuildOptions::default())?;
        let first = site.build_id();
        site.build(&mut fs, BuildOptions::default())?;
        assert_eq!(
            first,
            site.build_id(),
            "build id moved despite identical output"
        );
        Ok(())
    }

    /// A build plans against a snapshot taken before it released the write lock. If an
    /// edit lands in that window, the plan parses the pre-edit bytes - and must not leave
    /// that parse in the live object cache, which nothing would invalidate a second time.
    #[test]
    fn a_plan_from_a_stale_snapshot_does_not_poison_the_object_cache() -> Result<()> {
        let mut fs = MemoryFileSystem::default();
        fs.write_str(
            Path::new(OBJECT_DEFINITION_FILE_NAME),
            "[post]\ntemplate = \"post\"\nname = \"string\"\n".to_string(),
        )?;
        let object = Path::new("objects/post/a-post.toml");
        fs.write_str(object, "name = \"Original\"\n".to_string())?;
        fs.write_str(
            Path::new("pages/post.liquid"),
            "{{ post.name }}\n".to_string(),
        )?;
        fs.write_str(Path::new("pages/index.liquid"), "index\n".to_string())?;

        let site = Site::load(&fs, Some("test"))?;
        site.build(&mut fs, BuildOptions::default())?;

        // A build takes its snapshot here and releases the lock.
        let snapshot = fs.snapshot().expect("memory filesystems snapshot");
        let generation = site.objects_generation();

        // An edit lands while that build is still planning.
        fs.write_str(object, "name = \"Edited\"\n".to_string())?;
        site.invalidate_file(object);

        // The in-flight build plans against bytes that are now stale.
        let _ = site.plan_build(&snapshot, BuildOptions::default(), generation)?;

        // The next build reads the live filesystem, and must not be handed the pre-edit
        // parse the stale plan just went through.
        site.build(&mut fs, BuildOptions::default())?;
        let rendered = fs
            .read_to_string(site.manifest.build_dir.join("post").join("a-post.html"))?
            .expect("page was not built");
        assert!(
            rendered.contains("Edited"),
            "a stale snapshot's parse was served from the object cache: {rendered}"
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

#[cfg(test)]
mod incremental_render {
    use crate::{Archival, BuildOptions, FileSystemAPI, MemoryFileSystem};
    use std::path::Path;

    /// Two articles, a per-article template page, an index that embeds both, and a
    /// layout that reads the `site` object - so every kind of dependency a page can
    /// have on the shared context is represented.
    fn site() -> MemoryFileSystem {
        let mut fs = MemoryFileSystem::default();
        fs.write_str("archival.toml", "upload_prefix = \"\"\n".to_string())
            .unwrap();
        fs.write_str(
            "archival_objects.toml",
            "[site]\nname = \"string\"\n\n[articles]\ntemplate = \"article\"\nheadline = \"string\"\n"
                .to_string(),
        )
        .unwrap();
        fs.write_str("objects/site.toml", "name = \"First\"\n".to_string())
            .unwrap();
        for (index, headline) in ["One", "Two"].iter().enumerate() {
            fs.write_str(
                format!("objects/articles/article-{index}.toml"),
                format!("headline = \"{headline}\"\norder = {index}\n"),
            )
            .unwrap();
        }
        fs.write_str(
            "pages/article.liquid",
            "{% layout 'theme' %}<h1>{{ articles.headline }}</h1>\n".to_string(),
        )
        .unwrap();
        fs.write_str(
            "pages/index.liquid",
            "{% layout 'theme' %}{% for a in articles %}<li>{{ a.headline }}</li>{% endfor %}\n"
                .to_string(),
        )
        .unwrap();
        fs.write_str(
            "layout/theme.liquid",
            "<html><title>{{ objects.site.name }}</title>{{ page_content }}</html>\n".to_string(),
        )
        .unwrap();
        fs
    }

    fn built(archival: &Archival<MemoryFileSystem>, path: &str) -> String {
        archival.fs_read_file(path).unwrap()
    }

    fn rewrite(archival: &Archival<MemoryFileSystem>, path: &str, contents: &str) {
        archival.fs_write_file(path, contents.to_string()).unwrap();
        archival.site.invalidate_file(Path::new(path));
    }

    /// The page whose object changed picks the change up, and so does the index that
    /// reads every article - while the untouched article's page keeps its content.
    #[test]
    fn an_object_edit_reaches_every_page_that_reads_it() {
        let archival = Archival::new(site()).unwrap();
        archival.build(BuildOptions::default()).unwrap();
        assert!(built(&archival, "dist/articles/article-1.html").contains("Two"));

        rewrite(
            &archival,
            "objects/articles/article-1.toml",
            "headline = \"Rewritten\"\norder = 1\n",
        );
        archival.build(BuildOptions::default()).unwrap();

        assert!(built(&archival, "dist/articles/article-1.html").contains("Rewritten"));
        assert!(built(&archival, "dist/index.html").contains("Rewritten"));
        assert!(built(&archival, "dist/articles/article-0.html").contains("One"));
    }

    /// The layout reads `objects.site`, so every page depends on it even though no
    /// page names it directly.
    #[test]
    fn an_edit_to_an_object_the_layout_reads_reaches_every_page() {
        let archival = Archival::new(site()).unwrap();
        archival.build(BuildOptions::default()).unwrap();

        rewrite(&archival, "objects/site.toml", "name = \"Renamed\"\n");
        archival.build(BuildOptions::default()).unwrap();

        for page in [
            "dist/index.html",
            "dist/articles/article-0.html",
            "dist/articles/article-1.html",
        ] {
            assert!(
                built(&archival, page).contains("Renamed"),
                "{page} kept a stale layout"
            );
        }
    }

    /// A layout is a partial rather than a context read, so it is the parser's hash
    /// that has to invalidate every page.
    #[test]
    fn a_layout_edit_reaches_every_page() {
        let archival = Archival::new(site()).unwrap();
        archival.build(BuildOptions::default()).unwrap();

        rewrite(
            &archival,
            "layout/theme.liquid",
            "<html><title>{{ objects.site.name }}</title><main>{{ page_content }}</main></html>\n",
        );
        archival.build(BuildOptions::default()).unwrap();

        for page in [
            "dist/index.html",
            "dist/articles/article-0.html",
            "dist/articles/article-1.html",
        ] {
            assert!(
                built(&archival, page).contains("<main>"),
                "{page} kept a stale layout"
            );
        }
    }

    /// A template edit changes every page rendered through it, and nothing else.
    #[test]
    fn a_template_edit_reaches_the_pages_rendered_through_it() {
        let archival = Archival::new(site()).unwrap();
        archival.build(BuildOptions::default()).unwrap();

        rewrite(
            &archival,
            "pages/article.liquid",
            "{% layout 'theme' %}<h2>{{ articles.headline }}</h2>\n",
        );
        archival.build(BuildOptions::default()).unwrap();

        assert!(built(&archival, "dist/articles/article-0.html").contains("<h2>One</h2>"));
        assert!(built(&archival, "dist/articles/article-1.html").contains("<h2>Two</h2>"));
    }

    /// Rebuilding with nothing changed leaves every page exactly as it was.
    #[test]
    fn an_idle_rebuild_changes_nothing() {
        let archival = Archival::new(site()).unwrap();
        archival.build(BuildOptions::default()).unwrap();
        let before: Vec<String> = ["dist/index.html", "dist/articles/article-0.html"]
            .iter()
            .map(|p| built(&archival, p))
            .collect();

        archival.build(BuildOptions::default()).unwrap();

        let after: Vec<String> = ["dist/index.html", "dist/articles/article-0.html"]
            .iter()
            .map(|p| built(&archival, p))
            .collect();
        assert_eq!(before, after);
    }
}
