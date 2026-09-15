use super::file::File;
use super::meta::Meta;
use super::DateTime;
use super::{FieldType, InvalidFieldError};
use crate::fields::file::RenderedFile;
use crate::fields::DisplayType;
use crate::manifest::{EditorTypes, ManifestEditorTypeValidator};
use crate::object::to_liquid::{object_to_liquid_with, ToLiquidOptions};
use crate::object::Renderable;
use crate::util::integer_decode;
use crate::value_path::ValuePathError;
use crate::{FieldConfig, ObjectDefinition, ValuePath};
use anyhow::Result;
use comrak::nodes::NodeValue;
use comrak::{format_html, parse_document, Arena, ComrakOptions};
use liquid::{model, ValueView};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::{self, Debug};
use std::hash::Hash;
use toml::Value;
use tracing::{instrument, warn};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum FieldValueValidationError {
    #[error("Type mismatch: {0} provided for path {1} which is of type {2}")]
    TypeMismatch(FieldValue, ValuePath, FieldType),
    #[error("Invalid enum value {0} provided for path {1} (valid values {2})")]
    InvalidEnumValue(String, ValuePath, String),
    #[error("Invalid file type {0} provided for path {1} which is of type {2}")]
    InvalidFileType(DisplayType, ValuePath, FieldType),
    #[error("Field definition not found for {0} in {1:?} ({2})")]
    FieldDefinitionNotFound(ValuePath, ObjectDefinition, ValuePathError),
    #[error("Invalid oneof name {0} for path {1} (options {2})")]
    InvalidOneofName(String, ValuePath, String),
    #[error("Invalid oneof type {0} for path {1}: expected {2}")]
    InvalidOneofType(FieldType, ValuePath, FieldType),
    #[error("Cannot Validate type {0} at path {1}")]
    CannotValidateType(FieldValue, ValuePath),
    #[error("field '{0}' at {1} failed validator '{2}'")]
    FailedValidation(String, ValuePath, String),
}

// These are BTrees rather than OrderMaps because we only serialize them when we
// have access to the definition, which has the field order.
pub type ObjectValues = BTreeMap<String, FieldValue>;
pub type RenderedObjectValues = BTreeMap<String, RenderedFieldValue>;

impl Renderable for ObjectValues {
    type Output = RenderedObjectValues;
    fn rendered(self, field_config: &FieldConfig) -> Self::Output {
        self.into_iter()
            .map(|(o, v)| (o, v.rendered(field_config)))
            .collect()
    }
}

#[cfg(feature = "typescript")]
mod typedefs {
    use typescript_type_def::{
        type_expr::{Ident, NativeTypeInfo, TypeExpr, TypeInfo},
        TypeDef,
    };
    pub struct RenderedObjectValuesTypeDef;
    impl TypeDef for RenderedObjectValuesTypeDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            // Workaround for circular type: https://github.com/dbeckwith/rust-typescript-type-def/issues/18#issuecomment-2078469020
            r#ref: TypeExpr::ident(Ident("Record<string, RenderedFieldValue>[]")),
        });
    }
    pub struct ObjectValuesTypeDef;
    impl TypeDef for ObjectValuesTypeDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            // Workaround for circular type: https://github.com/dbeckwith/rust-typescript-type-def/issues/18#issuecomment-2078469020
            r#ref: TypeExpr::ident(Ident("Record<string, FieldValue>[]")),
        });
    }
    pub struct RenderedOneofTypeDef;
    impl TypeDef for RenderedOneofTypeDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            // Workaround for circular type: https://github.com/dbeckwith/rust-typescript-type-def/issues/18#issuecomment-2078469020
            r#ref: TypeExpr::ident(Ident("[string, RenderedFieldValue | null]")),
        });
    }
    pub struct OneofTypeDef;
    impl TypeDef for OneofTypeDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            // Workaround for circular type: https://github.com/dbeckwith/rust-typescript-type-def/issues/18#issuecomment-2078469020
            r#ref: TypeExpr::ident(Ident("[string, FieldValue | null]")),
        });
    }
}

macro_rules! compare_values {
    ($left:ident, $right:ident, $($t:path),*) => {
        match $left {
            $($t(lv) => {
                if let $t(rv) = $right {
                    lv.partial_cmp(rv)
                } else {
                    None
                }
            })*
            _ => None
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub enum RenderedFieldValue {
    String(String),
    Secret(String),
    Enum(String),
    Markdown(String),
    Number(f64),
    Date(DateTime),
    Objects(
        #[cfg_attr(
            feature = "typescript",
            type_def(type_of = "typedefs::RenderedObjectValuesTypeDef")
        )]
        Vec<RenderedObjectValues>,
    ),
    Oneof(
        #[cfg_attr(
            feature = "typescript",
            type_def(type_of = "typedefs::RenderedOneofTypeDef")
        )]
        (String, Box<Option<RenderedFieldValue>>),
    ),
    Boolean(bool),
    File(RenderedFile),
    Meta(Meta),
    Null,
}
impl Hash for RenderedFieldValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            RenderedFieldValue::Number(n) => integer_decode(*n).hash(state),
            v => v.hash(state),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub enum FieldValue {
    String(String),
    /// A string that is never exposed to templates. Read it from rust with
    /// [`FieldValue::as_secret`].
    Secret(String),
    Enum(String),
    Markdown(String),
    Number(f64),
    Date(DateTime),
    Objects(
        #[cfg_attr(
            feature = "typescript",
            type_def(type_of = "typedefs::ObjectValuesTypeDef")
        )]
        Vec<ObjectValues>,
    ),
    Oneof(
        #[cfg_attr(feature = "typescript", type_def(type_of = "typedefs::OneofTypeDef"))]
        (String, Box<Option<FieldValue>>),
    ),
    Boolean(bool),
    File(File),
    Meta(Meta),
    Null,
}

impl Hash for FieldValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            FieldValue::Number(n) => integer_decode(*n).hash(state),
            v => v.hash(state),
        }
    }
}

impl Renderable for FieldValue {
    type Output = RenderedFieldValue;
    fn rendered(self, field_config: &FieldConfig) -> Self::Output {
        match self {
            FieldValue::String(s) => RenderedFieldValue::String(s),
            FieldValue::Secret(s) => RenderedFieldValue::Secret(s),
            FieldValue::Enum(e) => RenderedFieldValue::Enum(e),
            FieldValue::Markdown(m) => RenderedFieldValue::Markdown(m),
            FieldValue::Number(n) => RenderedFieldValue::Number(n),
            FieldValue::Date(date) => RenderedFieldValue::Date(date),
            FieldValue::Objects(obj) => RenderedFieldValue::Objects(
                obj.into_iter()
                    .map(|objects| {
                        objects
                            .into_iter()
                            .map(|(k, v)| (k, v.rendered(field_config)))
                            .collect()
                    })
                    .collect(),
            ),
            FieldValue::Oneof((t, v)) => {
                RenderedFieldValue::Oneof((t, Box::new(v.map(|fv| fv.rendered(field_config)))))
            }
            FieldValue::Boolean(b) => RenderedFieldValue::Boolean(b),
            FieldValue::File(file) => RenderedFieldValue::File(file.rendered(field_config)),
            FieldValue::Meta(m) => RenderedFieldValue::Meta(m),
            FieldValue::Null => todo!(),
        }
    }
}

pub static MARKDOWN_OPTIONS: Lazy<ComrakOptions> = Lazy::new(|| {
    let mut options = ComrakOptions::default();
    options.extension.autolink = true;
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.superscript = true;
    options.extension.description_lists = true;
    // NOTE: it's unclear how much nannying we need to do here, as users are
    // only able to update their own markdown and by definition they have access
    // to the html if they have access to the repo... however if someone is
    // tricked into pasting things into markdown they could potentially open
    // some issues?
    options.extension.tagfilter = false;
    options.extension.header_ids = Some("".to_string());
    options.extension.footnotes = true;
    options.render.unsafe_ = true;
    options
});

