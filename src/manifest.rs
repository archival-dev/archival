use anyhow::Result;
use ordermap::OrderMap;
use regex::Regex;
use serde::{de::Visitor, Deserialize, Deserializer, Serialize, Serializer};
use std::{
    fmt::{self, Display},
    hash::Hash,
    ops::Deref,
    path::{Path, PathBuf},
};
use toml::{Table, Value};

use crate::{
    constants::{LAYOUT_DIR_NAME, NESTED_TYPES, SCHEMAS_DIR_NAME},
    file_system::FileSystemAPI,
    object::ValuePath,
};

use super::constants::{
    BUILD_DIR_NAME, LEGACY_MANIFEST_FILE_NAME, LEGACY_OBJECT_DEFINITION_FILE_NAME,
    MANIFEST_FILE_NAME, OBJECTS_DIR_NAME, OBJECT_DEFINITION_FILE_NAME, PAGES_DIR_NAME,
    STATIC_DIR_NAME,
};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum InvalidManifestError {
    #[error("Invalid Site Path")]
    InvalidSitePath,
    #[error("Failed Parsing Manifest File")]
    FailedParsing,
    #[error("Manifest Field {0} was of an unrecognized type.")]
    BadType(String),
    #[error("Validator {0} was not a valid regular expression ({1}).")]
    InvalidValidator(String, String),
    #[error("Manifest Missing Required Field: {0}")]
    MissingRequired(String),
    #[error("Bad Path '{1}' for Field {0}")]
    BadPath(Value, String),
    #[error("Cannot define a nested validator for type {0} ({1})")]
    InvalidNestedValidator(String, String),
    #[error("build_dir ({0}) overlaps {1} ({2}); a build must not read what it writes")]
    BuildDirOverlapsSource(String, String, String),
    #[error("Invalid Manifest value '{1}' for field {0}.")]
    InvalidField(Value, String),
    #[error("Invalid Metadata value '{1}' for field {0}.")]
    InvalidMetadata(Value, String),
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct Validator(#[cfg_attr(feature = "typescript", type_def(type_of = "String"))] Regex);

impl PartialEq for Validator {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Hash for Validator {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.as_str().hash(state);
    }
}

impl Validator {
    pub fn new(regex: &str, name: &str) -> Result<Self, InvalidManifestError> {
        Ok(Self(Regex::new(regex).map_err(|e| {
            InvalidManifestError::InvalidValidator(name.to_string(), e.to_string())
        })?))
    }
    pub fn validate(&self, input: &str) -> bool {
        self.0.is_match(input)
    }
}

impl Display for Validator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_str())
    }
}

impl Deref for Validator {
    type Target = Regex;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Serialize for Validator {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.0.as_str())
    }
}
struct ValidatorVisitor;
impl Visitor<'_> for ValidatorVisitor {
    type Value = Validator;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a regular expression")
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        let re = Regex::new(v).map_err(|e| E::custom(format!("Invalid Regex {}: {}", v, e)))?;
        Ok(Validator(re))
    }
}
impl<'de> Deserialize<'de> for Validator {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(ValidatorVisitor)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Hash)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct ManifestEditorTypePathValidator {
    pub path: ValuePath,
    pub validate: Validator,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Hash)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub enum ManifestEditorTypeValidator {
    Value(Validator),
    Path(ManifestEditorTypePathValidator),
}

impl Display for ManifestEditorTypeValidator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Path(p) => {
                    format!("{}: {}", p.path, p.validate)
                }
                Self::Value(v) => v.to_string(),
            }
        )
    }
}

impl From<&ManifestEditorTypeValidator> for toml::Value {
    fn from(value: &ManifestEditorTypeValidator) -> Self {
        match value {
            ManifestEditorTypeValidator::Value(v) => toml::Value::String(v.to_string()),
            ManifestEditorTypeValidator::Path(p) => {
                let mut map = toml::map::Map::new();
                map.insert("path".to_string(), toml::Value::String(p.path.to_string()));
                map.insert(
                    "validate".to_string(),
                    toml::Value::String(p.validate.to_string()),
                );
                map.into()
            }
        }
    }
}
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Hash)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct ManifestEditorType {
    pub alias_of: String,
    pub validate: Vec<ManifestEditorTypeValidator>,
    pub editor_url: String,
}

