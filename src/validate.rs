//! Checking a site without building it.
//!
//! Everything here runs against the parsers compiled into this binary, so what
//! it reports is what the archival building the site will accept. That is the
//! same promise the language server makes, and it is the same code: the server
//! renders these diagnostics into LSP ones rather than computing its own.
//!
//! It is deliberately stricter than `build` in two places, because both are
//! things a green build hides:
//!
//! - object values are checked, where `Site::load` parses them with validation
//!   skipped;
//! - a file archival would log and step over is reported, where a build carries
//!   on without it and renders a page with the value missing.

use crate::diagnostic::{Code, Diagnostic, Report, Span};
use crate::fields::InvalidFieldError;
use crate::file_system::FileSystemAPI;
use crate::manifest::{EditorTypes, Manifest};
use crate::object::Object;
use crate::object_definition::{ObjectDefinition, ObjectDefinitions};
use crate::reserved_fields::{is_reserved_field, ReservedFieldError, ORDER, RESERVED_FIELDS};
use std::ops::Range;
use std::path::{Path, PathBuf};
use toml_edit::{Item, Table};

/// What archival will tell a person who wrote a reserved name. Kept beside the
/// rule rather than in whatever is reading the diagnostic, so there is one
/// place to correct it.
const RESERVED_HELP: &str = "archival populates these itself and a definition cannot declare them. Rename it and keep everything it holds.";
const TYPES_HELP: &str = "field types are string, markdown, number, boolean, date, image, video, audio, upload, meta and secret, plus any editor type archival.toml declares. A definition holds types, never content.";
const ONEOF_HELP: &str = "a oneof is an array of tables and every table needs both `name` and `type`. A list of child objects is one table - `[type.child]` holding field types, not `[[type.child]]`.";
const ENUM_HELP: &str = "an enum is an array of strings.";

/// Checks `archival.toml`.
pub fn manifest(source: &str, root: &Path, file: impl AsRef<Path>) -> Vec<Diagnostic> {
    let file = file.as_ref();
    if let Some(syntax) = toml_syntax(source, file) {
        return vec![syntax];
    }
    // The upload prefix is supplied by whoever is building, not by the file.
    match Manifest::from_string(root, source.to_string(), Some("")) {
        Ok(_) => vec![],
        Err(err) => vec![located(
            Code::InvalidManifest,
            source,
            file,
            err.to_string(),
            None,
        )],
    }
}

/// Checks the object definitions file.
///
/// Custom types come from the manifest, so a definition using one is only
/// understood when the manifest alongside it loaded - which is why this takes
/// `editor_types` rather than reading them itself.
pub fn definitions(
    source: &str,
    editor_types: &EditorTypes,
    file: impl AsRef<Path>,
) -> Vec<Diagnostic> {
    let file = file.as_ref();
    if let Some(syntax) = toml_syntax(source, file) {
        return vec![syntax];
    }
    if ObjectDefinition::from_source(source, editor_types).is_ok() {
        return vec![];
    }
    // Narrow rather than restate. `ObjectDefinition::new` stops at the first
    // mistake, so a file with three costs three round trips to fix; running the
    // real parser over one table, and then over one key at a time, finds each
    // of them without a second copy of the rules to drift from.
    let Ok(table) = toml::from_str::<toml::Table>(source) else {
        return vec![];
    };
    let comments = crate::definition_comments::extract_comments(source).unwrap_or_default();
    let mut found = vec![];
    for (name, value) in &table {
        let Some(definition) = value.as_table() else {
            continue;
        };
        let whole = ObjectDefinition::new(name, definition, comments.child(name), editor_types);
        let Err(err) = whole else {
            continue;
        };
        // A reserved object name is not about any one key, and the narrowing
        // pass below would report it once per field.
        if matches!(
            err.downcast_ref::<InvalidFieldError>(),
            Some(InvalidFieldError::ReservedObjectNameError(_))
        ) {
            found.push(definition_error(source, file, &err).about(Some(name.clone()), None));
            continue;
        }
        let mut per_key = vec![];
        for (key, one) in definition {
            let mut alone = toml::Table::new();
            alone.insert(key.clone(), one.clone());
            if let Err(err) =
                ObjectDefinition::new(name, &alone, comments.child(name), editor_types)
            {
                per_key.push(
                    definition_error(source, file, &err)
                        .about(Some(name.clone()), Some(key.clone())),
                );
            }
        }
        // A table that only fails as a whole - a reserved object name, or a
        // rule no single key breaks - keeps its own error.
        if per_key.is_empty() {
            found.push(definition_error(source, file, &err).about(Some(name.clone()), None));
        } else {
            found.extend(per_key);
        }
    }
    if found.is_empty() {
        // Something outside any table, which the narrowing pass cannot reach.
        if let Err(err) = ObjectDefinition::from_source(source, editor_types) {
            found.push(definition_error(source, file, &err));
        }
    }
    found
}