// Markdown fields are converted to html every time they are turned into a
// liquid value, which happens at least once per build and, in long-lived
// processes like the dev server, once per rebuild. Conversion output only
// depends on the source, so memoize it. The cap just bounds memory in
// processes that see many unique documents (e.g. editing sessions).
const MARKDOWN_CACHE_MAX_ENTRIES: usize = 1024;
static MARKDOWN_CACHE: Lazy<std::sync::RwLock<std::collections::HashMap<String, String>>> =
    Lazy::new(|| std::sync::RwLock::new(std::collections::HashMap::new()));

/// Liquid written inside markdown code — an inline span, or a fenced or
/// indented block — is there to be read, not run: the `{{ … }}` in a code
/// sample *is* the sample. A markdown field is turned into html before the page
/// renders (`as_scalar`, below) and `crate::tags::output` renders liquid it
/// finds in a value, so without this the sample would be evaluated —
/// or, more often, fail to parse, since the html escaping applied to code turns
/// the quotes in a filter argument into `&quot;`.
///
/// The html renderer escapes code itself, so there is no way to ask it to write
/// a literal `{`. Instead every `{` that opens a liquid delimiter is replaced in
/// the ast with a sentinel the renderer passes through untouched, and the
/// sentinel becomes `&#123;` once the html exists. `\0` is sound as that
/// sentinel because comrak replaces every NUL in its input with U+FFFD while
/// reading lines — CommonMark requires it — so one can never reach the ast from
/// the source.
///
/// Only markdown's own code nodes are treated this way. Html the author wrote
/// out by hand passes through untouched, `<code>` included, so a template that
/// deliberately outputs liquid through one keeps working.
const CODE_SENTINEL: char = '\0';

/// What [`CODE_SENTINEL`] stands for. A numeric character reference, because a
/// `{` written literally here would be liquid again.
const ESCAPED_BRACE: &str = "&#123;";

fn markdown_to_html(source: &str) -> String {
    let arena = Arena::new();
    let root = parse_document(&arena, source, &MARKDOWN_OPTIONS);
    let mut found = false;
    for node in root.descendants() {
        let mut data = node.data.borrow_mut();
        let literal = match data.value {
            NodeValue::Code(ref mut code) => &mut code.literal,
            NodeValue::CodeBlock(ref mut block) => &mut block.literal,
            _ => continue,
        };
        if let Some(neutralized) = neutralize_liquid(literal) {
            *literal = neutralized;
            found = true;
        }
    }
    let mut html = Vec::new();
    format_html(root, &MARKDOWN_OPTIONS, &mut html).expect("writing to a Vec cannot fail");
    let html = String::from_utf8(html).expect("comrak writes utf-8");
    if found {
        html.replace(CODE_SENTINEL, ESCAPED_BRACE)
    } else {
        html
    }
}