impl From<&ManifestEditorType> for toml::Value {
    fn from(value: &ManifestEditorType) -> Self {
        let mut map = toml::map::Map::new();
        map.insert("type".into(), value.alias_of.to_string().into());
        map.insert(
            "validate".into(),
            toml::Value::Array(value.validate.iter().map(|v| v.into()).collect()),
        );
        if !value.editor_url.is_empty() {
            map.insert("editor_url".into(), value.editor_url.to_string().into());
        }
        map.into()
    }
}

#[cfg(feature = "typescript")]
mod typedefs {
    use typescript_type_def::{
        type_expr::{Ident, NativeTypeInfo, TypeExpr, TypeInfo, TypeName, TypeUnion},
        TypeDef,
    };

    use crate::manifest::ManifestEditorType;
    pub struct EditorTypesDef;
    impl TypeDef for EditorTypesDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            r#ref: TypeExpr::Name(TypeName {
                path: &[],
                name: Ident("Record"),
                generic_args: &[
                    TypeExpr::Ref(&String::INFO),
                    TypeExpr::Ref(&ManifestEditorType::INFO),
                ],
            }),
        });
    }
    pub struct MetadataTypeDef;
    impl TypeDef for MetadataTypeDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            r#ref: TypeExpr::Union(TypeUnion {
                docs: None,
                members: &[
                    TypeExpr::Name(TypeName {
                        path: &[],
                        name: Ident("Record"),
                        generic_args: &[TypeExpr::Ref(&String::INFO), TypeExpr::Ref(&String::INFO)],
                    }),
                    TypeExpr::ident(Ident("null")),
                ],
            }),
        });
    }
}

pub type EditorTypes = OrderMap<String, ManifestEditorType>;
pub type MetadataType = OrderMap<String, String>;

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Hash)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct Manifest {
    #[serde(skip)]
    root: PathBuf,
    pub upload_prefix: String,
    pub archival_version: Option<String>,
    pub prebuild: Vec<String>,
    pub postbuild: Vec<String>,
    pub site_name: Option<String>,
    pub site_url: Option<String>,
    pub object_definition_file: PathBuf,
    pub schemas_dir: PathBuf,
    pub pages_dir: PathBuf,
    pub objects_dir: PathBuf,
    pub build_dir: PathBuf,
    pub static_dir: PathBuf,
    pub layout_dir: PathBuf,
    pub uploads_url: Option<String>,
    #[cfg_attr(feature = "typescript", type_def(type_of = "typedefs::EditorTypesDef"))]
    pub editor_types: EditorTypes,
    #[cfg_attr(
        feature = "typescript",
        type_def(type_of = "typedefs::MetadataTypeDef")
    )]
    pub metadata: Option<MetadataType>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "binary", derive(clap::ValueEnum))]
#[cfg_attr(feature = "binary", value(rename_all = "snake_case"))]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub enum ManifestField {
    UploadPrefix,
    ArchivalVersion,
    SiteUrl,
    SiteName,
    ObjectDefinitionFile,
    ObjectsDir,
    Prebuild,
    Postbuild,
    PagesDir,
    BuildDir,
    StaticDir,
    SchemasDir,
    LayoutDir,
    UploadsUrl,
    EditorTypes,
    Metadata,
}

impl ManifestField {
    /// The key this field is read from and written to in archival.toml.
    pub(crate) fn field_name(&self) -> &str {
        match self {
            ManifestField::UploadPrefix => "upload_prefix",
            ManifestField::ArchivalVersion => "archival_version",
            ManifestField::SiteUrl => "site_url",
            ManifestField::SiteName => "site_name",
            ManifestField::ObjectDefinitionFile => "object_file",
            ManifestField::ObjectsDir => "objects",
            ManifestField::Prebuild => "prebuild",
            ManifestField::Postbuild => "postbuild",
            ManifestField::PagesDir => "pages",
            ManifestField::BuildDir => "build_dir",
            ManifestField::StaticDir => "static_dir",
            ManifestField::SchemasDir => "schemas_dir",
            ManifestField::LayoutDir => "layout_dir",
            ManifestField::UploadsUrl => "uploads_url",
            ManifestField::EditorTypes => "editor_types",
            ManifestField::Metadata => "metadata",
        }
    }
}

