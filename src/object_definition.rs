#[cfg(feature = "json-schema")]
use crate::object::ValuePath;
use crate::{
    definition_comments::{extract_comments, DefinitionComments},
    fields::{field_type::InvalidFieldError, FieldType, ObjectValues, OneofOption},
    manifest::EditorTypes,
    reserved_fields::{self, is_reserved_field, reserved_field_from_str, ReservedFieldError},
    FieldValue,
};
use anyhow::Result;
use ordermap::OrderMap;
use serde::{Deserialize, Serialize};
#[cfg(feature = "json-schema")]
use serde_json::json;
use std::fmt::Debug;
use toml::Table;
use tracing::instrument;

pub type ObjectDefinitions = OrderMap<String, ObjectDefinition>;
#[cfg(feature = "typescript")]
pub mod typedefs {
    use typescript_type_def::{
        type_expr::{Ident, NativeTypeInfo, TypeExpr, TypeInfo, TypeName},
        TypeDef,
    };

    use crate::{object_definition::FieldDefinition, ObjectDefinition};
    pub struct ObjectDefinitionsDef;
    impl TypeDef for ObjectDefinitionsDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            r#ref: TypeExpr::Name(TypeName {
                path: &[],
                name: Ident("Record"),
                generic_args: &[
                    TypeExpr::Ref(&String::INFO),
                    TypeExpr::Ref(&ObjectDefinition::INFO),
                ],
            }),
        });
    }
    pub struct FieldsMapDef;
    impl TypeDef for FieldsMapDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            r#ref: TypeExpr::Name(TypeName {
                path: &[],
                name: Ident("Record"),
                generic_args: &[
                    TypeExpr::Ref(&String::INFO),
                    TypeExpr::Ref(&FieldDefinition::INFO),
                ],
            }),
        });
    }

    pub struct ChildrenDef;
    impl TypeDef for ChildrenDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            r#ref: TypeExpr::ident(Ident("Record<string, ObjectDefinition>")),
        });
    }
}

pub type FieldsMap = OrderMap<String, FieldDefinition>;

/// A field's type, plus whatever the schema author wrote about it.
///
/// `description` comes from the comment above the field in
/// `archival_objects.toml` (see [`crate::definition_comments`]) and exists so
/// editors and generated types can explain a field rather than just naming it.
#[derive(Debug, Serialize, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct FieldDefinition {
    pub r#type: FieldType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl FieldDefinition {
    pub fn new(r#type: FieldType, description: Option<String>) -> Self {
        Self {
            r#type,
            description,
        }
    }
}

impl From<FieldType> for FieldDefinition {
    fn from(r#type: FieldType) -> Self {
        Self {
            r#type,
            description: None,
        }
    }
}