/// Replaces each `{` that opens a `{{` or `{%` with [`CODE_SENTINEL`],
/// returning `None` when there is none to replace. One pass is enough because
/// the `{` is consumed rather than kept: a `{` left behind is by definition not
/// followed by `{` or `%`, and a sentinel contributes no `{` of its own, so no
/// delimiter can survive the pass or form behind it.
fn neutralize_liquid(literal: &str) -> Option<String> {
    if !literal.contains("{{") && !literal.contains("{%") {
        return None;
    }
    let bytes = literal.as_bytes();
    let mut out = String::with_capacity(literal.len());
    for (at, c) in literal.char_indices() {
        if c == '{' && matches!(bytes.get(at + 1), Some(b'{' | b'%')) {
            out.push(CODE_SENTINEL);
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn markdown_to_html_cached(source: &str) -> String {
    if let Some(html) = MARKDOWN_CACHE.read().unwrap().get(source) {
        return html.clone();
    }
    let html = markdown_to_html(source);
    let mut cache = MARKDOWN_CACHE.write().unwrap();
    if cache.len() >= MARKDOWN_CACHE_MAX_ENTRIES {
        cache.clear();
    }
    cache.insert(source.to_owned(), html.clone());
    html
}

impl FieldValue {
    // Note that this comparison just skips fields that cannot be compared and
    // returns None.
    pub fn compare(&self, to: &FieldValue) -> Option<Ordering> {
        compare_values!(
            self,
            to,
            Self::String,
            Self::Markdown,
            Self::Number,
            Self::Date,
            Self::Boolean,
            Self::File
        )
    }

    pub fn typed_objects(
        &self,
        definition: &ObjectDefinition,
        field_config: &FieldConfig,
    ) -> model::Value {
        self.typed_objects_with(definition, field_config, ToLiquidOptions::default())
    }

    pub fn typed_objects_with(
        &self,
        definition: &ObjectDefinition,
        field_config: &FieldConfig,
        options: ToLiquidOptions,
    ) -> model::Value {
        if let FieldValue::Objects(children) = self {
            model::Value::Array(
                children
                    .iter()
                    .map(|child| {
                        model::Value::Object(object_to_liquid_with(
                            child,
                            definition,
                            field_config,
                            options,
                        ))
                    })
                    .collect(),
            )
        } else {
            panic!("cannot call typed_objects on FieldValue: {:?}", self);
        }
    }

    /// Secret values are stripped from template contexts, so this is the only
    /// way to read one.
    pub fn as_secret(&self) -> Option<&str> {
        match self {
            FieldValue::Secret(s) => Some(s),
            FieldValue::Oneof((_, v)) => v.as_ref().as_ref().and_then(|v| v.as_secret()),
            _ => None,
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            FieldValue::String(s) => s.trim().is_empty(),
            FieldValue::Secret(s) => s.trim().is_empty(),
            FieldValue::Enum(_) => false,
            FieldValue::Markdown(m) => m.trim().is_empty(),
            FieldValue::Number(_) => false,
            FieldValue::Date(_) => false,
            FieldValue::Objects(_) => false,
            FieldValue::Oneof(_) => false,
            FieldValue::Boolean(_) => false,
            FieldValue::File(f) => !f.is_valid(),
            FieldValue::Meta(meta) => meta.is_empty(),
            FieldValue::Null => true,
        }
    }

    #[allow(clippy::result_large_err)]
    fn run_custom_validation(
        &self,
        path: &ValuePath,
        field_type: &FieldType,
        custom_types: &EditorTypes,
    ) -> Result<(), FieldValueValidationError> {
        // You can only define a validator via editor_types, which will always
        // create an alias type
        if let FieldType::Alias(a) = field_type {
            if let Some(custom_type) = custom_types.get(&a.1) {
                for validator in &custom_type.validate {
                    match validator {
                        ManifestEditorTypeValidator::Path(p) => {
                            if let Ok(validated_value) = p.path.get_value(self) {
                                if !p.validate.validate(&validated_value.to_string()) {
                                    return Err(FieldValueValidationError::FailedValidation(
                                        validated_value.to_string(),
                                        path.clone(),
                                        p.validate.to_string(),
                                    ));
                                }
                            } else {
                                // Value not found - if our validator passes
                                // with an empty string, this is ok. Otherwise
                                // this is an error.
                                if !p.validate.validate("") {
                                    return Err(FieldValueValidationError::FailedValidation(
                                        "(not found)".to_string(),
                                        path.clone(),
                                        p.validate.to_string(),
                                    ));
                                }
                            }
                        }
                        ManifestEditorTypeValidator::Value(v) => {
                            // Secrets are redacted by Display, so validate
                            // against the actual value.
                            let value = self
                                .as_secret()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| self.to_string());
                            if !v.validate(&value) {
                                return Err(FieldValueValidationError::FailedValidation(
                                    self.to_string(),
                                    path.clone(),
                                    v.to_string(),
                                ));
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[allow(clippy::result_large_err)]
    fn validate_type(
        &self,
        path: &ValuePath,
        field_type: &FieldType,
    ) -> Result<(), FieldValueValidationError> {
        let field_mismatch = || {
            Err(FieldValueValidationError::TypeMismatch(
                self.clone(),
                path.clone(),
                field_type.clone(),
            ))
        };
        match self {
            Self::Enum(enum_val) => {
                if let FieldType::Enum(valid_values) = field_type {
                    if !valid_values.contains(enum_val) {
                        return Err(FieldValueValidationError::InvalidEnumValue(
                            enum_val.clone(),
                            path.clone(),
                            valid_values.join(","),
                        ));
                    }
                    Ok(())
                } else {
                    field_mismatch()
                }
            }
            Self::String(_) => {
                if !matches!(field_type, FieldType::String) {
                    field_mismatch()
                } else {
                    Ok(())
                }
            }
            Self::Secret(_) => {
                if !matches!(field_type, FieldType::Secret) {
                    field_mismatch()
                } else {
                    Ok(())
                }
            }
            Self::Markdown(_) => {
                if !matches!(field_type, FieldType::Markdown) {
                    field_mismatch()
                } else {
                    Ok(())
                }
            }
            Self::Number(_) => {
                if !matches!(field_type, FieldType::Number) {
                    field_mismatch()
                } else {
                    Ok(())
                }
            }
            Self::Date(_) => {
                if !matches!(field_type, FieldType::Date) {
                    field_mismatch()
                } else {
                    Ok(())
                }
            }
            Self::Boolean(_) => {
                if !matches!(field_type, FieldType::Boolean) {
                    field_mismatch()
                } else {
                    Ok(())
                }
            }
            Self::File(f) => {
                if !matches!(
                    field_type,
                    FieldType::Audio | FieldType::Image | FieldType::Video | FieldType::Upload
                ) {
                    field_mismatch()
                } else {
                    let assert_file_match = |match_ok: bool, required_type: FieldType| {
                        if !match_ok {
                            Err(FieldValueValidationError::InvalidFileType(
                                f.display_type.clone(),
                                path.clone(),
                                required_type,
                            ))
                        } else {
                            Ok(())
                        }
                    };
                    match &f.display_type {
                        DisplayType::Image => assert_file_match(
                            matches!(field_type, FieldType::Image),
                            FieldType::Image,
                        ),
                        DisplayType::Video => assert_file_match(
                            matches!(field_type, FieldType::Video),
                            FieldType::Video,
                        ),
                        DisplayType::Audio => assert_file_match(
                            matches!(field_type, FieldType::Audio),
                            FieldType::Audio,
                        ),
                        DisplayType::Download => assert_file_match(
                            matches!(field_type, FieldType::Upload),
                            FieldType::Upload,
                        ),
                    }
                }
            }
            Self::Meta(_meta) => {
                if !matches!(field_type, FieldType::Meta) {
                    field_mismatch()
                } else {
                    // TODO: we could/should do some more sophisticated meta
                    // validation here
                    Ok(())
                }
            }
            // All fields can be set to null
            Self::Null => Ok(()),
            // Fallthrough for types that require upstream/complex validation
            t => Err(FieldValueValidationError::CannotValidateType(
                t.clone(),
                path.clone(),
            )),
        }
    }

    #[allow(clippy::result_large_err)]
    pub fn validate(
        &self,
        path: &ValuePath,
        definition: &ObjectDefinition,
        custom_types: &EditorTypes,
    ) -> Result<(), FieldValueValidationError> {
        let field_type = path.get_field_definition(definition).map_err(|e| {
            FieldValueValidationError::FieldDefinitionNotFound(path.clone(), definition.clone(), e)
        })?;
        let field_mismatch = || {
            Err(FieldValueValidationError::TypeMismatch(
                self.clone(),
                path.clone(),
                field_type.clone(),
            ))
        };
        self.run_custom_validation(path, field_type, custom_types)?;
        // After we've run custom validation, aliases should just be dereferenced.
        let field_type = match field_type {
            FieldType::Alias(val) => &val.0,
            _ => field_type,
        };
        match self {
            // Oneof needs special checking since we need to validate the inner
            // type and the type name against the valid options
            Self::Oneof((name, value)) => {
                if let FieldType::Oneof(valid_values) = field_type {
                    let found_type = valid_values
                        .iter()
                        .find(|val| val.name == *name)
                        .ok_or_else(|| {
                            FieldValueValidationError::InvalidOneofName(
                                name.clone(),
                                path.clone(),
                                valid_values
                                    .iter()
                                    .map(|v| v.name.to_string())
                                    .collect::<Vec<String>>()
                                    .join(","),
                            )
                        })?;
                    if let Some(v) = value.as_ref() {
                        v.validate_type(path, &found_type.r#type)
                    } else {
                        // Empty is ok for all values
                        Ok(())
                    }
                } else {
                    field_mismatch()
                }
            }
            // Objects should be recursively validated
            Self::Objects(children) => {
                for (idx, child) in children.iter().enumerate() {
                    for (name, field) in child {
                        field.validate(
                            &path.clone().concat(
                                ValuePath::empty()
                                    .append(ValuePath::index(idx))
                                    .append(ValuePath::key(name)),
                            ),
                            definition,
                            custom_types,
                        )?;
                    }
                }
                Ok(())
            }
            t => t.validate_type(path, field_type),
        }
    }

    #[cfg(test)]
    pub fn liquid_date(&self) -> model::DateTime {
        match self {
            FieldValue::Date(d) => d.as_liquid_datetime(),
            _ => panic!("Not a date"),
        }
    }
}

impl fmt::Display for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_string(Some(&FieldConfig::default())))
    }
}

// Conversions

impl From<&FieldValue> for Option<toml::Value> {
    fn from(value: &FieldValue) -> Self {
        match value {
            FieldValue::String(v) => Some(toml::Value::String(v.to_owned())),
            FieldValue::Secret(v) => Some(toml::Value::String(v.to_owned())),
            FieldValue::Enum(v) => Some(toml::Value::String(v.to_owned())),
            FieldValue::Markdown(v) => Some(toml::Value::String(v.to_owned())),
            FieldValue::Number(n) => Some(toml::Value::Float(*n)),
            FieldValue::Date(d) => {
                let d = d.as_liquid_datetime();
                Some(toml::Value::Datetime(toml_datetime::Datetime {
                    date: Some(toml_datetime::Date {
                        year: d.year() as u16,
                        month: d.month(),
                        day: d.day(),
                    }),
                    time: Some(toml_datetime::Time {
                        hour: d.hour(),
                        minute: d.minute(),
                        second: Some(d.second()),
                        nanosecond: Some(d.nanosecond()),
                    }),
                    offset: None,
                }))
            }
            FieldValue::Boolean(v) => Some(toml::Value::Boolean(v.to_owned())),
            FieldValue::Objects(o) => Some(toml::Value::Array(
                o.iter()
                    .map(|child| {
                        let mut vals: toml::map::Map<String, Value> = toml::map::Map::new();
                        for (key, cv) in child {
                            if let Some(val) = cv.into() {
                                vals.insert(key.to_string(), val);
                            }
                        }
                        toml::Value::Table(vals)
                    })
                    .collect(),
            )),
            FieldValue::Oneof((t, v)) => {
                let mut table = toml::map::Map::new();
                table.insert("type".to_string(), toml::Value::String(t.to_string()));
                if let Some(value) = v.as_ref().as_ref().and_then(Option::<toml::Value>::from) {
                    table.insert("value".to_string(), value);
                }
                Some(toml::Value::Table(table))
            }
            FieldValue::File(f) => Some(toml::Value::Table(f.to_toml())),
            FieldValue::Meta(m) => Some(toml::Value::Table(m.to_toml())),
            FieldValue::Null => None,
        }
    }
}

impl ValueView for FieldValue {
    /// Get a `Debug` representation
    fn as_debug(&self) -> &dyn fmt::Debug {
        self
    }
    /// A `Display` for a `BoxedValue` rendered for the user.
    fn render(&self) -> model::DisplayCow<'_> {
        model::DisplayCow::Owned(Box::new(self))
    }
    /// A `Display` for a `Value` as source code.
    fn source(&self) -> model::DisplayCow<'_> {
        model::DisplayCow::Owned(Box::new(self))
    }

    /// Report the data type (generally for error reporting).
    fn type_name(&self) -> &'static str {
        match self {
            FieldValue::String(_) => "string",
            FieldValue::Secret(_) => "secret",
            FieldValue::Enum(_) => "enum",
            FieldValue::Markdown(_) => "markdown",
            FieldValue::Number(_) => "number",
            FieldValue::Date(_) => "date",
            FieldValue::Objects(_) => "objects",
            FieldValue::Boolean(_) => "boolean",
            FieldValue::File(_) => "file",
            FieldValue::Meta(_) => "meta",
            FieldValue::Oneof(_) => "oneof",
            FieldValue::Null => "null",
        }
    }
    /// Interpret as a string.
    fn to_kstr(&self) -> model::KStringCow<'_> {
        model::KStringCow::from(self.as_string(None))
    }
    /// Query the value's state
    fn query_state(&self, state: model::State) -> bool {
        match state {
            model::State::Truthy => !self.is_empty(),
            model::State::DefaultValue => self.is_empty(),
            model::State::Empty => self.is_empty(),
            model::State::Blank => self.is_empty(),
        }
    }

    fn as_scalar(&self) -> Option<model::ScalarCow<'_>> {
        match self {
            FieldValue::String(s) => Some(model::ScalarCow::new(s)),
            // Secrets never resolve to a value in liquid.
            FieldValue::Secret(_) => None,
            FieldValue::Enum(s) => Some(model::ScalarCow::new(s)),
            FieldValue::Number(n) => Some(model::ScalarCow::new(*n)),
            // TODO: should be able to return a datetime value here
            FieldValue::Date(d) => Some(model::ScalarCow::new((*d).as_liquid_datetime())),
            FieldValue::Markdown(s) => Some(model::ScalarCow::new(markdown_to_html_cached(s))),
            FieldValue::Boolean(b) => Some(model::ScalarCow::new(*b)),
            FieldValue::Objects(_) => None,
            FieldValue::Oneof((_, v)) => v.as_scalar(),
            FieldValue::File(_f) => None,
            FieldValue::Meta(_m) => None,
            FieldValue::Null => None,
        }
    }
    fn as_array(&self) -> Option<&dyn model::ArrayView> {
        match self {
            FieldValue::Objects(a) => Some(a),
            _ => None,
        }
    }
    fn as_object(&self) -> Option<&dyn model::ObjectView> {
        match self {
            FieldValue::Meta(m) => Some(m),
            _ => None,
        }
    }

    fn to_value(&self) -> liquid::model::Value {
        match self {
            FieldValue::String(_) => self.as_scalar().to_value(),
            FieldValue::Secret(_) => liquid::model::Value::Nil,
            FieldValue::Enum(_) => self.as_scalar().to_value(),
            FieldValue::Markdown(_) => self.as_scalar().to_value(),
            FieldValue::Number(_) => self.as_scalar().to_value(),
            FieldValue::Date(_) => self.as_scalar().to_value(),
            FieldValue::Boolean(_) => self.as_scalar().to_value(),
            FieldValue::Objects(_) => self.as_array().to_value(),
            FieldValue::File(_) => {
                panic!("files cannot be rendered via value parsing. Use file.to_liquid instead.")
            }
            FieldValue::Oneof(_) => {
                panic!("oneof cannot be rendered via value parsing. Use oneof.to_liquid instead.")
            }
            FieldValue::Meta(_) => self.as_object().to_value(),
            FieldValue::Null => self.as_scalar().to_value(),
        }
    }
}

impl FieldValue {
    pub fn from_string(
        key: &String,
        field_type: &FieldType,
        value: String,
    ) -> Result<FieldValue, InvalidFieldError> {
        // Aliases are just their underlying type as far as values go.
        let field_type = field_type.base_type();
        if value.is_empty() {
            // Defaults
            let default_val = match field_type {
                FieldType::String => Ok(FieldValue::String(value.clone())),
                FieldType::Secret => Ok(FieldValue::Secret(value.clone())),
                FieldType::Markdown => Ok(FieldValue::Markdown(value.clone())),
                FieldType::Number => Ok(FieldValue::Number(0.0)),
                FieldType::Boolean => Ok(FieldValue::Boolean(false)),
                _ => Err(InvalidFieldError::NoDefaultForType(field_type.to_string())),
            };
            if default_val.is_ok() {
                return default_val;
            }
        }
        match field_type {
            FieldType::String => Ok(FieldValue::String(value)),
            FieldType::Secret => Ok(FieldValue::Secret(value)),
            FieldType::Enum(valid_values) => {
                if !valid_values.contains(&value) {
                    Err(InvalidFieldError::EnumMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    })?
                }
                Ok(FieldValue::Enum(value))
            }
            FieldType::Markdown => Ok(FieldValue::Markdown(value)),
            FieldType::Number => Ok(FieldValue::Number(value.parse::<f64>().map_err(|_| {
                InvalidFieldError::TypeMismatch {
                    field: key.to_owned(),
                    field_type: field_type.to_string(),
                    value,
                }
            })?)),
            FieldType::Boolean => Ok(FieldValue::Boolean(value.parse::<bool>().map_err(
                |_| InvalidFieldError::TypeMismatch {
                    field: key.to_owned(),
                    field_type: field_type.to_string(),
                    value,
                },
            )?)),
            FieldType::Date => {
                let date_str = DateTime::parse_date_string(value.to_string()).map_err(|_| {
                    InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value,
                    }
                })?;
                Ok(FieldValue::Date(DateTime::from(&date_str)?))
            }
            _ => Err(InvalidFieldError::UnsupportedStringValue(
                field_type.to_string(),
            )),
        }
    }

    fn field_from_json(
        value: &serde_json::Value,
        field_type: &FieldType,
        parent_path: &ValuePath,
        object_definition: &ObjectDefinition,
    ) -> Result<Option<Self>> {
        // Aliases are just their underlying type as far as values go.
        let field_type = match field_type {
            FieldType::Alias(a) => &a.0,
            t => t,
        };
        match value {
            serde_json::Value::String(s) => Ok(Some(match field_type {
                FieldType::Markdown => FieldValue::Markdown(s.to_string()),
                FieldType::Secret => FieldValue::Secret(s.to_string()),
                _ => FieldValue::String(s.to_string()),
            })),
            serde_json::Value::Bool(b) => Ok(Some(FieldValue::Boolean(*b))),
            serde_json::Value::Number(n) => Ok(Some(FieldValue::Number(n.as_f64().unwrap()))),
            serde_json::Value::Null => Ok(Some(match field_type {
                FieldType::Secret => FieldValue::Secret("".into()),
                _ => FieldValue::String("".into()),
            })),
            serde_json::Value::Object(o) => match field_type {
                FieldType::Oneof(options) => {
                    if let (Some(serde_json::Value::String(typ)), val) =
                        (o.get("type"), o.get("value"))
                    {
                        let found_type = options
                            .iter()
                            .find_map(|opt| {
                                if opt.name == *typ {
                                    Some(opt.r#type.clone())
                                } else {
                                    None
                                }
                            })
                            .ok_or_else(|| {
                                InvalidFieldError::InvalidOneof(format!(
                                    "faied finding type {}",
                                    typ
                                ))
                            })?;
                        println!("FOUND TYPE: {:?} ({:?})", found_type, val);
                        match val {
                            Some(val) => Ok(FieldValue::field_from_json(
                                val,
                                &found_type,
                                &parent_path.clone(),
                                object_definition,
                            )?
                            .map(|value| {
                                FieldValue::Oneof((typ.to_string(), Box::new(Some(value))))
                            })),
                            None => Ok(Some(FieldValue::Oneof((typ.to_string(), Box::new(None))))),
                        }
                    } else {
                        Err(InvalidFieldError::InvalidOneof(format!(
                            "expected value at {parent_path}, found: {:?}",
                            o
                        ))
                        .into())
                    }
                }
                FieldType::Video | FieldType::Audio | FieldType::Upload | FieldType::Image => {
                    Ok(Some(
                        File::download()
                            .fill_from_json_map(o)
                            .map(FieldValue::File)?,
                    ))
                }
                FieldType::Meta => Ok(Some(FieldValue::Meta(Meta::from(o)))),
                _ => Err(InvalidFieldError::UnrecognizedType(field_type.to_string()).into()),
            },
            serde_json::Value::Array(v) => Ok(Some(FieldValue::Objects(
                v.iter()
                    .enumerate()
                    .map(|(index, val)| {
                        let mut map = BTreeMap::new();
                        if let Some(obj) = val.as_object() {
                            for (k, v) in obj.iter() {
                                if let Some(value) = FieldValue::from_json(
                                    v,
                                    &parent_path.clone(),
                                    object_definition,
                                )? {
                                    map.insert(k.to_string(), value);
                                } else {
                                    warn!(
                                        "{} {} had invalid key {}, skipping.",
                                        parent_path, index, k
                                    );
                                }
                            }
                        } else {
                            panic!("Invalid value {} for child", val);
                        }
                        Ok::<_, anyhow::Error>(map)
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            ))),
        }
    }

    #[instrument(skip(value))]
    pub fn from_json(
        value: &serde_json::Value,
        field_path: &ValuePath,
        object_definition: &ObjectDefinition,
    ) -> Result<Option<Self>> {
        match field_path.get_field_definition(object_definition) {
            Ok(field_type) => {
                Self::field_from_json(value, field_type, field_path, object_definition)
            }
            Err(e) => {
                if matches!(e, ValuePathError::NotFound(..)) {
                    Ok(None)
                } else {
                    Err(e.into())
                }
            }
        }
    }
    #[instrument(skip(value))]
    pub fn from_toml(key: &String, field_type: &FieldType, value: &Value) -> Result<FieldValue> {
        match field_type {
            FieldType::String => Ok(FieldValue::String(
                value
                    .as_str()
                    .ok_or_else(|| InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    })?
                    .to_string(),
            )),
            FieldType::Secret => Ok(FieldValue::Secret(
                value
                    .as_str()
                    .ok_or_else(|| InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    })?
                    .to_string(),
            )),
            FieldType::Enum(valid_values) => {
                let value = value
                    .as_str()
                    .ok_or_else(|| InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    })?
                    .to_string();
                if !valid_values.contains(&value) {
                    Err(InvalidFieldError::EnumMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    })?
                } else {
                    Ok(FieldValue::Enum(value))
                }
            }
            FieldType::Markdown => Ok(FieldValue::Markdown(
                value
                    .as_str()
                    .ok_or_else(|| InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    })?
                    .to_string(),
            )),
            FieldType::Number => {
                let number = if let Some(float_val) = value.as_float() {
                    Ok(float_val)
                } else if let Some(int_val) = value.as_integer() {
                    Ok(int_val as f64)
                } else {
                    Err(InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    })
                }?;
                Ok(FieldValue::Number(number))
            }
            FieldType::Boolean => Ok(FieldValue::Boolean(value.as_bool().ok_or_else(|| {
                InvalidFieldError::TypeMismatch {
                    field: key.to_owned(),
                    field_type: field_type.to_string(),
                    value: value.to_string(),
                }
            })?)),
            FieldType::Date => {
                if let Value::Datetime(val) = value {
                    return Ok(FieldValue::Date(DateTime::from_toml(val)?));
                }
                let mut date_str =
                    (value
                        .as_str()
                        .ok_or_else(|| InvalidFieldError::TypeMismatch {
                            field: key.to_owned(),
                            field_type: field_type.to_string(),
                            value: value.to_string(),
                        })?)
                    .to_string();
                date_str = DateTime::parse_date_string(date_str)?;
                Ok(FieldValue::Date(DateTime::from(&date_str)?))
            }
            FieldType::Oneof(valid_types) => {
                let (type_name, selected_type, value) = value
                    .as_table()
                    .and_then(|info| {
                        let type_name = info.get("type")?.as_str()?;
                        let selected_type = valid_types.iter().find_map(|opt| {
                            if opt.name == type_name {
                                Some(opt.r#type.clone())
                            } else {
                                None
                            }
                        })?;
                        Some((type_name, selected_type, info.get("value")))
                    })
                    .ok_or_else(|| InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    })?;
                if !valid_types.iter().any(|t| t.r#type == selected_type) {
                    Err(InvalidFieldError::OneofMismatch {
                        field: key.to_owned(),
                        field_type: selected_type.to_string(),
                        value: match value {
                            Some(value) => value.to_string(),
                            None => "null".to_string(),
                        },
                    })?
                } else {
                    Ok(FieldValue::Oneof((
                        type_name.to_string(),
                        Box::new(match value {
                            Some(value) => {
                                Some(Self::from_toml(key, &selected_type, value).map_err(|_| {
                                    InvalidFieldError::TypeMismatch {
                                        field: format!("{}:{}", key, type_name),
                                        field_type: selected_type.to_string(),
                                        value: value.to_string(),
                                    }
                                })?)
                            }
                            None => None,
                        }),
                    )))
                }
            }
            FieldType::Audio => Ok(FieldValue::File(
                File::audio().fill_from_toml_map(value.as_table().ok_or_else(|| {
                    InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    }
                })?)?,
            )),
            FieldType::Video => Ok(FieldValue::File(
                File::video().fill_from_toml_map(value.as_table().ok_or_else(|| {
                    InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    }
                })?)?,
            )),
            FieldType::Upload => Ok(FieldValue::File(
                File::download().fill_from_toml_map(value.as_table().ok_or_else(|| {
                    InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    }
                })?)?,
            )),
            FieldType::Image => Ok(FieldValue::File(
                File::image().fill_from_toml_map(value.as_table().ok_or_else(|| {
                    InvalidFieldError::TypeMismatch {
                        field: key.to_owned(),
                        field_type: field_type.to_string(),
                        value: value.to_string(),
                    }
                })?)?,
            )),
            FieldType::Meta => Ok(FieldValue::Meta(Meta::from(value.as_table().ok_or_else(
                || InvalidFieldError::TypeMismatch {
                    field: key.to_owned(),
                    field_type: field_type.to_string(),
                    value: value.to_string(),
                },
            )?))),
            FieldType::Alias(a) => Self::from_toml(key, &a.0, value),
        }
    }

    // Note that this is the string used to render a value (via Display and
    // ValueView), so secrets are redacted here - use `as_secret` to read them.
    fn as_string(&self, config: Option<&FieldConfig>) -> String {
        match self {
            FieldValue::String(s) => s.clone(),
            FieldValue::Secret(_) => String::new(),
            FieldValue::Enum(s) => s.clone(),
            FieldValue::Markdown(n) => n.clone(),
            FieldValue::Number(n) => n.to_string(),
            FieldValue::Date(d) => d.as_liquid_datetime().to_rfc2822(),
            FieldValue::Boolean(b) => b.to_string(),
            FieldValue::Objects(o) => format!("{:?}", o),
            FieldValue::File(f) => format!(
                "{:?}",
                config
                    .map(|c| f.clone().into_map(Some(c)))
                    .expect("cannot render files without a config")
            ),
            FieldValue::Oneof((name, val)) => format!(
                "{name}:{}",
                match val.as_ref() {
                    Some(v) => v.as_string(config),
                    None => "null".to_string(),
                }
            ),
            FieldValue::Meta(m) => format!("{:?}", serde_json::Value::from(m)),
            FieldValue::Null => "null".to_string(),
        }
    }
}