fn string_list(value: &Value) -> Vec<String> {
    value.as_array().map_or(vec![], |v| {
        v.iter()
            .map(|s| s.as_str().map_or("".to_string(), |s| s.to_string()))
            .collect()
    })
}

fn string_list_value(list: &[String]) -> Option<Value> {
    if list.is_empty() {
        None
    } else {
        Some(Value::Array(
            list.iter().map(|v| Value::String(v.to_string())).collect(),
        ))
    }
}

impl fmt::Display for Manifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            r#"
        upload_prefix: {}
        archival_version: {}
        site_url: {}
        uploads_url: {}
        object file: {}
        objects: {}
        pages: {}
        static files: {}
        layout dir: {}
        build dir: {}{}
        {}
        "#,
            self.upload_prefix,
            self.archival_version
                .as_ref()
                .unwrap_or(&"unknown".to_owned()),
            self.site_url.as_ref().unwrap_or(&"none".to_owned()),
            self.uploads_url.as_ref().unwrap_or(&"none".to_owned()),
            self.object_definition_file.display(),
            self.objects_dir.display(),
            self.pages_dir.display(),
            self.static_dir.display(),
            self.layout_dir.display(),
            self.build_dir.display(),
            self.metadata
                .as_ref()
                .map(|m| {
                    format!(
                        "\n\t\t\tmetadata:\t\t\t\t{}",
                        m.iter()
                            .map(|(k, v)| format!("{}:{}", k, v))
                            .collect::<Vec<_>>()
                            .join("\n\t\t\t\t")
                    )
                })
                .unwrap_or_default(),
            if !self.editor_types.is_empty() {
                format!(
                    "editor types:\n{}",
                    self.editor_types
                        .iter()
                        .map(|(tn, i)| {
                            format!(
                                "            {}: {}{}",
                                tn,
                                i.alias_of,
                                if i.validate.is_empty() {
                                    "".to_string()
                                } else {
                                    format!(
                                        " ({})",
                                        i.validate
                                            .iter()
                                            .map(|v| v.to_string())
                                            .collect::<Vec<String>>()
                                            .join(",")
                                    )
                                }
                            )
                        })
                        .collect::<Vec<String>>()
                        .join("\n")
                )
            } else {
                "".to_string()
            }
        )
    }
}

impl Manifest {
    /// The manifest file archival reads and writes for the site at `root`. The
    /// canonical name wins when it exists; the legacy name is only used when
    /// it's the site's only manifest, so writes land in the file the site
    /// already has rather than silently creating a second one.
    pub fn path_in(root: &Path, fs: &impl FileSystemAPI) -> Result<PathBuf> {
        let canonical = root.join(MANIFEST_FILE_NAME);
        if !fs.exists(&canonical)? {
            let legacy = root.join(LEGACY_MANIFEST_FILE_NAME);
            if fs.exists(&legacy)? {
                return Ok(legacy);
            }
        }
        Ok(canonical)
    }

    /// Points an unconfigured `object_definition_file` at the legacy
    /// `objects.toml` when that is the only object definition file present.
    /// A manifest that names the file explicitly is left alone.
    pub(crate) fn resolve_object_definition_file(&mut self, fs: &impl FileSystemAPI) -> Result<()> {
        if self.object_definition_file != self.root.join(OBJECT_DEFINITION_FILE_NAME)
            || fs.exists(&self.object_definition_file)?
        {
            return Ok(());
        }
        let legacy = self.root.join(LEGACY_OBJECT_DEFINITION_FILE_NAME);
        if fs.exists(&legacy)? {
            self.object_definition_file = legacy;
        }
        Ok(())
    }

