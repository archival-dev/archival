use crate::liquid_kstring::KString;
use crate::{
    fields::{File, ObjectValues},
    FieldConfig, FieldValue, ObjectDefinition,
};
use liquid::model::ObjectIndex;
use liquid_core::{Value, ValueView};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToLiquidOptions {
    pub include_secrets: bool,
}

pub fn object_to_liquid_with(
    object_values: &ObjectValues,
    definition: &ObjectDefinition,
    field_config: &FieldConfig,
    options: ToLiquidOptions,
) -> liquid::model::Object {
    let mut values: Vec<(KString, Value)> = definition
        .fields
        .iter()
        // Secret fields are never added to template contexts.
        .filter(|(_, field)| options.include_secrets || !field.r#type.is_secret())
        .map(|(k, _)| {
            (
                KString::from_ref(k.as_index()),
                object_values
                    .get(k)
                    .map(|v| v.to_liquid_with(field_config, options))
                    .unwrap_or_else(|| Value::Nil),
            )
        })
        .collect();
    let mut child_values: Vec<(KString, Value)> = definition
        .children
        .iter()
        .map(|(k, child_def)| {
            (
                KString::from_ref(k.as_index()),
                object_values
                    .get(k)
                    .map(|v| v.typed_objects_with(child_def, field_config, options))
                    .unwrap_or_else(|| Value::Array(vec![])),
            )
        })
        .collect();
    let mut meta_values: Vec<(KString, Value)> = object_values
        .iter()
        .filter_map(|(k, v)| {
            if let FieldValue::Meta(meta) = v {
                Some((KString::from_ref(k.as_index()), meta.to_liquid()))
            } else {
                None
            }
        })
        .collect();
    values.append(&mut meta_values);
    values.append(&mut child_values);
    values.into_iter().collect()
}

impl FieldValue {
    pub fn to_liquid(&self, field_config: &FieldConfig) -> liquid::model::Value {
        self.to_liquid_with(field_config, ToLiquidOptions::default())
    }

    pub fn to_liquid_with(
        &self,
        field_config: &FieldConfig,
        options: ToLiquidOptions,
    ) -> liquid::model::Value {
        match self {
            // Belt and braces: secret-typed fields are dropped from contexts
            // above, but a secret value can never render regardless.
            FieldValue::Secret(s) => {
                if options.include_secrets {
                    liquid::model::Value::scalar(s.to_owned())
                } else {
                    liquid::model::Value::Nil
                }
            }
            FieldValue::File(file) => file.to_liquid(field_config),
            FieldValue::Oneof((t, v)) => match v.as_ref() {
                Some(v) => liquid::object!({
                    "type": t,
                    "value": v.to_liquid_with(field_config, options)
                })
                .into(),
                None => liquid::model::Value::Nil,
            },
            _ => self.to_value(),
        }
    }
}

impl File {
    pub fn to_liquid(&self, field_config: &FieldConfig) -> liquid::model::Value {
        let mut m = liquid::model::Object::new();
        for (k, v) in self.clone().into_map(Some(field_config)) {
            m.insert(k.into(), liquid::model::Value::scalar(v));
        }
        liquid_core::Value::Object(m)
    }
}

#[cfg(test)]
mod secret_tests {
    use super::*;
    use crate::{fields::FieldType, object::Object};
    use ordermap::OrderMap;
    use std::path::Path;
    use toml::Table;

    fn definitions() -> crate::ObjectDefinitions {
        ObjectDefinition::from_source(
            r#"
            [artists]
            name = "string"
            api_key = "secret"
            [artists.keys]
            child_key = "secret"
            "#,
            &OrderMap::new(),
        )
        .unwrap()
    }

    fn artist() -> (ObjectDefinition, Object) {
        let defs = definitions();
        let definition = defs.get("artists").unwrap().clone();
        let table: Table = toml::from_str(
            r#"
            name = "Tormenta Rey"
            api_key = "hunter2"
            [[keys]]
            child_key = "child-secret"
            "#,
        )
        .unwrap();
        let object = Object::from_table(
            &definition,
            Path::new("tormenta-rey"),
            &table,
            &OrderMap::new(),
            false,
        )
        .unwrap();
        (definition, object)
    }

    #[test]
    fn secret_definitions_are_parsed() {
        let defs = definitions();
        let artists = defs.get("artists").unwrap();
        assert_eq!(artists.field_type("api_key"), Some(&FieldType::Secret));
        assert!(FieldType::Secret.is_secret());
        assert!(!FieldType::String.is_secret());
    }

    #[test]
    fn secrets_are_readable_from_rust() {
        let (_, object) = artist();
        assert_eq!(
            object.values.get("api_key"),
            Some(&FieldValue::Secret("hunter2".to_string()))
        );
        assert_eq!(
            object.values.get("api_key").unwrap().as_secret(),
            Some("hunter2")
        );
    }

    #[test]
    fn secrets_round_trip_through_toml() {
        let (definition, object) = artist();
        let written = object.to_toml(&definition).unwrap();
        assert!(written.contains("hunter2"), "{}", written);
        let table: Table = toml::from_str(&written).unwrap();
        let reparsed = Object::from_table(
            &definition,
            Path::new("tormenta-rey"),
            &table,
            &OrderMap::new(),
            false,
        )
        .unwrap();
        assert_eq!(reparsed.values, object.values);
    }

    fn liquid_values(options: ToLiquidOptions) -> liquid::model::Object {
        let (definition, object) = artist();
        object_to_liquid_with(
            &object.values,
            &definition,
            &FieldConfig::default(),
            options,
        )
    }

    #[test]
    fn secrets_are_omitted_from_liquid_objects() {
        let values = liquid_values(ToLiquidOptions::default());
        assert!(values.contains_key("name"));
        assert!(
            !values.contains_key("api_key"),
            "secret fields are not added to contexts"
        );
        // Children are built from their own definitions, so they get the same
        // treatment.
        let children = values.get("keys").unwrap().as_array().unwrap();
        let child = children.values().next().unwrap();
        let child = child.as_object().unwrap();
        assert!(!child.contains_key("child_key"));
    }

    #[test]
    fn secrets_are_included_when_requested() {
        let values = liquid_values(ToLiquidOptions {
            include_secrets: true,
        });
        assert_eq!(values.get("api_key").unwrap().to_kstr(), "hunter2");
        let children = values.get("keys").unwrap().as_array().unwrap();
        let child = children.values().next().unwrap();
        let child = child.as_object().unwrap();
        assert_eq!(child.get("child_key").unwrap().to_kstr(), "child-secret");
    }

    #[test]
    fn secrets_inside_a_oneof_follow_the_same_rule() {
        let secret = FieldValue::Oneof((
            "key".to_string(),
            Box::new(Some(FieldValue::Secret("hunter2".to_string()))),
        ));
        let config = FieldConfig::default();
        let hidden = secret.to_liquid(&config);
        assert!(hidden.as_object().unwrap().get("value").unwrap().is_nil());
        let shown = secret.to_liquid_with(
            &config,
            ToLiquidOptions {
                include_secrets: true,
            },
        );
        assert_eq!(
            shown.as_object().unwrap().get("value").unwrap().to_kstr(),
            "hunter2"
        );
    }

    #[test]
    fn secret_values_never_render() {
        let secret = FieldValue::Secret("hunter2".to_string());
        assert!(secret.to_liquid(&FieldConfig::default()).is_nil());
        assert!(secret.as_scalar().is_none());
        assert!(secret.to_value().is_nil());
        assert_eq!(secret.to_kstr(), "");
        assert_eq!(secret.to_string(), "");
    }
}