#[cfg(test)]
pub mod enum_tests {

    use super::*;

    #[test]
    fn enum_value_validation_from_toml() -> Result<()> {
        let enum_field_type = FieldType::Enum(vec!["emo".to_string(), "metal".to_string()]);
        assert!(FieldValue::from_toml(
            &"some_key".to_string(),
            &enum_field_type,
            &Value::String("butt rock".to_string())
        )
        .is_err_and(|e| {
            e.downcast_ref::<InvalidFieldError>()
                .map(|inner| {
                    matches!(
                        inner,
                        InvalidFieldError::EnumMismatch {
                            field: _,
                            field_type: _,
                            value: _,
                        }
                    )
                })
                .unwrap_or(false)
        }));

        Ok(())
    }
    #[test]
    fn enum_value_validation_from_string() -> Result<()> {
        let enum_field_type = FieldType::Enum(vec!["emo".to_string(), "metal".to_string()]);
        assert!(FieldValue::from_string(
            &"some_key".to_string(),
            &enum_field_type,
            "butt rock".to_string()
        )
        .is_err_and(|inner| {
            matches!(
                inner,
                InvalidFieldError::EnumMismatch {
                    field: _,
                    field_type: _,
                    value: _,
                }
            )
        }));

        Ok(())
    }
}