    pub fn default(root: &Path, upload_prefix: &str) -> Manifest {
        Manifest {
            root: root.to_owned(),
            upload_prefix: upload_prefix.to_string(),
            archival_version: None,
            prebuild: vec![],
            postbuild: vec![],
            site_url: None,
            site_name: None,
            uploads_url: None,
            object_definition_file: root.join(OBJECT_DEFINITION_FILE_NAME),
            pages_dir: root.join(PAGES_DIR_NAME),
            objects_dir: root.join(OBJECTS_DIR_NAME),
            schemas_dir: root.join(SCHEMAS_DIR_NAME),
            build_dir: root.join(BUILD_DIR_NAME),
            static_dir: root.join(STATIC_DIR_NAME),
            layout_dir: root.join(LAYOUT_DIR_NAME),
            editor_types: EditorTypes::new(),
            metadata: None,
        }
    }
    fn is_default(&self, field: &ManifestField) -> bool {
        let str_value = self.field_as_string(field);
        match field {
            ManifestField::Prebuild | ManifestField::Postbuild => str_value == "[]",
            ManifestField::ObjectDefinitionFile => {
                // The legacy name counts as a default too, so formatting a
                // site that still uses objects.toml doesn't write out an
                // object_file line it never had.
                str_value
                    == self
                        .root
                        .join(OBJECT_DEFINITION_FILE_NAME)
                        .to_string_lossy()
                    || str_value
                        == self
                            .root
                            .join(LEGACY_OBJECT_DEFINITION_FILE_NAME)
                            .to_string_lossy()
            }
            ManifestField::ObjectsDir => {
                str_value == self.root.join(OBJECTS_DIR_NAME).to_string_lossy()
            }
            ManifestField::PagesDir => {
                str_value == self.root.join(PAGES_DIR_NAME).to_string_lossy()
            }
            ManifestField::BuildDir => {
                str_value == self.root.join(BUILD_DIR_NAME).to_string_lossy()
            }
            ManifestField::StaticDir => {
                str_value == self.root.join(STATIC_DIR_NAME).to_string_lossy()
            }
            ManifestField::LayoutDir => {
                str_value == self.root.join(LAYOUT_DIR_NAME).to_string_lossy()
            }
            ManifestField::SchemasDir => {
                str_value == self.root.join(SCHEMAS_DIR_NAME).to_string_lossy()
            }
            _ => str_value.is_empty(),
        }
    }
    pub fn from_string(
        root: &Path,
        string: String,
        upload_prefix: Option<&str>,
    ) -> Result<Manifest> {
        let mut manifest = Manifest::default(root, "" /* set below */);
        let values: Table = toml::from_str(&string)?;
        let path_or_err = |value: Value, field: &str| -> Result<PathBuf, InvalidManifestError> {
            if let Some(string) = value.as_str() {
                return Ok(root.join(string));
            }
            Err(InvalidManifestError::BadPath(value, field.to_string()))
        };
        // Required fields
        let upload_prefix = if let Some(upload_prefix) = upload_prefix {
            upload_prefix.to_string()
        } else if let Some(manifest_value) = values.get("upload_prefix") {
            manifest_value
                .as_str()
                .ok_or_else(|| {
                    InvalidManifestError::InvalidField(
                        manifest_value.clone(),
                        "upload_prefix".to_string(),
                    )
                })?
                .to_string()
        } else {
            String::default()
        };
        manifest.upload_prefix = upload_prefix;
        // Optional fields
        for (key, value) in values.into_iter() {
            match key.as_str() {
                "archival_version" => {
                    manifest.archival_version = value.as_str().map(|s| s.to_string())
                }
                "uploads_url" => manifest.uploads_url = value.as_str().map(|s| s.to_string()),
                "site_url" => manifest.site_url = value.as_str().map(|s| s.to_string()),
                "site_name" => manifest.site_name = value.as_str().map(|s| s.to_string()),
                "prebuild" => manifest.prebuild = string_list(&value),
                "postbuild" => manifest.postbuild = string_list(&value),
                "pages" => manifest.pages_dir = path_or_err(value, "pages")?,
                "objects" => manifest.objects_dir = path_or_err(value, "objects")?,
                "build_dir" => manifest.build_dir = path_or_err(value, "build_dir")?,
                "static_dir" => manifest.static_dir = path_or_err(value, "static_dir")?,
                "schemas_dir" => manifest.schemas_dir = path_or_err(value, "schemas_dir")?,
                "layout_dir" => manifest.layout_dir = path_or_err(value, "layout_dir")?,
                "object_file" => {
                    manifest.object_definition_file = path_or_err(value, "object_file")?
                }
                "editor_types" => manifest.parse_editor_types(value)?,
                "metadata" => manifest.parse_metadata(value)?,
                _ => {}
            }
        }
        manifest.validate_build_dir()?;
        Ok(manifest)
    }

