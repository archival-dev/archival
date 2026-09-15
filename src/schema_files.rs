//! Tests for the JSON Schema files checked in at the repo root
//! (`archival.schema.json`, `archival_objects.schema.json`,
//! `archival_editor.schema.json` and `archival_template.schema.json`).
//!
//! These files are published to <https://archival.dev/schemas/> and registered
//! with SchemaStore, so editors hand them to users as autocompletion and
//! validation for their `*.toml` config files. Nothing at runtime reads them,
//! which means they can drift from the parsers without anything failing — and
//! they had, badly: the manifest schema declared `pages_dir`/`objects_dir`/
//! `object_definition_file`, none of which [`Manifest::from_table`] has ever
//! read, while omitting `metadata` entirely. Combined with
//! `additionalProperties: false` that flagged correct manifests as errors and
//! silently accepted keys archival ignores.
//!
//! So: the `*_matches_*` tests below tie each schema back to the code that
//! actually parses the file, and the fixture tests run every schema against
//! `tests/schemas/{positive,negative}`. Adding a variant to [`ManifestField`] or
//! [`FieldType`] breaks an exhaustive match here until the schema is updated too.
//!
//! `archival_editor.schema.json` has no counterpart in this crate — the types it
//! describes live in the (private) archival-editor repo. It is covered by the
//! fixtures here, and archival-editor's own CI validates its fixtures against
//! this file so the two cannot drift apart.

use crate::fields::FieldType;
use crate::manifest::ManifestField;
use crate::reserved_fields::RESERVED_FIELDS;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const CANONICAL_ID_PREFIX: &str = "https://archival.dev/schemas/";
const DRAFT_07: &str = "http://json-schema.org/draft-07/schema#";

/// Every schema file published from this repo, paired with the constant that
/// embeds it. Going through [`crate::schemas`] rather than re-reading the files
/// means these tests check what dependents actually get.
const SCHEMAS: [(&str, &str); 4] = [
    ("archival.schema.json", crate::schemas::ARCHIVAL),
    (
        "archival_objects.schema.json",
        crate::schemas::ARCHIVAL_OBJECTS,
    ),
    (
        "archival_editor.schema.json",
        crate::schemas::ARCHIVAL_EDITOR,
    ),
    (
        "archival_template.schema.json",
        crate::schemas::ARCHIVAL_TEMPLATE,
    ),
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn schema(name: &str) -> Value {
    let (_, contents) = SCHEMAS
        .iter()
        .find(|(schema_name, _)| *schema_name == name)
        .unwrap_or_else(|| panic!("{name} is not in SCHEMAS"));
    serde_json::from_str(contents).unwrap_or_else(|e| panic!("parsing {name}: {e}"))
}

fn string_set(value: Option<&Value>, context: &str) -> BTreeSet<String> {
    value
        .unwrap_or_else(|| panic!("{context} is missing"))
        .as_array()
        .unwrap_or_else(|| panic!("{context} is not an array"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("{context} contains a non-string"))
                .to_string()
        })
        .collect()
}

fn property_names(schema: &Value, context: &str) -> BTreeSet<String> {
    schema
        .get("properties")
        .unwrap_or_else(|| panic!("{context} has no properties"))
        .as_object()
        .unwrap_or_else(|| panic!("{context} properties is not an object"))
        .keys()
        .cloned()
        .collect()
}

/// Every [`ManifestField`], with an exhaustive match so that adding a variant
/// fails to compile until it is listed here (and, via
/// `manifest_schema_matches_parser`, in archival.schema.json).
fn all_manifest_fields() -> Vec<ManifestField> {
    let all = vec![
        ManifestField::UploadPrefix,
        ManifestField::ArchivalVersion,
        ManifestField::SiteUrl,
        ManifestField::SiteName,
        ManifestField::ObjectDefinitionFile,
        ManifestField::ObjectsDir,
        ManifestField::Prebuild,
        ManifestField::PagesDir,
        ManifestField::BuildDir,
        ManifestField::StaticDir,
        ManifestField::SchemasDir,
        ManifestField::LayoutDir,
        ManifestField::UploadsUrl,
        ManifestField::EditorTypes,
        ManifestField::Metadata,
    ];
    for field in &all {
        match field {
            ManifestField::UploadPrefix
            | ManifestField::ArchivalVersion
            | ManifestField::SiteUrl
            | ManifestField::SiteName
            | ManifestField::ObjectDefinitionFile
            | ManifestField::ObjectsDir
            | ManifestField::Prebuild
            | ManifestField::PagesDir
            | ManifestField::BuildDir
            | ManifestField::StaticDir
            | ManifestField::SchemasDir
            | ManifestField::LayoutDir
            | ManifestField::UploadsUrl
            | ManifestField::EditorTypes
            | ManifestField::Metadata => {}
        }
    }
    all
}