#[cfg(test)]
pub mod markdown_tests {

    use super::*;

    // We use tagfilter
    // (https://github.github.com/gfm/#disallowed-raw-html-extension-) instead
    // of fully removing or disabling html, so most things users want to do will
    // still work.
    #[test]
    fn some_html_is_allowed() {
        // tricky; indenting these will cause them to be parsed as code, which
        // will fail the test.
        let value = FieldValue::Markdown(
            "# Hello!
here is some markdown.

Within it I can add some tags like: <a href=\"https://taskmastersbirthday.com\">links</a>
"
            .to_string(),
        );

        let rendered = value.as_scalar().expect("parsing failed").into_string();

        println!("rendered: {}", rendered);

        assert!(
            rendered.contains("<a href=\"https://"),
            "links are rendered properly"
        );
    }

    fn html(markdown: &str) -> String {
        FieldValue::Markdown(markdown.to_string())
            .as_scalar()
            .expect("parsing failed")
            .into_string()
            .to_string()
    }

    #[test]
    fn liquid_in_an_inline_code_span_is_not_liquid() {
        let rendered = html(r#"use `{{ posts | where: "slug", "a" }}` here"#);
        assert!(
            rendered
                .contains("<code>&#123;{ posts | where: &quot;slug&quot;, &quot;a&quot; }}</code>"),
            "code span was rewritten: {rendered}"
        );
    }

    #[test]
    fn liquid_in_a_code_fence_is_not_liquid() {
        let rendered = html("```liquid\n{% for p in posts %}{{ p.title }}{% endfor %}\n```");
        assert!(
            rendered.contains("&#123;% for p in posts %}&#123;{ p.title }}&#123;% endfor %}"),
            "code fence was rewritten: {rendered}"
        );
    }

    #[test]
    fn liquid_in_an_indented_code_block_is_not_liquid() {
        let rendered = html("text\n\n    {{ title }}\n");
        assert!(
            rendered.contains("&#123;{ title }}"),
            "indented block was rewritten: {rendered}"
        );
    }

    /// Only the brace that opens a delimiter is replaced, so braces that are
    /// just braces — the common case in a code sample — are left alone.
    #[test]
    fn braces_that_open_nothing_are_untouched() {
        let rendered = html("```\nfn main() { println!(\"{}\", 1) }\n```");
        assert!(
            rendered.contains("fn main() { println!(&quot;{}&quot;, 1) }"),
            "plain braces were escaped: {rendered}"
        );
    }

    /// The pass consumes each brace it replaces, so a run of them cannot leave
    /// a delimiter behind.
    #[test]
    fn runs_of_braces_leave_no_delimiter() {
        let rendered = html("`{{{{ x }}`");
        assert!(
            !rendered.contains("{{") && !rendered.contains("{%"),
            "delimiter survived: {rendered}"
        );
        // The last brace opens nothing once the three before it are gone.
        assert_eq!(rendered.matches("&#123;").count(), 3, "{rendered}");
    }

    /// Html the author wrote is not markdown's code, and liquid in it still
    /// renders — `{% raw %}` remains the way to opt out of that.
    #[test]
    fn liquid_in_handwritten_html_is_left_alone() {
        let rendered = html("<span>{{ title }}</span>");
        assert!(
            rendered.contains("<span>{{ title }}</span>"),
            "handwritten html was escaped: {rendered}"
        );
    }

    #[test]
    fn code_without_liquid_is_unchanged() {
        let rendered = html("`plain code`");
        assert!(
            rendered.contains("<code>plain code</code>") && !rendered.contains("&#123;"),
            "{rendered}"
        );
    }
}
#[cfg(test)]
pub mod file_tests {