    /// A build reads its source directories and writes only into `build_dir`, which is
    /// what lets the whole build be computed against a read-only snapshot. Nesting one
    /// inside the other breaks that: a static sync would copy the build into itself, and
    /// `NativeFileSystem::walk_dir` is lazy, so it observes those writes inside the same
    /// walk that is producing them.
    fn validate_build_dir(&self) -> Result<()> {
        for (name, dir) in [
            ("objects", &self.objects_dir),
            ("pages", &self.pages_dir),
            ("layout_dir", &self.layout_dir),
            ("static_dir", &self.static_dir),
        ] {
            if self.build_dir.starts_with(dir) || dir.starts_with(&self.build_dir) {
                return Err(InvalidManifestError::BuildDirOverlapsSource(
                    self.build_dir.to_string_lossy().to_string(),
                    name.to_string(),
                    dir.to_string_lossy().to_string(),
                )
                .into());
            }
        }
        Ok(())
    }

    pub fn from_file(
        manifest_path: &Path,
        fs: &impl FileSystemAPI,
        upload_prefix: Option<&str>,
    ) -> Result<Manifest> {
        let root = manifest_path
            .parent()
            .ok_or(InvalidManifestError::InvalidSitePath)?;
        let string = fs.read_to_string(manifest_path)?.unwrap_or_default();
        Manifest::from_string(root, string, upload_prefix)
    }

    fn toml_field(&self, field: &ManifestField) -> Option<Value> {
        match field {
            ManifestField::ArchivalVersion => self.archival_version.to_owned().map(Value::String),
            ManifestField::UploadPrefix => Some(Value::String(self.upload_prefix.to_owned())),
            ManifestField::SiteUrl => self.site_url.to_owned().map(Value::String),
            ManifestField::SiteName => self.site_name.to_owned().map(Value::String),
            ManifestField::ObjectDefinitionFile => Some(Value::String(
                self.object_definition_file.to_string_lossy().to_string(),
            )),
            ManifestField::UploadsUrl => self.uploads_url.to_owned().map(Value::String),
            ManifestField::Prebuild => string_list_value(&self.prebuild),
            ManifestField::Postbuild => string_list_value(&self.postbuild),
            ManifestField::ObjectsDir => Some(Value::String(
                self.objects_dir.to_string_lossy().to_string(),
            )),
            ManifestField::PagesDir => {
                Some(Value::String(self.pages_dir.to_string_lossy().to_string()))
            }
            ManifestField::BuildDir => {
                Some(Value::String(self.build_dir.to_string_lossy().to_string()))
            }
            ManifestField::StaticDir => {
                Some(Value::String(self.static_dir.to_string_lossy().to_string()))
            }
            ManifestField::SchemasDir => Some(Value::String(
                self.schemas_dir.to_string_lossy().to_string(),
            )),
            ManifestField::LayoutDir => {
                Some(Value::String(self.layout_dir.to_string_lossy().to_string()))
            }
            ManifestField::Metadata => self.metadata.as_ref().map(|metadata| {
                let mut map = toml::map::Map::new();
                for (key, v) in metadata {
                    map.insert(key.into(), Value::String(v.to_string()));
                }
                Value::Table(map)
            }),
            ManifestField::EditorTypes => {
                let mut map = toml::map::Map::new();
                for (type_name, type_val) in &self.editor_types {
                    map.insert(type_name.into(), type_val.into());
                }
                Some(Value::Table(map))
            }
        }
    }

    fn parse_metadata(&mut self, types: toml::Value) -> Result<(), InvalidManifestError> {
        let metadata_table = match types {
            toml::Value::Table(t) => t,
            _ => return Err(InvalidManifestError::FailedParsing),
        };
        let mut metadata = MetadataType::new();
        for (type_name, value) in metadata_table {
            if let Some(val) = value.as_str() {
                metadata.insert(type_name, val.to_string());
            } else {
                return Err(InvalidManifestError::InvalidMetadata(value, type_name));
            }
        }
        self.metadata = Some(metadata);
        Ok(())
    }

