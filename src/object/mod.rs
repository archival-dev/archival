pub use crate::value_path::{ValuePath, ValuePathComponent, ValuePathError};
use crate::{
    events::AddObjectValue,
    fields::{field_value::RenderedObjectValues, FieldValue, InvalidFieldError, ObjectValues},
    manifest::EditorTypes,
    object_definition::ObjectDefinition,
    reserved_fields::{self, is_reserved_field},
    util::{integer_decode, path_to_slash},
    FieldConfig,
};
use anyhow::Result;
use context_value::ContextObject;
use liquid::{model::KString, ObjectView, ValueView};
use ordermap::OrderMap;
use serde::{Deserialize, Serialize};
use std::{
    fmt::Debug,
    hash::Hash,
    path::{Path, PathBuf},
};
use to_liquid::{object_to_liquid_with, ToLiquidOptions};
use toml::Table;
use tracing::{instrument, warn};
pub(crate) mod context_value;
mod object_entry;
pub(crate) mod to_liquid;
pub use object_entry::{ObjectEntry, RenderedObjectEntry};

pub type ObjectMap = OrderMap<String, ObjectEntry>;
pub type RenderedObjectMap = OrderMap<String, RenderedObjectEntry>;

pub trait Renderable {
    type Output;
    fn rendered(self, field_config: &FieldConfig) -> Self::Output;
}

impl Renderable for ObjectMap {
    type Output = RenderedObjectMap;
    fn rendered(self, field_config: &FieldConfig) -> Self::Output {
        self.into_iter()
            .map(|(k, o)| (k, o.rendered(field_config)))
            .collect()
    }
}

#[cfg(feature = "typescript")]
pub mod typedefs {
    use typescript_type_def::{
        type_expr::{Ident, NativeTypeInfo, TypeExpr, TypeInfo, TypeName},
        TypeDef,
    };

    use crate::object::RenderedObjectEntry;
    pub struct RenderedObjectMapDef;
    impl TypeDef for RenderedObjectMapDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            r#ref: TypeExpr::Name(TypeName {
                path: &[],
                name: Ident("Record"),
                generic_args: &[
                    TypeExpr::Ref(&String::INFO),
                    TypeExpr::Ref(&RenderedObjectEntry::INFO),
                ],
            }),
        });
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct RenderedObject {
    pub filename: String,
    pub object_name: String,
    pub order: Option<f64>,
    pub path: PathBuf,
    pub values: RenderedObjectValues,
}
impl Hash for RenderedObject {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.filename.hash(state);
        self.object_name.hash(state);
        self.order.map(integer_decode).hash(state);
        self.path.hash(state);
        self.values.hash(state);
    }
}

#[derive(Debug, ObjectView, ValueView, Deserialize, Serialize, Clone, PartialEq)]
pub struct Object {
    pub filename: String,
    pub object_name: String,
    pub order: Option<f64>,
    pub values: ObjectValues,
}
impl Object {
    pub fn path(&self) -> PathBuf {
        Path::new(&self.object_name).join(self.filename.clone())
    }
    /// The object's path as it is exposed to templates. This is used to build
    /// urls, so it always uses `/` separators, even on windows.
    pub fn url_path(&self) -> String {
        path_to_slash(self.path())
    }
}

impl Hash for Object {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.filename.hash(state);
        self.object_name.hash(state);
        self.order.map(integer_decode).hash(state);
        self.values.hash(state);
    }
}
impl Renderable for Object {
    type Output = RenderedObject;
    fn rendered(self, field_config: &FieldConfig) -> Self::Output {
        // Rendered objects are consumed by editors, which use this path to
        // build urls, so it is stored with `/` separators on every platform.
        let path = PathBuf::from(self.url_path());
        Self::Output {
            filename: self.filename,
            object_name: self.object_name,
            order: self.order,
            path,
            values: self
                .values
                .into_iter()
                .map(|(k, v)| (k, v.rendered(field_config)))
                .collect(),
        }
    }
}