    use liquid::model::State;

    use crate::{liquid_parser, MemoryFileSystem};

    use super::*;

    #[test]
    fn state_is_blank_when_invalid() {
        let file = File::image();
        assert!(!file.is_valid());
        let value = FieldValue::File(file);
        assert!(value.query_state(State::Blank));
        assert!(value.query_state(State::Empty));
        assert!(!value.query_state(State::Truthy));
    }

    #[test]
    fn is_not_truthy_if_invalid() {
        let mut file = File::image();
        assert!(!file.is_valid());
        file.filename = "image.png".to_string();
        file.sha = "fake-sha".to_string();
        let parser = liquid_parser::get(None, None, &MemoryFileSystem::default()).unwrap();
        let template = parser
            .parse("{% if file %}BLANK{% else %}OH NO!!{% endif %}")
            .unwrap();
        let field_config = FieldConfig::default();
        let ctx = liquid::object!({ "file": file.to_liquid(&field_config) });
        assert_eq!(template.render(&ctx).unwrap(), "BLANK");
    }
}
#[cfg(test)]
pub mod json_parsing_tests {
    use std::collections::HashMap;

    use ordermap::OrderMap;
    use serde_json::{json, Map, Value};
    use toml::Table;
    use tracing_test::traced_test;

    use crate::{fields::File, FieldValue, ObjectDefinition, ValuePath};

    pub fn object_definition_toml() -> &'static str {
        r#"
        [site]
        name = "string"
        description = "string"

        [post]
        title = "string"
        content = "markdown"
        excerpt = "string"
        date = "date"
        tags = "string"
        template = "post"
        [[post.media]]
        name = "image"
        type = "image"
        [[post.media]]
        name = "video"
        type = "video"
        [[post.media]]
        name = "audio"
        type = "audio"
        [[post.media]]
        name = "link"
        type = "string"
        "#
    }