    fn parse_editor_types(&mut self, types: toml::Value) -> Result<(), InvalidManifestError> {
        let types = match types {
            toml::Value::Table(t) => t,
            _ => return Err(InvalidManifestError::FailedParsing),
        };
        let mut editor_types = EditorTypes::new();
        for (type_name, info) in types {
            let mut editor_type = ManifestEditorType::default();
            let info_map = match info {
                toml::Value::Table(t) => t,
                _ => return Err(InvalidManifestError::FailedParsing),
            };
            editor_type.alias_of = info_map
                .get("type")
                .ok_or_else(|| InvalidManifestError::MissingRequired(format!("{type_name}.type")))?
                .as_str()
                .ok_or_else(|| {
                    InvalidManifestError::MissingRequired(format!("{type_name}.type (string)"))
                })?
                .to_string();
            if let Some(editor_url) = info_map.get("editor_url") {
                editor_type.editor_url = editor_url
                    .as_str()
                    .ok_or_else(|| {
                        InvalidManifestError::InvalidField(
                            editor_url.to_owned(),
                            "editor_url".to_string(),
                        )
                    })?
                    .to_string();
            }
            if let Some(validator_val) = info_map.get("validate") {
                let is_nested_type = NESTED_TYPES.contains(&&editor_type.alias_of[..]);
                editor_type.validate = match validator_val {
                    toml::Value::Array(arr) => arr
                        .iter()
                        .map(|val| match val {
                            toml::Value::Table(t) => {
                                if !is_nested_type {
                                    return Err(InvalidManifestError::InvalidNestedValidator(
                                        editor_type.alias_of.to_string(),
                                        type_name.to_string(),
                                    ));
                                }
                                let path = ValuePath::from_string(
                                    t.get("path")
                                        .ok_or_else(|| {
                                            InvalidManifestError::MissingRequired(format!(
                                                "{type_name}.validate.path"
                                            ))
                                        })?
                                        .as_str()
                                        .ok_or_else(|| {
                                            InvalidManifestError::MissingRequired(format!(
                                                "{type_name}.validate.path (string)"
                                            ))
                                        })?,
                                );
                                let validate_string = t
                                    .get("validate")
                                    .ok_or_else(|| {
                                        InvalidManifestError::MissingRequired(format!(
                                            "{type_name}.validate.validate"
                                        ))
                                    })?
                                    .as_str()
                                    .ok_or_else(|| {
                                        InvalidManifestError::MissingRequired(format!(
                                            "{type_name}.validate.validate (string)"
                                        ))
                                    })?
                                    .to_string();
                                Ok(ManifestEditorTypeValidator::Path(
                                    ManifestEditorTypePathValidator {
                                        path,
                                        validate: Validator::new(&validate_string, &type_name)?,
                                    },
                                ))
                            }
                            toml::Value::String(s) => Ok(ManifestEditorTypeValidator::Value(
                                Validator::new(s, &type_name)?,
                            )),
                            _ => Err(InvalidManifestError::BadType("validate (item)".to_string())),
                        })
                        .collect::<Result<Vec<ManifestEditorTypeValidator>, _>>()?,
                    _ => return Err(InvalidManifestError::BadType("validate (root)".to_string())),
                };
            }
            editor_types.insert(type_name, editor_type);
        }
        self.editor_types = editor_types;
        Ok(())
    }

    pub fn set(&mut self, field: &ManifestField, value: String) {
        match field {
            ManifestField::ArchivalVersion => self.archival_version = Some(value),
            ManifestField::UploadPrefix => panic!("uploads prefix is not modifiable via events"),
            ManifestField::ObjectDefinitionFile => {
                self.object_definition_file = PathBuf::from(value)
            }
            ManifestField::SiteUrl => self.site_url = Some(value),
            ManifestField::SiteName => self.site_name = Some(value),
            ManifestField::UploadsUrl => self.uploads_url = Some(value),
            ManifestField::Prebuild => {
                panic!("Prebuild is not modifiable via events")
            }
            ManifestField::Postbuild => {
                panic!("Postbuild is not modifiable via events")
            }
            ManifestField::ObjectsDir => self.objects_dir = PathBuf::from(value),
            ManifestField::PagesDir => self.pages_dir = PathBuf::from(value),
            ManifestField::BuildDir => self.build_dir = PathBuf::from(value),
            ManifestField::StaticDir => self.static_dir = PathBuf::from(value),
            ManifestField::SchemasDir => self.schemas_dir = PathBuf::from(value),
            ManifestField::LayoutDir => self.layout_dir = PathBuf::from(value),
            ManifestField::Metadata => {
                panic!("Metadata is not modifiable via events")
            }
            ManifestField::EditorTypes => {
                panic!("EditorTypes are not modifiable via events")
            }
        }
    }