/// The archival types a user may name as a bare string in archival_objects.toml
/// or as an editor type's `type` in archival.toml. Enum, oneof and alias field types have
/// no spelling of their own (enums and oneofs are written as TOML arrays, and an
/// alias is spelled as the custom type's name), so they contribute nothing here
/// — but the match is exhaustive, so a new variant forces a decision.
fn archival_field_type_names() -> BTreeSet<String> {
    let all = [
        FieldType::String,
        FieldType::Secret,
        FieldType::Number,
        FieldType::Date,
        FieldType::Markdown,
        FieldType::Boolean,
        FieldType::Image,
        FieldType::Video,
        FieldType::Audio,
        FieldType::Upload,
        FieldType::Meta,
    ];
    for field_type in &all {
        match field_type {
            FieldType::String
            | FieldType::Secret
            | FieldType::Number
            | FieldType::Date
            | FieldType::Markdown
            | FieldType::Boolean
            | FieldType::Image
            | FieldType::Video
            | FieldType::Audio
            | FieldType::Upload
            | FieldType::Meta => {}
            // Not nameable as a bare string: see above.
            FieldType::Enum(_) | FieldType::Oneof(_) | FieldType::Alias(_) => {}
        }
    }
    all.iter().map(|t| t.as_str().to_string()).collect()
}

#[test]
fn schemas_are_published_under_the_canonical_id() {
    for (name, _) in SCHEMAS {
        let schema = schema(name);
        assert_eq!(
            schema.get("$id").and_then(|v| v.as_str()),
            Some(format!("{CANONICAL_ID_PREFIX}{name}").as_str()),
            "{name} has the wrong $id. SchemaStore resolves $ref against it and \
             editors key their cache on it, so it must be the URL the file is \
             actually served from."
        );
        assert_eq!(
            schema.get("$schema").and_then(|v| v.as_str()),
            Some(DRAFT_07),
            "{name} must declare draft-07 — SchemaStore does not yet accept later drafts."
        );
    }
}

/// The config filenames a schema is allowed to name. The two archival reads come
/// from the constants; the other two belong to archival-editor, which is not
/// renaming them.
fn current_config_filenames() -> BTreeSet<String> {
    BTreeSet::from([
        crate::constants::MANIFEST_FILE_NAME.to_string(),
        crate::constants::OBJECT_DEFINITION_FILE_NAME.to_string(),
        "archival_editor.toml".to_string(),
        "archival_template.toml".to_string(),
    ])
}

/// Renaming a config file has to reach into the schemas, and it is easy to miss:
/// the filenames appear in `description` strings, which are prose that nothing
/// else validates but which editors show to users as hover text. One is not even
/// prose — archival.schema.json's `object_file` default is the actual default
/// archival uses.
///
/// So rather than trusting a grep, every `*.toml` a schema mentions must be a
/// filename archival currently reads.
#[test]
fn schemas_name_only_current_config_files() {
    let filename = regex::Regex::new(r"\b[a-z_]+\.toml\b").expect("valid regex");
    let allowed = current_config_filenames();
    for (name, contents) in SCHEMAS {
        for found in filename.find_iter(contents) {
            let found = found.as_str();
            assert!(
                allowed.contains(found),
                "{name} mentions {found}, which is not a config file archival reads \
                 (expected one of {allowed:?}). Editors show these strings to users \
                 as hover text, so a rename has to update them too."
            );
        }
    }

    // Not prose: this is the default archival falls back to for `object_file`.
    assert_eq!(
        schema("archival.schema.json")
            .pointer("/properties/object_file/default")
            .and_then(|v| v.as_str()),
        Some(crate::constants::OBJECT_DEFINITION_FILE_NAME),
    );
}

#[test]
fn manifest_schema_matches_parser() {
    let schema = schema("archival.schema.json");
    let declared = property_names(&schema, "archival.schema.json");
    let parsed: BTreeSet<String> = all_manifest_fields()
        .iter()
        .map(|f| f.field_name().to_string())
        .collect();
    assert_eq!(
        declared, parsed,
        "archival.schema.json properties must be exactly the keys \
         Manifest::from_table reads. The schema sets additionalProperties: false, \
         so a key here that the parser ignores is a silent no-op for the user, and \
         a key the parser reads but the schema omits is reported as an error in \
         their editor."
    );
}