/// Checks an object file against the definition it is an instance of.
pub fn object(
    source: &str,
    path: &Path,
    file: impl AsRef<Path>,
    definition: &ObjectDefinition,
    editor_types: &EditorTypes,
) -> Vec<Diagnostic> {
    let file = file.as_ref();
    let table: toml::Table = match toml::from_str(source) {
        Ok(table) => table,
        Err(err) => return vec![toml_error(source, file, &err)],
    };
    let mut found = unknown_fields(source, file, definition, editor_types);
    found.extend(non_numeric_order(source, file, &table));
    // Archival stops at the first bad value, so this adds at most one more.
    if let Err(err) = Object::from_table(definition, path, &table, editor_types, false) {
        found.push(value_error(source, file, &err, &definition.name));
    }
    found
}

/// Everything wrong with the site rooted at `root`.
///
/// A site whose manifest or definitions do not parse is reported and not walked
/// further: every object check needs a definition to check against, and
/// reporting each of them as "unknown type" would bury the one error that
/// matters.
pub fn site<F: FileSystemAPI>(fs: &F, root: &Path) -> Report {
    let mut found = vec![];
    let manifest_name = crate::constants::MANIFEST_FILE_NAME;

    let loaded = match read(fs, Path::new(manifest_name)) {
        Some(source) => {
            found.extend(manifest(&source, root, manifest_name));
            Manifest::from_string(root, source, Some("")).ok()
        }
        // A site with no manifest is legal; the defaults apply.
        None => Some(Manifest::default(root, "")),
    };
    let Some(manifest) = loaded else {
        return Report::new(found);
    };

    // Manifest paths are rooted; a report names files the way a person typed
    // them, so a stored result does not carry whoever ran it.
    let definitions_name = relative(root, &manifest.object_definition_file);
    let Some(source) = read(fs, &definitions_name) else {
        return Report::new(found);
    };
    found.extend(definitions(
        &source,
        &manifest.editor_types,
        &definitions_name,
    ));
    let Ok(defined) = ObjectDefinition::from_source(&source, &manifest.editor_types) else {
        return Report::new(found);
    };

    found.extend(objects_in(
        fs,
        root,
        &relative(root, &manifest.objects_dir),
        &defined,
        &manifest.editor_types,
    ));
    Report::new(found)
}

/// Every object file under `objects_dir`, checked against its type.
fn objects_in<F: FileSystemAPI>(
    fs: &F,
    root: &Path,
    objects_dir: &Path,
    defined: &ObjectDefinitions,
    editor_types: &EditorTypes,
) -> Vec<Diagnostic> {
    let mut found = vec![];
    for (name, definition) in defined {
        // One of a kind at `<dir>/<name>.toml`, many at `<dir>/<name>/*.toml`.
        let single = objects_dir.join(format!("{name}.toml"));
        if let Some(source) = read(fs, &single) {
            found.extend(object(
                &source,
                &root.join(&single),
                &single,
                definition,
                editor_types,
            ));
        }
        let listed = objects_dir.join(name);
        // `list_dir` answers paths relative to the filesystem's own root, which
        // is where every other archival path in a site is measured from.
        let Ok(entries) = fs.list_dir(&listed, false) else {
            continue;
        };
        for relative in entries {
            if relative.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Some(source) = read(fs, &relative) else {
                continue;
            };
            found.extend(object(
                &source,
                &root.join(&relative),
                &relative,
                definition,
                editor_types,
            ));
        }
    }
    found
}