    pub fn field_as_string(&self, field: &ManifestField) -> String {
        match self.toml_field(field) {
            Some(fv) => match fv {
                Value::Array(a) => Value::Array(a).to_string(),
                Value::String(s) => s,
                Value::Table(t) => toml::to_string(&t).unwrap_or_default(),
                _ => panic!("unsupported manifest field type"),
            },
            None => String::default(),
        }
    }

    fn durable_fields() -> Vec<ManifestField> {
        vec![
            ManifestField::ArchivalVersion,
            ManifestField::SiteUrl,
            ManifestField::SiteName,
            ManifestField::UploadPrefix,
            ManifestField::UploadsUrl,
            ManifestField::Prebuild,
            ManifestField::Postbuild,
            ManifestField::ObjectDefinitionFile,
            ManifestField::StaticDir,
            ManifestField::BuildDir,
            ManifestField::PagesDir,
            ManifestField::ObjectsDir,
            ManifestField::LayoutDir,
            ManifestField::SchemasDir,
            ManifestField::EditorTypes,
            ManifestField::Metadata,
        ]
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        let mut write_obj = Table::new();
        for field in Self::durable_fields() {
            let key = field.field_name();
            if !self.is_default(&field) {
                if let Some(value) = self.toml_field(&field) {
                    write_obj.insert(key.to_string(), value);
                }
            }
        }
        toml::to_string_pretty(&write_obj)
    }

    pub fn watched_paths(&self) -> Vec<String> {
        [
            &self.object_definition_file,
            &self.objects_dir,
            &self.pages_dir,
            &self.static_dir,
            &self.layout_dir,
        ]
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    use pretty_assertions::assert_eq;

    fn full_manifest_content() -> &'static str {
        r#"archival_version = "0.8.0"
site_url = "https://jesse.onarchival.dev"
site_name = "jesse's site"
upload_prefix = "site-repo-doid/"
uploads_url = "https://uploads.archival.dev"
prebuild = ['echo "HELLO!"']
postbuild = ['echo "BYE!"']
object_file = "m_objects.toml"
static_dir = "m_public"
build_dir = "m_dist"
pages = "m_pages"
objects = "m_objects"
layout_dir = "m_layout"
schemas_dir = "m_schemas"

[editor_types.day]
type = "date"
validate = ['\d{2}/\d{2}/\d{4}']

[editor_types.custom]
type = "meta"
editor_url = "https://editor.archival.dev/editors/json/editor.html"

[[editor_types.custom.validate]]
path = "field_a"
validate = ".+"

[[editor_types.custom.validate]]
path = "field_b"
validate = ".+"

[metadata]
foo = "bar"
baz = "hello!"
"#
    }

    /// A build reads its source dirs and writes only into build_dir. Nesting either
    /// inside the other makes a build read what it just wrote, which is the invariant
    /// the whole plan/apply split rests on.
    #[test]
    fn build_dir_may_not_overlap_a_source_dir() {
        for manifest in [
            "build_dir = \"public/dist\"\n",
            "static_dir = \"dist/public\"\n",
            "build_dir = \"pages\"\n",
            "objects = \"dist\"\n",
        ] {
            let result = Manifest::from_string(Path::new(""), manifest.to_string(), Some("test"));
            assert!(
                result.is_err(),
                "overlapping directories were accepted: {manifest}"
            );
        }
    }

    #[test]
    fn the_default_layout_keeps_build_dir_separate() {
        assert!(Manifest::from_string(Path::new(""), String::new(), Some("test")).is_ok());
    }

    #[test]
    fn hook_fields_print_as_arrays() -> Result<()> {
        let m = Manifest::from_string(Path::new(""), full_manifest_content().to_string(), None)?;
        assert_eq!(
            m.field_as_string(&ManifestField::Prebuild),
            r#"['echo "HELLO!"']"#
        );
        assert_eq!(
            m.field_as_string(&ManifestField::Postbuild),
            r#"['echo "BYE!"']"#
        );
        Ok(())
    }