impl Object {
    #[instrument(skip(definition, table))]
    pub fn values_from_table(
        file: &Path,
        table: &Table,
        definition: &ObjectDefinition,
        custom_types: &EditorTypes,
        skip_validation: bool,
    ) -> Result<ObjectValues> {
        // liquid-rust only supports strict parsing. This is reasonable but we
        // also want to allow empty root keys, so we fill in defaults for any
        // missing definition keys
        let mut values = definition.empty_object();
        for (type_name, value) in table {
            let def_key = custom_types
                .get(type_name)
                .map(|i| &i.alias_of)
                .unwrap_or(type_name);
            if let Some(field_type) = definition.field_type(def_key) {
                // Values
                let field_value = FieldValue::from_toml(def_key, field_type, value)?;
                if !skip_validation {
                    field_value.validate(
                        &ValuePath::from_string(def_key),
                        definition,
                        custom_types,
                    )?;
                }
                values.insert(def_key.to_string(), field_value);
            } else if let Some(child_def) = definition.children.get(&def_key.to_string()) {
                // Children
                let m_objects = value
                    .as_array()
                    .ok_or_else(|| InvalidFieldError::NotAnArray {
                        key: def_key.to_string(),
                        value: value.to_string(),
                    })?;
                let mut objects: Vec<ObjectValues> = Vec::new();
                for (index, object) in m_objects.iter().enumerate() {
                    let table =
                        object
                            .as_table()
                            .ok_or_else(|| InvalidFieldError::InvalidChild {
                                key: def_key.to_owned(),
                                index,
                                child: value.to_string(),
                            })?;
                    let object = Object::values_from_table(
                        file,
                        table,
                        child_def,
                        custom_types,
                        skip_validation,
                    )?;
                    objects.push(object);
                }
                let field_value = FieldValue::Objects(objects);

                values.insert(def_key.to_string(), field_value);
            } else if !is_reserved_field(def_key) {
                warn!("{}: unknown field {}", file.display(), def_key);
            }
        }
        Ok(values)
    }

    #[instrument(skip(definition, table))]
    pub fn from_table(
        definition: &ObjectDefinition,
        file: &Path,
        table: &Table,
        custom_types: &EditorTypes,
        skip_validation: bool,
    ) -> Result<Object> {
        let values =
            Object::values_from_table(file, table, definition, custom_types, skip_validation)?;
        let mut order = None;
        if let Some(t_order) = table.get(reserved_fields::ORDER) {
            if let Some(int_order) = t_order.as_integer() {
                order = Some(int_order as f64);
            } else if let Some(float_order) = t_order.as_float() {
                order = Some(float_order);
            } else {
                warn!("Invalid order {}", t_order);
            }
        }
        let filename = file.file_name().unwrap().to_string_lossy().to_string();
        let object = Object {
            filename,
            object_name: definition.name.clone(),
            order,
            values,
        };
        Ok(object)
    }

    pub fn from_def(
        definition: &ObjectDefinition,
        filename: &str,
        order: Option<f64>,
        defaults: Vec<AddObjectValue>,
    ) -> Result<Self> {
        let path = Path::new(&definition.name).join(filename);
        let values =
            Object::values_from_table(&path, &Table::new(), definition, &OrderMap::new(), false)?;
        let mut object = Self {
            filename: filename.to_owned(),
            object_name: definition.name.clone(),
            order,
            values,
        };
        for default in defaults {
            default
                .path
                .set_in_object(&mut object, Some(default.value))?;
        }
        Ok(object)
    }

    pub fn to_toml(&self, definition: &ObjectDefinition) -> Result<String, toml::ser::Error> {
        let mut write_obj = Table::new();
        if let Some(order) = self.order {
            write_obj.insert(
                "order".to_string(),
                if order.fract() == 0. {
                    toml::Value::Integer(order as i64)
                } else {
                    toml::Value::Float(order)
                },
            );
        }
        for key in definition.fields.keys() {
            if let Some(value) = self.values.get(key) {
                if let Some(val) = value.into() {
                    write_obj.insert(key.to_string(), val);
                }
            }
        }
        for key in definition.children.keys() {
            if let Some(value) = self.values.get(key) {
                if let Some(val) = value.into() {
                    write_obj.insert(key.to_string(), val);
                }
            }
        }
        toml::to_string_pretty(&write_obj)
    }

    pub fn liquid_object(
        &self,
        definition: &ObjectDefinition,
        field_config: &FieldConfig,
    ) -> ContextObject {
        self.liquid_object_with(definition, field_config, ToLiquidOptions::default())
    }

    pub fn liquid_object_with(
        &self,
        definition: &ObjectDefinition,
        field_config: &FieldConfig,
        options: ToLiquidOptions,
    ) -> ContextObject {
        let mut values = object_to_liquid_with(&self.values, definition, field_config, options);
        // Reserved/special
        if values.contains_key("path") {
            panic!("Objects may not define path key.");
        }
        if values.contains_key("order") {
            panic!("Objects may not define order key.");
        }
        values.insert(KString::from_ref("path"), self.url_path().to_value().into());
        values.insert(KString::from_ref("order"), self.order.to_value().into());
        values
    }
}