#[test]
fn objects_schema_forbids_the_reserved_fields() {
    let schema = schema("archival_objects.schema.json");
    let reserved: BTreeSet<String> = RESERVED_FIELDS.iter().map(|f| f.to_string()).collect();

    assert_eq!(
        string_set(
            schema.pointer("/definitions/objectName/not/enum"),
            "archival_objects.schema.json objectName",
        ),
        reserved,
        "archival_objects.schema.json must forbid exactly the names in RESERVED_FIELDS as \
         object names — ObjectDefinition::new rejects all of them."
    );

    // `template` is reserved as a field, but `template = "<page>"` is how an
    // object names the page it renders with, so it stays legal as a key.
    let mut allowed_as_field = reserved.clone();
    allowed_as_field.remove(crate::reserved_fields::TEMPLATE);
    assert_eq!(
        string_set(
            schema.pointer("/definitions/fieldName/not/enum"),
            "archival_objects.schema.json fieldName",
        ),
        allowed_as_field,
        "archival_objects.schema.json must forbid the reserved names as field names, except \
         `template`."
    );
}

#[test]
fn field_type_enums_match_field_type() {
    let expected = archival_field_type_names();
    for name in ["archival_objects.schema.json", "archival.schema.json"] {
        let schema = schema(name);
        let declared = string_set(
            schema.pointer("/definitions/archivalFieldType/enum"),
            &format!("{name} archivalFieldType"),
        );
        assert_eq!(
            declared, expected,
            "{name} lists the wrong archival types. These are the values editors \
             offer as completions, so they must match FieldType::from_str."
        );
    }
}

fn fixture_files(kind: &str, schema_name: &str) -> Vec<PathBuf> {
    let stem = schema_name.trim_end_matches(".schema.json");
    let dir = repo_root().join("tests/schemas").join(kind).join(stem);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {}", dir.display(), e))
        .map(|entry| entry.expect("reading dir entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no {kind} fixtures for {schema_name} in {}",
        dir.display()
    );
    files
}

fn fixture_json(path: &Path) -> Value {
    let contents = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("reading {}: {}", path.display(), e));
    let table: toml::Table =
        toml::from_str(&contents).unwrap_or_else(|e| panic!("parsing {}: {}", path.display(), e));
    serde_json::to_value(table)
        .unwrap_or_else(|e| panic!("converting {} to json: {}", path.display(), e))
}

#[test]
fn positive_fixtures_validate() {
    for (name, _) in SCHEMAS {
        let schema = schema(name);
        for path in fixture_files("positive", name) {
            let instance = fixture_json(&path);
            if let Err(error) = jsonschema::validate(&schema, &instance) {
                panic!(
                    "{} should validate against {name} but did not: {error}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn negative_fixtures_are_rejected() {
    for (name, _) in SCHEMAS {
        let schema = schema(name);
        for path in fixture_files("negative", name) {
            let instance = fixture_json(&path);
            assert!(
                jsonschema::validate(&schema, &instance).is_err(),
                "{} is a negative fixture but validated against {name}",
                path.display()
            );
        }
    }
}

/// The fixtures above are hand-written; this checks the schemas against the real
/// site files in `tests/fixtures`.
///
/// Every config file under there is validated, whatever it is named and however
/// deep it sits, rather than a hardcoded path — one of these sites is unzipped
/// from `archival-website.zip` into a gitignored directory, so a fixed path is
/// either missing on a fresh checkout or silently stops matching when a fixture
/// moves. The legacy names are included on purpose: archival still reads them, so
/// the schemas still have to accept what is in those files.
#[test]
fn repo_fixtures_validate() {
    use crate::constants::{
        LEGACY_MANIFEST_FILE_NAME, LEGACY_OBJECT_DEFINITION_FILE_NAME, MANIFEST_FILE_NAME,
        OBJECT_DEFINITION_FILE_NAME,
    };

    fn config_files(dir: &Path, found: &mut Vec<(PathBuf, &'static str)>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => panic!("reading {}: {}", dir.display(), e),
        };
        for entry in entries {
            let path = entry.expect("reading dir entry").path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "tmp") {
                    continue;
                }
                config_files(&path, found);
                continue;
            }
            let schema_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) if name == MANIFEST_FILE_NAME || name == LEGACY_MANIFEST_FILE_NAME => {
                    "archival.schema.json"
                }
                Some(name)
                    if name == OBJECT_DEFINITION_FILE_NAME
                        || name == LEGACY_OBJECT_DEFINITION_FILE_NAME =>
                {
                    "archival_objects.schema.json"
                }
                _ => continue,
            };
            found.push((path, schema_name));
        }
    }

    let mut found = vec![];
    config_files(&repo_root().join("tests/fixtures"), &mut found);
    assert!(
        !found.is_empty(),
        "no site config files found under tests/fixtures — this test has quietly \
         become a no-op"
    );
    for (path, schema_name) in found {
        let instance = fixture_json(&path);
        if let Err(error) = jsonschema::validate(&schema(schema_name), &instance) {
            panic!(
                "this repo's own {} does not validate against {schema_name}: {error}",
                path.display()
            );
        }
    }
}