    #[traced_test]
    #[test]
    fn parsing_basics() {
        let object_definitions =
            ObjectDefinition::from_source(object_definition_toml(), &OrderMap::new()).unwrap();
        let mut example = serde_json::json!({
          "post": [
            {
              "__filename": "welcome-post",
              "content": "Welcome to my personal blog! Here you'll find updates about my life, thoughts, and interests. Stay tuned for more posts.",
              "date": "2025-12-19 00:00:00",
              "excerpt": "Welcome to my blog! This is the first post.",
              "media": {
                "type": "image",
                "value": {
                  "description": "A welcoming image for my blog",
                  "display_type": "image",
                  "filename": "welcome-image.png",
                  "mime": "image/png",
                  "name": "Welcome Image",
                  "sha": "abc123def4567890abc123def4567890abc123def4567890abc123def4567890"
                }
              },
              "order": 1,
              "tags": "personal,blog,welcome",
              "title": "Hello World!"
            },
            {
              "__filename": "second-post",
              "content": "Today I want to share some thoughts on the importance of hobbies and taking time for oneself.",
              "date": "2025-12-19 00:00:00",
              "excerpt": "Reflecting on hobbies and self-care.",
              "media": {
                "type": "video",
                "value": {
                  "description": "A short video about my hobbies",
                  "display_type": "video",
                  "filename": "hobbies-video.mp4",
                  "mime": "video/mp4",
                  "name": "Hobbies Video",
                  "sha": "def456abc1237890def456abc1237890def456abc1237890def456abc1237890"
                }
              },
              "order": 2,
              "tags": "personal,blog,hobbies",
              "title": "My Hobbies"
            },
            {
              "__filename": "third-post",
              "content": "Sharing a recent picture from my trip to the mountains. Nature is truly breathtaking.",
              "date": "2025-12-19 00:00:00",
              "excerpt": "A beautiful mountain landscape.",
              "media": {
                "type": "image",
                "value": {
                  "description": "Scenic mountain view from my trip",
                  "display_type": "image",
                  "filename": "mountain-trip.jpg",
                  "mime": "image/jpeg",
                  "name": "Mountain Trip",
                  "sha": "789abc012def3456789abc012def3456789abc012def3456789abc012def345"
                }
              },
              "order": 3,
              "tags": "personal,blog,travel,photography",
              "title": "Mountain Adventure"
            },
            {
              "__filename": "fourth-post",
              "content": "Considering my favorite books and why they matter to me. Here are some recommendations.",
              "date": "2025-12-19 00:00:00",
              "excerpt": "My favorite books and why I love them.",
              "media": { "type": "link", "value": "https://mybookrecommendations.com" },
              "order": 4,
              "tags": "personal,blog,books,recommendations",
              "title": "My Favorite Books"
            },
            {
              "__filename": "fifth-post",
              "content": "Here's a quick update on my recent projects and future plans. Exciting stuff ahead!",
              "date": "2025-12-19 00:00:00",
              "excerpt": "Upcoming projects and goals.",
              "media": {
                "type": "audio",
                "value": {
                  "description": "Audio update about my projects",
                  "display_type": "audio",
                  "filename": "project-update.mp3",
                  "mime": "audio/mpeg",
                  "name": "Project Update",
                  "sha": "012def3456789abc012def3456789abc012def3456789abc012def3456789abc"
                }
              },
              "order": 5,
              "tags": "personal,blog,updates,projects",
              "title": "Project and Future Plans"
            },
            {
              "__filename": "sixth-post",
              "content": "This one is a work in progress and has no value yet",
              "date": "2025-12-19 00:00:00",
              "excerpt": "Upcoming projects and goals.",
              "media": {
                "type": "audio",
              },
              "order": 5,
              "tags": "personal,blog,updates,projects",
              "title": "Project and Future Plans"
            }
          ],
          "site": {
            "description": "I would like to make a blog",
            "name": "My Personal Blog",
            "order": 1
          }
        });
        fn get_fields(
            object: &Map<String, Value>,
            object_def: &ObjectDefinition,
        ) -> HashMap<String, FieldValue> {
            let mut fields = HashMap::new();
            for (key, value) in object.into_iter() {
                if key == "order" {
                    continue;
                };
                if let Some(value) =
                    FieldValue::from_json(value, &ValuePath::from_string(key), object_def).unwrap()
                {
                    fields.insert(key.to_string(), value);
                } else {
                    println!(
                        "skipping key {key} in {} (valid values: [{}])",
                        object_def.name,
                        object_def
                            .fields
                            .iter()
                            .map(|f| f.0.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                }
            }
            fields
        }
        let mut output = HashMap::new();
        for (obj_name, obj_val) in example.as_object_mut().unwrap() {
            if let serde_json::Value::Array(objects) = obj_val {
                let mut parsed_objs = HashMap::new();
                for obj_val in objects {
                    let object = obj_val.as_object_mut().unwrap();
                    let filename = object
                        .remove("__filename")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string();
                    let object_def = object_definitions.get(obj_name).unwrap();
                    let fields = get_fields(object, object_def);
                    parsed_objs.insert(filename, fields);
                }
                output.insert(obj_name.to_string(), parsed_objs);
            } else {
                let object = obj_val.as_object().unwrap();
                let object_def = object_definitions.get(obj_name).unwrap();
                let fields = get_fields(object, object_def);
                output.insert(
                    obj_name.to_string(),
                    HashMap::from([("root".to_string(), fields)]),
                );
            }
        }
        println!("PARSED: {:#?}", output);
        let posts = output.get("post").expect("missing posts");
        assert_eq!(posts.len(), 6);
        let site = output
            .get("site")
            .expect("missing site")
            .get("root")
            .expect("site was not a root object");
        assert_eq!(
            *site.get("description").unwrap(),
            FieldValue::String("I would like to make a blog".to_string())
        );
        assert_eq!(
            *site.get("name").unwrap(),
            FieldValue::String("My Personal Blog".to_string())
        );
        let fifth_post = posts.get("fifth-post").expect("missing fifth-post");
        assert_eq!(
            *fifth_post.get("tags").unwrap(),
            FieldValue::String("personal,blog,updates,projects".to_string())
        );
        assert_eq!(
            *fifth_post.get("media").unwrap(),
            FieldValue::Oneof((
                "audio".to_string(),
                Box::new(Some(FieldValue::File(
                    File::audio()
                        .fill_from_json_map(json!({
                          "description": "Audio update about my projects",
                          "display_type": "audio",
                          "filename": "project-update.mp3",
                          "mime": "audio/mpeg",
                          "name": "Project Update",
                          "sha": "012def3456789abc012def3456789abc012def3456789abc012def3456789abc"
                        }).as_object().unwrap())
                        .unwrap()
                )))
            ))
        );
        let sixth_post = posts.get("sixth-post").expect("missing sixth-post");
        assert_eq!(
            *sixth_post.get("media").unwrap(),
            FieldValue::Oneof(("audio".to_string(), Box::new(None)))
        );
    }

    #[traced_test]
    #[test]
    fn parsing_basics_from_toml() {
        let object_definitions =
            ObjectDefinition::from_source(object_definition_toml(), &OrderMap::new()).unwrap();

        let example_toml = r#"
        [[post]]
        __filename = "welcome-post"
        content = "Welcome to my personal blog! Here you'll find updates about my life, thoughts, and interests. Stay tuned for more posts."
        date = 2025-12-19T00:00:00
        excerpt = "Welcome to my blog! This is the first post."
        media = { type = "image", value = { description = "A welcoming image for my blog", display_type = "image", filename = "welcome-image.png", mime = "image/png", name = "Welcome Image", sha = "abc123def4567890abc123def4567890abc123def4567890abc123def4567890" } }
        order = 1
        tags = "personal,blog,welcome"
        title = "Hello World!"

        [[post]]
        __filename = "second-post"
        content = "Today I want to share some thoughts on the importance of hobbies and taking time for oneself."
        date = 2025-12-19T00:00:00
        excerpt = "Reflecting on hobbies and self-care."
        media = { type = "video", value = { description = "A short video about my hobbies", display_type = "video", filename = "hobbies-video.mp4", mime = "video/mp4", name = "Hobbies Video", sha = "def456abc1237890def456abc1237890def456abc1237890def456abc1237890" } }
        order = 2
        tags = "personal,blog,hobbies"
        title = "My Hobbies"

        [[post]]
        __filename = "third-post"
        content = "Sharing a recent picture from my trip to the mountains. Nature is truly breathtaking."
        date = 2025-12-19T00:00:00
        excerpt = "A beautiful mountain landscape."
        media = { type = "image", value = { description = "Scenic mountain view from my trip", display_type = "image", filename = "mountain-trip.jpg", mime = "image/jpeg", name = "Mountain Trip", sha = "789abc012def3456789abc012def3456789abc012def3456789abc012def345" } }
        order = 3
        tags = "personal,blog,travel,photography"
        title = "Mountain Adventure"

        [[post]]
        __filename = "fourth-post"
        content = "Considering my favorite books and why they matter to me. Here are some recommendations."
        date = 2025-12-19T00:00:00
        excerpt = "My favorite books and why I love them."
        media = { type = "link", value = "https://mybookrecommendations.com" }
        order = 4
        tags = "personal,blog,books,recommendations"
        title = "My Favorite Books"

        [[post]]
        __filename = "fifth-post"
        content = "Here's a quick update on my recent projects and future plans. Exciting stuff ahead!"
        date = 2025-12-19T00:00:00
        excerpt = "Upcoming projects and goals."
        media = { type = "audio", value = { description = "Audio update about my projects", display_type = "audio", filename = "project-update.mp3", mime = "audio/mpeg", name = "Project Update", sha = "012def3456789abc012def3456789abc012def3456789abc012def3456789abc" } }
        order = 5
        tags = "personal,blog,updates,projects"
        title = "Project and Future Plans"

        [[post]]
        __filename = "sixth-post"
        content = "This one is a work in progress and has no value yet"
        date = 2025-12-19T00:00:00
        excerpt = "Upcoming projects and goals."
        media = { type = "audio" }
        order = 5
        tags = "personal,blog,updates,projects"
        title = "Project and Future Plans"

        [site]
        description = "I would like to make a blog"
        name = "My Personal Blog"
        order = 1
        "#;

        let table: Table = toml::from_str(example_toml).unwrap();
        let parsed_table = table.clone();

        fn get_fields(
            object: &toml::value::Table,
            object_def: &ObjectDefinition,
        ) -> HashMap<String, FieldValue> {
            let mut fields = HashMap::new();
            for (key, value) in object.iter() {
                if key == "order" || key == "__filename" {
                    continue;
                };
                if let Some(field_type) = object_def.field_type(key) {
                    let fv = FieldValue::from_toml(&key.to_string(), field_type, value).unwrap();
                    fields.insert(key.to_string(), fv);
                } else {
                    println!(
                        "skipping key {} in {} (valid values: [{}])",
                        key,
                        object_def.name,
                        object_def
                            .fields
                            .iter()
                            .map(|f| f.0.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                }
            }
            fields
        }

        let mut output = HashMap::new();
        for (obj_name, obj_val) in table.into_iter() {
            if let toml::Value::Array(objects) = obj_val {
                let mut parsed_objs = HashMap::new();
                for obj_val in objects {
                    let object = obj_val.as_table().unwrap();
                    let filename = object
                        .get("__filename")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string();
                    let object_def = object_definitions.get(obj_name.as_str()).unwrap();
                    let fields = get_fields(object, object_def);
                    parsed_objs.insert(filename, fields);
                }
                output.insert(obj_name.to_string(), parsed_objs);
            } else {
                let object = obj_val.as_table().unwrap();
                let object_def = object_definitions.get(obj_name.as_str()).unwrap();
                let fields = get_fields(object, object_def);
                output.insert(
                    obj_name.to_string(),
                    HashMap::from([("root".to_string(), fields)]),
                );
            }
        }

        let posts = output.get("post").expect("missing posts");
        assert_eq!(posts.len(), 6);
        let site = output
            .get("site")
            .expect("missing site")
            .get("root")
            .expect("site was not a root object");
        assert_eq!(
            *site.get("description").unwrap(),
            FieldValue::String("I would like to make a blog".to_string())
        );
        assert_eq!(
            *site.get("name").unwrap(),
            FieldValue::String("My Personal Blog".to_string())
        );
        let fifth_post = posts.get("fifth-post").expect("missing fifth-post");
        assert_eq!(
            *fifth_post.get("tags").unwrap(),
            FieldValue::String("personal,blog,updates,projects".to_string())
        );

        // construct expected file from parsed toml
        let posts_array = parsed_table.get("post").unwrap().as_array().unwrap();
        let fifth_media_value = posts_array[4]
            .as_table()
            .unwrap()
            .get("media")
            .unwrap()
            .as_table()
            .unwrap();
        let fifth_media_inner_value = fifth_media_value.get("value").unwrap().as_table().unwrap();
        let expected_file = File::audio()
            .fill_from_toml_map(fifth_media_inner_value)
            .unwrap();
        assert_eq!(
            *fifth_post.get("media").unwrap(),
            FieldValue::Oneof((
                "audio".to_string(),
                Box::new(Some(FieldValue::File(expected_file)))
            ))
        );
        let sixth_post = posts.get("sixth-post").expect("missing sixth-post");
        assert_eq!(
            *sixth_post.get("media").unwrap(),
            FieldValue::Oneof(("audio".to_string(), Box::new(None)))
        );
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use ordermap::OrderMap;

    #[test]
    fn type_mismatch_returns_type_mismatch() {
        let object_definitions = ObjectDefinition::from_source(
            json_parsing_tests::object_definition_toml(),
            &OrderMap::new(),
        )
        .unwrap();
        let post_def = object_definitions.get("post").unwrap();
        let path = ValuePath::from_string("date");
        let v = FieldValue::String("not a date".to_string());
        let err = v
            .validate(&path, post_def, &EditorTypes::new())
            .unwrap_err();
        assert!(matches!(err, FieldValueValidationError::TypeMismatch(_, p, _) if p == path));
    }

    #[test]
    fn field_definition_not_found_returns_error() {
        let object_definitions = ObjectDefinition::from_source(
            json_parsing_tests::object_definition_toml(),
            &OrderMap::new(),
        )
        .unwrap();
        let post_def = object_definitions.get("post").unwrap();
        let path = ValuePath::from_string("does_not_exist");
        let v = FieldValue::String("x".to_string());
        let err = v
            .validate(&path, post_def, &EditorTypes::new())
            .unwrap_err();
        assert!(
            matches!(err, FieldValueValidationError::FieldDefinitionNotFound(pth, _, _) if pth == path)
        );
    }

    #[test]
    fn invalid_oneof_name_returns_error() {
        let object_definitions = ObjectDefinition::from_source(
            json_parsing_tests::object_definition_toml(),
            &OrderMap::new(),
        )
        .unwrap();
        let post_def = object_definitions.get("post").unwrap();
        let path = ValuePath::from_string("media");
        let v = FieldValue::Oneof(("notatype".to_string(), Box::new(None)));
        let err = v
            .validate(&path, post_def, &EditorTypes::new())
            .unwrap_err();
        assert!(matches!(err, FieldValueValidationError::InvalidOneofName(_, p, _) if p == path));
    }

    #[test]
    fn invalid_oneof_type_returns_error() {
        let object_definitions = ObjectDefinition::from_source(
            json_parsing_tests::object_definition_toml(),
            &OrderMap::new(),
        )
        .unwrap();
        let post_def = object_definitions.get("post").unwrap();
        let path = ValuePath::from_string("media");
        // Provide an invalid inner value so validation fails (None values validate)
        let v = FieldValue::Oneof((
            "audio".to_string(),
            Box::new(Some(FieldValue::String("not-an-audio".to_string()))),
        ));
        let err = v
            .validate(&path, post_def, &EditorTypes::new())
            .unwrap_err();
        assert!(matches!(err, FieldValueValidationError::TypeMismatch(_, p, _) if p == path));
    }
}

#[cfg(test)]
mod from_string_alias_tests {
    use super::*;
    use crate::manifest::Manifest;
    use std::path::Path;

    fn editor_types() -> EditorTypes {
        Manifest::from_string(
            Path::new(""),
            r#"
[editor_types.slug]
type = "string"
validate = ['[a-z-]+']

[editor_types.short_slug]
type = "slug"

[editor_types.day]
type = "date"
"#
            .to_string(),
            None,
        )
        .unwrap()
        .editor_types
    }

    #[test]
    fn aliased_types_are_parsed_as_their_underlying_type() {
        let types = editor_types();
        let slug = FieldType::from_str("slug", &types).unwrap();
        assert_eq!(
            FieldValue::from_string(&"name".to_string(), &slug, "a-slug".to_string()).unwrap(),
            FieldValue::String("a-slug".to_string())
        );
        let day = FieldType::from_str("day", &types).unwrap();
        assert!(matches!(
            FieldValue::from_string(&"when".to_string(), &day, "01/21/1987".to_string()).unwrap(),
            FieldValue::Date(_)
        ));
    }

    #[test]
    fn aliases_of_aliases_are_parsed_as_their_base_type() {
        let types = editor_types();
        let short_slug = FieldType::from_str("short_slug", &types).unwrap();
        assert_eq!(
            FieldValue::from_string(&"name".to_string(), &short_slug, "a-slug".to_string())
                .unwrap(),
            FieldValue::String("a-slug".to_string())
        );
    }

    #[test]
    fn aliased_types_use_their_underlying_default() {
        let types = editor_types();
        let slug = FieldType::from_str("slug", &types).unwrap();
        assert_eq!(
            FieldValue::from_string(&"name".to_string(), &slug, "".to_string()).unwrap(),
            FieldValue::String("".to_string())
        );
    }
}