#[cfg(test)]
mod tests {

    use crate::{
        fields::DateTime, object_definition::tests::artist_and_example_definition_str, FieldConfig,
    };

    use super::*;

    fn artist_object_str() -> &'static str {
        "name = \"Tormenta Rey\"
        order = 1
        [media]
        type = \"video\"
        value = {sha = \"fake-sha\", name = \"Video Name\", filename = \"video.mp4\", mime = \"video/mpeg4\"}

      [[tour_dates]]
      date = \"12/22/2022\"
      ticket_link = \"foo.com\"

      [[videos]]
      video = {sha = \"fake-sha\", name = \"Video Name\", filename = \"video.mp4\", mime = \"video/mpeg4\"}

      [[numbers]]
      number = 2.57"
    }

    #[test]
    fn object_parsing() {
        let defs =
            ObjectDefinition::from_source(artist_and_example_definition_str(), &OrderMap::new())
                .unwrap();
        let table: Table = toml::from_str(artist_object_str()).unwrap();
        let obj = Object::from_table(
            defs.get("artists").unwrap(),
            Path::new("tormenta-rey"),
            &table,
            &OrderMap::new(),
            false,
        )
        .unwrap();
        assert_eq!(obj.order, Some(1.));
        assert_eq!(obj.object_name, "artists");
        assert_eq!(obj.filename, "tormenta-rey");
        assert_eq!(obj.values.len(), 5);
        assert!(obj.values.contains_key("name"));
        assert!(obj.values.contains_key("tour_dates"));
        assert!(obj.values.contains_key("numbers"));
        assert!(obj.values.contains_key("videos"));
        assert!(obj.values.contains_key("media"));
        assert_eq!(
            obj.values.get("name"),
            Some(&FieldValue::String("Tormenta Rey".to_string()))
        );
        let tour_dates = obj.values.get("tour_dates").unwrap();
        assert!(matches!(tour_dates, FieldValue::Objects { .. }));
        if let FieldValue::Objects(tour_dates) = tour_dates {
            assert_eq!(tour_dates.len(), 1);
            let date = tour_dates.first().unwrap();
            assert!(date.contains_key("date"));
            assert!(date.contains_key("ticket_link"));
            assert_eq!(
                date.get("date").unwrap().liquid_date(),
                DateTime::from("12/22/2022 00:00:00")
                    .unwrap()
                    .as_liquid_datetime()
            );
            assert_eq!(
                date.get("ticket_link").unwrap(),
                &FieldValue::String("foo.com".to_string())
            );
        }
        let numbers = obj.values.get("numbers").unwrap();
        assert!(matches!(numbers, FieldValue::Objects { .. }));
        if let FieldValue::Objects(numbers) = numbers {
            assert_eq!(numbers.len(), 1);
            let num = numbers.first().unwrap();
            assert!(num.contains_key("number"));
            assert_eq!(num.get("number").unwrap(), &FieldValue::Number(2.57));
        }
        let media = obj.values.get("media").unwrap();
        assert!(matches!(media, FieldValue::Oneof(_)));
        if let FieldValue::Objects(numbers) = numbers {
            assert_eq!(numbers.len(), 1);
            let num = numbers.first().unwrap();
            assert!(num.contains_key("number"));
            assert_eq!(num.get("number").unwrap(), &FieldValue::Number(2.57));
        }
        let videos = obj.values.get("videos").unwrap();
        assert!(matches!(videos, FieldValue::Objects { .. }));
        if let FieldValue::Objects(videos) = videos {
            assert_eq!(videos.len(), 1);
            let video = videos.first().unwrap();
            assert!(video.contains_key("video"));
            let vf = video.get("video").unwrap();
            assert!(matches!(vf, FieldValue::File(_)));
            println!("{:?}", vf);
            let fc = FieldConfig {
                uploads_url: "test://foo.com".to_string(),
                upload_prefix: "butt/".to_string(),
            };
            if let FieldValue::File(vf) = vf {
                assert_eq!(vf.sha, "fake-sha");
                assert_eq!(vf.name, Some("Video Name".to_string()));
                assert_eq!(vf.filename, "video.mp4");
                assert_eq!(vf.mime, "video/mpeg4");
                assert_eq!(
                    vf.url(&fc),
                    format!("{}/{}fake-sha/video.mp4", fc.uploads_url, fc.upload_prefix)
                );
            }
        }
    }
}