/// Accepts both the current form (`{"type": "String", "description": ...}`) and
/// the bare `FieldType` this used to serialize as (`"String"`), so definitions
/// persisted by older versions still load.
impl<'de> Deserialize<'de> for FieldDefinition {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Described {
                r#type: FieldType,
                #[serde(default)]
                description: Option<String>,
            },
            Bare(FieldType),
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Described {
                r#type,
                description,
            } => Self {
                r#type,
                description,
            },
            Repr::Bare(r#type) => Self::from(r#type),
        })
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Hash)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct ObjectDefinition {
    pub name: String,
    #[cfg_attr(feature = "typescript", type_def(type_of = "typedefs::FieldsMapDef"))]
    pub fields: FieldsMap,
    pub template: Option<String>,

    #[cfg_attr(feature = "typescript", type_def(type_of = "typedefs::ChildrenDef"))]
    pub children: ObjectDefinitions,

    /// The comment above this object's `[header]` in `archival_objects.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ObjectDefinition {
    /// Parses the contents of an `archival_objects.toml` file.
    ///
    /// The source is parsed twice: once by `toml` for the definitions, and once
    /// by `toml_edit` for the comments that describe them.
    pub fn from_source(source: &str, editor_types: &EditorTypes) -> Result<ObjectDefinitions> {
        let table: Table = toml::from_str(source)?;
        // Comments are a convenience, never a reason to fail a parse that
        // `toml` just accepted - both crates track the same TOML spec version,
        // so disagreement here would be a bug rather than an authoring error.
        let comments = extract_comments(source).unwrap_or_default();
        Self::from_table(&table, &comments, editor_types)
    }

    /// A top-level object, whose name also becomes a key of every page's context.
    pub(crate) fn new_root(
        name: &str,
        definition: &Table,
        comments: &DefinitionComments,
        editor_types: &EditorTypes,
    ) -> Result<ObjectDefinition> {
        if reserved_fields::is_reserved_object_name(name) {
            return Err(InvalidFieldError::ReservedObjectNameError(name.to_string()).into());
        }
        Self::new(name, definition, comments, editor_types)
    }

    pub fn new(
        name: &str,
        definition: &Table,
        comments: &DefinitionComments,
        editor_types: &EditorTypes,
    ) -> Result<ObjectDefinition> {
        if is_reserved_field(name) {
            return Err(InvalidFieldError::ReservedObjectNameError(name.to_string()).into());
        }
        let mut obj_def = ObjectDefinition {
            name: name.to_string(),
            fields: FieldsMap::new(),
            template: None,
            children: ObjectDefinitions::new(),
            description: comments.own.clone(),
        };
        for (key, m_value) in definition {
            if is_reserved_field(key)
                && !(m_value.as_str().is_some() && key == reserved_fields::TEMPLATE)
            {
                return Err(ReservedFieldError {
                    field: reserved_field_from_str(key),
                }
                .into());
            }
            if let Some(child_table) = m_value.as_table() {
                obj_def.children.insert(
                    key.clone(),
                    ObjectDefinition::new(key, child_table, comments.child(key), editor_types)?,
                );
            } else if let Some(value) = m_value.as_array() {
                if let Some(tables) = value
                    .iter()
                    .map(|v| v.as_table())
                    .collect::<Option<Vec<_>>>()
                {
                    // this is an array of tables, try to parse as oneof
                    if let Some(options) = tables
                        .iter()
                        .map(|info| {
                            let name = info.get("name")?.as_str()?;
                            let type_name = info.get("type")?.as_str()?;
                            let def_type = FieldType::from_str(type_name, editor_types).ok()?;
                            Some((name, def_type))
                        })
                        .collect::<Option<Vec<_>>>()
                    {
                        let options = options
                            .into_iter()
                            .map(|(name, def_type)| OneofOption {
                                name: name.to_string(),
                                r#type: def_type,
                            })
                            .collect();
                        obj_def.fields.insert(
                            key.clone(),
                            FieldDefinition::new(FieldType::Oneof(options), comments.field(key)),
                        );
                    } else {
                        return Err(InvalidFieldError::InvalidOneof(format!("{tables:?}")).into());
                    }
                } else {
                    // not objects, try to parse as enum
                    let string_vals = value
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(|s| s.to_string())
                                .ok_or_else(|| InvalidFieldError::InvalidEnum(format!("{value:?}")))
                        })
                        .collect::<Result<Vec<String>, InvalidFieldError>>()?;
                    obj_def.fields.insert(
                        key.clone(),
                        FieldDefinition::new(FieldType::Enum(string_vals), comments.field(key)),
                    );
                }
            } else if let Some(value) = m_value.as_str() {
                if key == reserved_fields::TEMPLATE {
                    obj_def.template = Some(value.to_string());
                } else {
                    obj_def.fields.insert(
                        key.clone(),
                        FieldDefinition::new(
                            FieldType::from_str(value, editor_types)?,
                            comments.field(key),
                        ),
                    );
                }
            }
        }
        Ok(obj_def)
    }
    pub fn from_table(
        table: &Table,
        comments: &DefinitionComments,
        editor_types: &EditorTypes,
    ) -> Result<ObjectDefinitions> {
        let mut objects = ObjectDefinitions::new();
        for (name, m_def) in table.into_iter() {
            if let Some(def) = m_def.as_table() {
                objects.insert(
                    name.clone(),
                    ObjectDefinition::new_root(name, def, comments.child(name), editor_types)?,
                );
            }
        }
        Ok(objects)
    }

    /// The type of a field on this object, ignoring its description.
    pub fn field_type(&self, key: &str) -> Option<&FieldType> {
        self.fields.get(key).map(|field| &field.r#type)
    }

    #[instrument(skip(self))]
    pub fn empty_object(&self) -> ObjectValues {
        let mut values: ObjectValues = ObjectValues::new();
        for def in self.children.values() {
            values.insert(def.name.to_owned(), FieldValue::Objects(vec![]));
        }
        values
    }
}