    #[test]
    fn manifest_parsing() -> Result<()> {
        let m = Manifest::from_string(Path::new(""), full_manifest_content().to_string(), None)?;
        println!("M: {:?}", m);
        assert_eq!(m.archival_version, Some("0.8.0".to_string()));
        assert_eq!(m.upload_prefix, "site-repo-doid/".to_string());
        assert_eq!(m.site_url, Some("https://jesse.onarchival.dev".to_string()));
        assert_eq!(
            m.object_definition_file,
            Path::new("m_objects.toml").to_path_buf()
        );
        assert_eq!(m.objects_dir, Path::new("m_objects").to_path_buf());
        assert_eq!(m.pages_dir, Path::new("m_pages").to_path_buf());
        assert_eq!(m.build_dir, Path::new("m_dist").to_path_buf());
        assert_eq!(m.static_dir, Path::new("m_public").to_path_buf());
        assert_eq!(m.layout_dir, Path::new("m_layout").to_path_buf());
        assert_eq!(m.schemas_dir, Path::new("m_schemas").to_path_buf());
        assert_eq!(m.site_name, Some("jesse's site".to_string()));
        assert_eq!(
            m.uploads_url,
            Some("https://uploads.archival.dev".to_string())
        );
        assert_eq!(m.prebuild.len(), 1);
        assert_eq!(m.postbuild, vec!["echo \"BYE!\""]);
        let t1 = &m.editor_types["day"];
        assert_eq!(t1.alias_of, "date");
        assert_eq!(t1.validate.len(), 1);
        assert!(matches!(
            t1.validate[0],
            ManifestEditorTypeValidator::Value(_)
        ));
        if let ManifestEditorTypeValidator::Value(v) = &t1.validate[0] {
            assert_eq!(v.to_string(), "\\d{2}/\\d{2}/\\d{4}");
        }
        let t2 = &m.editor_types["custom"];
        assert_eq!(t2.alias_of, "meta");
        assert_eq!(
            t2.editor_url,
            "https://editor.archival.dev/editors/json/editor.html"
        );
        assert_eq!(t2.validate.len(), 2);
        assert!(matches!(
            t2.validate[0],
            ManifestEditorTypeValidator::Path(_)
        ));
        if let ManifestEditorTypeValidator::Path(v) = &t2.validate[0] {
            assert_eq!(v.path.to_string(), "field_a");
            assert_eq!(v.validate.to_string(), ".+");
        }
        assert!(matches!(
            t2.validate[1],
            ManifestEditorTypeValidator::Path(_)
        ));
        if let ManifestEditorTypeValidator::Path(v) = &t2.validate[1] {
            assert_eq!(v.path.to_string(), "field_b");
            assert_eq!(v.validate.to_string(), ".+");
        }
        let manifest_output = m.to_toml()?;
        println!("MTOML {}", manifest_output);
        assert!(manifest_output.contains("[editor_types.day]"));
        assert!(manifest_output.contains("[editor_types.custom]"));
        assert!(manifest_output
            .contains("editor_url = \"https://editor.archival.dev/editors/json/editor.html\""));
        assert!(manifest_output.contains("[[editor_types.custom.validate]]"));

        assert_eq!(full_manifest_content(), manifest_output);

        let parsed = Manifest::from_string(Path::new(""), manifest_output, None)?;
        assert_eq!(parsed, m);
        Ok(())
    }

    #[test]
    fn invalid_validator_returns_an_error() {
        let err = Manifest::from_string(
            Path::new(""),
            r#"
[editor_types.broken]
type = "string"
validate = ['[unclosed']
"#
            .to_string(),
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<InvalidManifestError>(),
            Some(InvalidManifestError::InvalidValidator(name, _)) if name == "broken"
        ));
    }

    #[test]
    fn invalid_metadata_returns_an_error() {
        let err = Manifest::from_string(
            Path::new(""),
            r#"
[metadata]
foo = 12
"#
            .to_string(),
            None,
        )
        .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<InvalidManifestError>(),
            Some(InvalidManifestError::InvalidMetadata(_, field)) if field == "foo"
        ));
    }
}