fn read<F: FileSystemAPI>(fs: &F, path: &Path) -> Option<String> {
    fs.read_to_string(path).ok().flatten()
}

/// A manifest path as the site names it, rather than as this machine does.
fn relative(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

fn toml_syntax(source: &str, file: &Path) -> Option<Diagnostic> {
    toml::from_str::<toml::Table>(source)
        .err()
        .map(|err| toml_error(source, file, &err))
}

fn toml_error(source: &str, file: &Path, err: &toml::de::Error) -> Diagnostic {
    let span = err.span().unwrap_or(0..source.len());
    Diagnostic::new(Code::TomlSyntax, file, err.message().to_string()).at(Span::new(source, span))
}

/// Classify a definitions failure, so a reader can key off the rule rather than
/// off the sentence. `ObjectDefinition::from_source` answers `anyhow`, so the
/// concrete error is recovered by downcast; one it does not know is still
/// reported, as `InvalidDefinitions`.
fn definition_error(source: &str, file: &Path, err: &anyhow::Error) -> Diagnostic {
    let message = err.to_string();
    if let Some(reserved) = err.downcast_ref::<ReservedFieldError>() {
        return located(
            Code::ReservedFieldName,
            source,
            file,
            message,
            Some(reserved.field),
        )
        .with_help(RESERVED_HELP)
        .about(None, Some(reserved.field.to_string()));
    }
    let Some(field) = err.downcast_ref::<InvalidFieldError>() else {
        return located(Code::InvalidDefinitions, source, file, message, None);
    };
    if let InvalidFieldError::UnrecognizedType(named) = field {
        // The error names the type, which is the *value* written in the file;
        // the field it was written under is what a reader wants pointed at.
        let span = span_of_value(source, named)
            .map(|span| Span::new(source, span))
            .unwrap_or_else(|| Span::whole(source));
        return Diagnostic::new(Code::UnrecognizedType, file, message)
            .at(span)
            .with_help(TYPES_HELP)
            .about(None, field_named(source, named));
    }
    let (code, key, help) = match field {
        InvalidFieldError::ReservedObjectNameError(name) => (
            Code::ReservedObjectName,
            Some(name.as_str()),
            Some(RESERVED_HELP),
        ),
        InvalidFieldError::InvalidOneof(_) => (Code::InvalidOneof, None, Some(ONEOF_HELP)),
        InvalidFieldError::InvalidEnum(_) => (Code::InvalidEnum, None, Some(ENUM_HELP)),
        InvalidFieldError::TypeMismatch { field, .. } => {
            (Code::TypeMismatch, Some(field.as_str()), None)
        }
        InvalidFieldError::InvalidChild { key, .. } => {
            (Code::InvalidChild, Some(key.as_str()), None)
        }
        _ => (Code::InvalidDefinitions, None, None),
    };
    let found = located(code, source, file, message, key);
    match help {
        Some(help) => found.with_help(help),
        None => found,
    }
}

/// Classify a failure to read an object's values against its definition.
fn value_error(source: &str, file: &Path, err: &anyhow::Error, object: &str) -> Diagnostic {
    let message = err.to_string();
    let Some(field) = err.downcast_ref::<InvalidFieldError>() else {
        return located(Code::InvalidObject, source, file, message, None)
            .about(Some(object.to_string()), None);
    };
    let (code, key) = match field {
        InvalidFieldError::TypeMismatch { field, .. } => (Code::TypeMismatch, Some(field.clone())),
        InvalidFieldError::OneofMismatch { field, .. } => (Code::InvalidOneof, Some(field.clone())),
        InvalidFieldError::EnumMismatch { field, .. } => (Code::InvalidEnum, Some(field.clone())),
        InvalidFieldError::InvalidChild { key, .. } => (Code::InvalidChild, Some(key.clone())),
        InvalidFieldError::NotAnArray { key, .. } => (Code::TypeMismatch, Some(key.clone())),
        _ => (Code::InvalidObject, None),
    };
    located(code, source, file, message, key.as_deref()).about(Some(object.to_string()), key)
}

/// A diagnostic pointed at `key` where the source has one, and at the whole
/// file where it does not - a name an error mentions is not always written in
/// the file it is about.
fn located(
    code: Code,
    source: &str,
    file: &Path,
    message: String,
    key: Option<&str>,
) -> Diagnostic {
    let span = key
        .and_then(|key| span_of_key(source, key))
        .map(|span| Span::new(source, span))
        .unwrap_or_else(|| Span::whole(source));
    Diagnostic::new(code, file, message).at(span)
}

/// The span of the first value written as `value`, for an error that names
/// what was written rather than where.
fn span_of_value(source: &str, value: &str) -> Option<Range<usize>> {
    let quoted = format!("\"{value}\"");
    let at = source.find(&quoted)?;
    Some(at..at + quoted.len())
}

/// The key a value was written under, where the source has one.
fn field_named(source: &str, value: &str) -> Option<String> {
    let at = source.find(&format!("\"{value}\""))?;
    let line_start = source[..at].rfind('\n').map_or(0, |n| n + 1);
    let line = &source[line_start..at];
    let name = line.split('=').next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn span_of_key(source: &str, key: &str) -> Option<Range<usize>> {
    if key.is_empty() {
        return None;
    }
    let parsed = toml_edit::Document::<&str>::parse(source).ok()?;
    find_key(parsed.as_table(), key)
}

fn find_key(table: &Table, name: &str) -> Option<Range<usize>> {
    for (key_name, item) in table.iter() {
        let (key, _) = table.get_key_value(key_name)?;
        if key_name == name {
            return key.span();
        }
        let nested = child_tables(item)
            .into_iter()
            .find_map(|child| find_key(child, name));
        if nested.is_some() {
            return nested;
        }
    }
    None
}

pub(crate) fn child_tables(item: &Item) -> Vec<&Table> {
    match item {
        Item::ArrayOfTables(tables) => tables.iter().collect(),
        Item::Table(table) => vec![table],
        _ => vec![],
    }
}

/// `order` sorts objects within their directory, and archival ignores it when
/// it is not a number, leaving the object in filename order instead.
fn non_numeric_order(source: &str, file: &Path, table: &toml::Table) -> Option<Diagnostic> {
    let value = table.get(ORDER)?;
    if value.is_integer() || value.is_float() {
        return None;
    }
    Some(located(
        Code::NonNumericOrder,
        source,
        file,
        format!("`order` must be a number, not {}.", value.type_str()),
        Some(ORDER),
    ))
}

/// Keys that are neither a field nor a child of the definition. Archival only
/// logs these, so they would otherwise be silent: the value is dropped and the
/// template renders nothing.
fn unknown_fields(
    source: &str,
    file: &Path,
    definition: &ObjectDefinition,
    editor_types: &EditorTypes,
) -> Vec<Diagnostic> {
    let Ok(parsed) = toml_edit::Document::<&str>::parse(source) else {
        return vec![];
    };
    let mut found = vec![];
    walk(
        source,
        file,
        parsed.as_table(),
        definition,
        editor_types,
        &mut found,
    );
    found
}

fn walk(
    source: &str,
    file: &Path,
    table: &Table,
    definition: &ObjectDefinition,
    editor_types: &EditorTypes,
    found: &mut Vec<Diagnostic>,
) {
    for (name, item) in table.iter() {
        let Some((key, _)) = table.get_key_value(name) else {
            continue;
        };
        let resolved = editor_types
            .get(name)
            .map(|alias| &alias.alias_of[..])
            .unwrap_or(name);
        if let Some(child) = definition.children.get(resolved) {
            // Children are arrays of tables; a mismatch there is reported by
            // archival's own parse rather than here.
            for entry in child_tables(item) {
                walk(source, file, entry, child, editor_types, found);
            }
            continue;
        }
        if definition.field_type(resolved).is_some() || is_reserved_field(resolved) {
            continue;
        }
        let Some(span) = key.span() else { continue };
        found.push(
            Diagnostic::new(
                Code::UnknownField,
                file,
                format!(
                    "`{name}` is not defined on `{}`. Known fields: {}.",
                    definition.name,
                    known_fields(definition)
                ),
            )
            .at(Span::new(source, span))
            .about(Some(definition.name.clone()), Some(name.to_string())),
        );
    }
}

fn known_fields(definition: &ObjectDefinition) -> String {
    let mut names: Vec<&str> = definition
        .fields
        .iter()
        .filter(|(_, field)| !matches!(field.r#type, crate::fields::FieldType::Secret))
        .map(|(name, _)| &name[..])
        .chain(definition.children.keys().map(|name| &name[..]))
        .collect();
    names.sort_unstable();
    if names.is_empty() {
        return "none".to_string();
    }
    names.join(", ")
}

/// The names a definition may not use, for a reader that wants to explain the
/// rule rather than repeat the message.
pub fn reserved_names() -> &'static [&'static str; 6] {
    &RESERVED_FIELDS
}

/// Where a site is rooted, for a caller holding only a path inside one.
pub fn site_root(dir: &Path) -> Option<PathBuf> {
    let markers = [
        crate::constants::MANIFEST_FILE_NAME,
        crate::constants::OBJECT_DEFINITION_FILE_NAME,
    ];
    dir.ancestors()
        .find(|candidate| markers.iter().any(|marker| candidate.join(marker).exists()))
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::Severity;

    fn defs(source: &str) -> Vec<Diagnostic> {
        definitions(source, &EditorTypes::default(), "archival_objects.toml")
    }

    fn only(found: &[Diagnostic]) -> &Diagnostic {
        assert_eq!(found.len(), 1, "{found:#?}");
        &found[0]
    }

    #[test]
    fn valid_definitions_report_nothing() {
        assert!(defs("[post]\ntitle = \"string\"\nbody = \"markdown\"\n").is_empty());
    }

    /// The error a run actually makes: a reserved name declared as a field.
    #[test]
    fn a_reserved_field_is_named_and_located() {
        let found = defs("[site]\nname = \"string\"\norder = \"number\"\n");
        let one = only(&found);
        assert_eq!(one.code, Code::ReservedFieldName);
        assert_eq!(one.field.as_deref(), Some("order"));
        assert_eq!(one.span.as_ref().map(|s| s.line), Some(3));
        assert!(one.help.is_some());
    }

    /// Fixing one mistake at a time costs a round trip each, and for anything
    /// automating archival that round trip is the expensive part.
    #[test]
    fn every_mistake_in_a_file_is_reported_at_once() {
        let found = defs("[site]\ntitle = \"FRIENDO\"\norder = \"number\"\nlogo = \"picture\"\n");
        assert_eq!(found.len(), 3, "{found:#?}");
        let codes: Vec<Code> = found.iter().map(|one| one.code).collect();
        assert!(codes.contains(&Code::ReservedFieldName), "{codes:?}");
        assert_eq!(
            codes
                .iter()
                .filter(|code| **code == Code::UnrecognizedType)
                .count(),
            2,
            "{found:#?}"
        );
        // Each is pointed at its own line rather than at the file.
        let lines: Vec<u32> = found
            .iter()
            .filter_map(|one| one.span.as_ref().map(|span| span.line))
            .collect();
        assert_eq!(lines, vec![2, 3, 4], "{found:#?}");
    }

    /// Narrowing finds nothing when no single key is at fault, so the table's
    /// own error has to survive.
    #[test]
    fn a_table_that_only_fails_as_a_whole_keeps_its_error() {
        let found = defs("[order]\ntitle = \"string\"\n");
        let one = only(&found);
        assert_eq!(one.code, Code::ReservedObjectName);
        assert_eq!(one.object.as_deref(), Some("order"));
    }

    #[test]
    fn a_reserved_object_name_is_reported_once_whatever_it_holds() {
        let found = defs("[page]\ntitle = \"string\"\nslug = \"string\"\nbody = \"markdown\"\n");
        let one = only(&found);
        assert_eq!(one.code, Code::ReservedObjectName);
        assert_eq!(one.object.as_deref(), Some("page"));
        assert_eq!(one.field, None);
    }

    /// A child list written as an array of tables reads as a malformed oneof,
    /// so the help has to name the syntax that was meant.
    #[test]
    fn the_oneof_help_names_the_child_list_syntax() {
        let found = defs("[site]\n[[site.nav]]\nlabel = \"string\"\nurl = \"string\"\n");
        let one = only(&found);
        assert_eq!(one.code, Code::InvalidOneof);
        let help = one.help.as_deref().unwrap_or_default();
        assert!(help.contains("[type.child]"), "{help}");
    }

    #[test]
    fn a_mistake_names_the_object_it_is_in() {
        let found = defs("[post]\nbody = \"prose\"\n");
        let one = only(&found);
        assert_eq!(one.object.as_deref(), Some("post"));
        assert_eq!(one.field.as_deref(), Some("body"));
    }

    #[test]
    fn a_reserved_object_name_is_located() {
        let found = defs("[order]\ntitle = \"string\"\n");
        let one = only(&found);
        assert_eq!(one.code, Code::ReservedObjectName);
        assert_eq!(one.span.as_ref().map(|s| s.line), Some(1));
    }

    /// A definitions file holding content where a type belongs - the other
    /// error a run actually makes.
    #[test]
    fn content_written_as_a_type_is_an_unrecognized_type() {
        let found = defs("[site]\ntitle = \"FRIENDO\"\n");
        let one = only(&found);
        assert_eq!(one.code, Code::UnrecognizedType);
        assert!(one.message.contains("FRIENDO"), "{}", one.message);
        assert!(one
            .help
            .as_ref()
            .is_some_and(|h| h.contains("never content")));
    }

    #[test]
    fn a_type_archival_does_not_have_is_named() {
        assert_eq!(
            only(&defs("[post]\nmedia = \"oneof\"\n")).code,
            Code::UnrecognizedType
        );
    }

    #[test]
    fn a_malformed_oneof_says_what_a_oneof_needs() {
        let found = defs("[[post.media]]\nname = \"image\"\n[[post.media]]\ntype = \"video\"\n");
        let one = only(&found);
        assert_eq!(one.code, Code::InvalidOneof);
        assert!(one.help.as_ref().is_some_and(|h| h.contains("`name`")));
    }

    #[test]
    fn text_that_is_not_toml_is_a_syntax_error() {
        let found = defs("[post\ntitle = \"string\"\n");
        assert_eq!(only(&found).code, Code::TomlSyntax);
    }

    /// A custom type is only recognized when the manifest declaring it loaded,
    /// which is why the corpus of real sites needs the manifest beside the
    /// definitions.
    #[test]
    fn an_editor_type_resolves_only_with_its_manifest() {
        let source = "[post]\nthumb = \"hero\"\n";
        assert_eq!(only(&defs(source)).code, Code::UnrecognizedType);

        let manifest = Manifest::from_string(
            Path::new("/site"),
            "[editor_types.hero]\ntype = \"image\"\n".to_string(),
            Some(""),
        )
        .unwrap();
        assert!(definitions(source, &manifest.editor_types, "archival_objects.toml").is_empty());
    }

    #[test]
    fn a_report_knows_whether_anything_stops_a_build() {
        assert!(!Report::new(vec![]).has_errors());
        assert!(Report::new(defs("[site]\norder = \"number\"\n")).has_errors());
    }

    #[test]
    fn severity_separates_a_broken_site_from_a_surprising_one() {
        let one = &defs("[site]\norder = \"number\"\n")[0];
        assert_eq!(one.severity, Severity::Error);
    }
}