#[cfg(feature = "json-schema")]
impl ObjectDefinition {
    pub fn to_json_schema_properties(
        &self,
        is_child: bool,
        options: &mut crate::json_schema::ObjectSchemaOptions,
        current_path: ValuePath,
    ) -> crate::json_schema::ObjectSchema {
        let mut properties = serde_json::Map::new();
        for (field, field_def) in &self.fields {
            let field_path = current_path.clone().append(ValuePath::key(field));
            // Skip if this path is omitted
            if options
                .omit_paths
                .as_ref()
                .is_some_and(|op| op.contains(&field_path))
            {
                continue;
            }
            // Fall back to the field name when the schema author didn't
            // describe it - that's all this had to go on before descriptions.
            let description = field_def.description.as_deref().unwrap_or(field);
            let field_props =
                field_def
                    .r#type
                    .to_json_schema_property(description, &field_path, options);
            properties.insert(field.into(), field_props.into());
        }
        if !is_child {
            properties.insert("order".into(), json!({"type": "number"}));
        }
        for (name, definition) in &self.children {
            let child_path = current_path.clone().append(ValuePath::key(name));
            // Skip if this path is omitted
            if options
                .omit_paths
                .as_ref()
                .is_some_and(|op| op.contains(&child_path))
            {
                continue;
            }
            let mut child = serde_json::Map::new();
            child.insert(
                "description".into(),
                definition.description.as_deref().unwrap_or(name).into(),
            );
            child.insert("type".into(), "array".into());
            let child_properties = definition.to_json_schema_properties(true, options, child_path);
            let mut child_items_type = serde_json::Map::new();
            child_items_type.insert("type".into(), "object".into());
            child_items_type.insert("additionalProperties".into(), false.into());
            if options.all_fields_required {
                let keys: Vec<String> = child_properties.keys().map(|k| k.to_string()).collect();
                child_items_type.insert("required".into(), keys.into());
            }
            child_items_type.insert("properties".into(), child_properties.into());
            child.insert("items".into(), child_items_type.into());
            properties.insert(name.into(), child.into());
        }
        properties
    }
}

#[cfg(test)]
pub mod tests {

    use super::*;

    pub fn artist_and_example_definition_str() -> &'static str {
        r#"
        # A musical act on the roster.
        [artists]
        # The artist's display name.
        # Shown in listings and page titles.
        name = "string"
        meta = "meta"
        genre = ["emo", "metal"]
        template = "artist"
        # Media attached to this artist.
        [[artists.media]]
        name = "video"
        type = "video"
        [[artists.media]]
        name = "photo"
        type = "image"
        # Upcoming shows.
        [artists.tour_dates]
        date = "date"
        ticket_link = "string"
        [artists.videos]
        video = "video"
        [artists.numbers]
        number = "number"

        [example]
        content = "markdown"
        [example.links]
        url = "string"
        "#
    }

    pub fn artist_and_example_definitions() -> ObjectDefinitions {
        ObjectDefinition::from_source(artist_and_example_definition_str(), &OrderMap::new())
            .unwrap()
    }

    #[test]
    fn parsing() {
        let defs = artist_and_example_definitions();

        println!("{:?}", defs);

        assert_eq!(defs.keys().len(), 2);
        assert!(defs.contains_key("artists"));
        assert!(defs.contains_key("example"));
        let artist = defs.get("artists").unwrap();
        let fields = artist.fields.keys();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[0], "name".to_string());
        assert_eq!(fields[1], "meta".to_string());
        assert_eq!(fields[2], "genre".to_string());
        assert_eq!(fields[3], "media".to_string());
        let oneof_field = artist.field_type("media").unwrap();
        assert!(matches!(oneof_field, FieldType::Oneof(_)));
        if let FieldType::Oneof(field) = oneof_field {
            assert_eq!(field.len(), 2);
            assert_eq!(field[0].name, "video");
            assert_eq!(field[1].name, "photo");
            assert!(matches!(field[0].r#type, FieldType::Video));
            assert!(matches!(field[1].r#type, FieldType::Image));
        }
        let children = artist.children.keys();
        assert_eq!(children.len(), 3);
        assert_eq!(children[0], "tour_dates".to_string());
        assert_eq!(children[1], "videos".to_string());
        assert_eq!(children[2], "numbers".to_string());
        assert!(artist.fields.contains_key("name"));
        assert_eq!(artist.field_type("name").unwrap(), &FieldType::String);
        assert!(
            !artist.fields.contains_key("template"),
            "did not copy the template reserved field"
        );
        assert!(artist.fields.contains_key("name"));
        assert_eq!(
            artist.field_type("genre").unwrap(),
            &FieldType::Enum(vec!["emo".to_string(), "metal".to_string()])
        );
        assert!(artist.template.is_some());
        assert_eq!(artist.template, Some("artist".to_string()));
        assert_eq!(artist.children.len(), 3);
        assert!(artist.children.contains_key("tour_dates"));
        assert!(artist.children.contains_key("numbers"));
        let tour_dates = artist.children.get("tour_dates").unwrap();
        assert!(tour_dates.fields.contains_key("date"));
        assert_eq!(tour_dates.field_type("date").unwrap(), &FieldType::Date);
        assert!(tour_dates.fields.contains_key("ticket_link"));
        assert_eq!(
            tour_dates.field_type("ticket_link").unwrap(),
            &FieldType::String
        );
        let numbers = artist.children.get("numbers").unwrap();
        assert!(numbers.fields.contains_key("number"));
        assert_eq!(numbers.field_type("number").unwrap(), &FieldType::Number);
        let numbers = artist.children.get("videos").unwrap();
        assert!(numbers.fields.contains_key("video"));
        assert_eq!(numbers.field_type("video").unwrap(), &FieldType::Video);

        let example = defs.get("example").unwrap();
        assert!(example.fields.contains_key("content"));
        assert_eq!(example.field_type("content").unwrap(), &FieldType::Markdown);
        assert_eq!(example.children.len(), 1);
        assert!(example.children.contains_key("links"));
        let links = example.children.get("links").unwrap();
        assert!(links.fields.contains_key("url"));
        assert_eq!(links.field_type("url").unwrap(), &FieldType::String);
    }

    fn description_of(def: &ObjectDefinition, field: &str) -> Option<String> {
        def.fields.get(field).unwrap().description.clone()
    }

    #[test]
    fn comments_become_descriptions() {
        let defs = artist_and_example_definitions();
        let artist = defs.get("artists").unwrap();

        // The comment above [artists].
        assert_eq!(
            artist.description,
            Some("A musical act on the roster.".to_string())
        );
        // A multi-line block above a field.
        assert_eq!(
            description_of(artist, "name"),
            Some("The artist's display name.\nShown in listings and page titles.".to_string())
        );
        // A oneof, described by the comment above its first [[artists.media]].
        assert_eq!(
            description_of(artist, "media"),
            Some("Media attached to this artist.".to_string())
        );
        // A child object, described by the comment above its header.
        let tour_dates = artist.children.get("tour_dates").unwrap();
        assert_eq!(tour_dates.description, Some("Upcoming shows.".to_string()));

        // Undescribed fields, children, and objects stay undescribed.
        assert_eq!(description_of(artist, "meta"), None);
        assert_eq!(description_of(tour_dates, "date"), None);
        assert_eq!(artist.children.get("videos").unwrap().description, None);
        assert_eq!(defs.get("example").unwrap().description, None);
    }

    #[test]
    fn descriptions_survive_the_inline_forms() {
        // toml_edit models `[[x]]` and `x = [{..}]` differently, where toml
        // treats both as an array of tables. The same for `[x]` versus an
        // inline table. Since descriptions come from one parser and fields from
        // the other, the two views have to agree on where a key lives.
        let defs = ObjectDefinition::from_source(
            r#"
            [artists]
            # Media attached to this artist.
            media = [{ name = "video", type = "video" }]
            # Upcoming shows.
            tour_dates = { date = "date" }
            "#,
            &OrderMap::new(),
        )
        .unwrap();
        let artist = defs.get("artists").unwrap();

        assert!(matches!(
            artist.field_type("media").unwrap(),
            FieldType::Oneof(_)
        ));
        assert_eq!(
            description_of(artist, "media"),
            Some("Media attached to this artist.".to_string())
        );

        let tour_dates = artist.children.get("tour_dates").unwrap();
        assert_eq!(tour_dates.field_type("date").unwrap(), &FieldType::Date);
        assert_eq!(tour_dates.description, Some("Upcoming shows.".to_string()));
    }

    #[test]
    fn a_field_definition_deserializes_from_either_form() {
        // Definitions persisted before descriptions existed serialized a field
        // as a bare FieldType.
        let bare: FieldDefinition = serde_json::from_str(r#""String""#).unwrap();
        assert_eq!(bare, FieldType::String.into());

        let bare_enum: FieldDefinition = serde_json::from_str(r#"{"Enum":["a","b"]}"#).unwrap();
        assert_eq!(
            bare_enum,
            FieldType::Enum(vec!["a".to_string(), "b".to_string()]).into()
        );

        let described: FieldDefinition =
            serde_json::from_str(r#"{"type":"String","description":"The name."}"#).unwrap();
        assert_eq!(
            described,
            FieldDefinition::new(FieldType::String, Some("The name.".to_string()))
        );

        // ...and the current form round-trips.
        let round_tripped: FieldDefinition =
            serde_json::from_str(&serde_json::to_string(&described).unwrap()).unwrap();
        assert_eq!(round_tripped, described);
    }

    #[test]
    fn an_undescribed_field_serializes_without_a_description() {
        let json = serde_json::to_string(&FieldDefinition::from(FieldType::String)).unwrap();
        assert_eq!(json, r#"{"type":"String"}"#);
    }
}
