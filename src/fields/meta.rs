use crate::liquid_kstring::KStringCow;
use liquid_core::model;
use ordermap::OrderMap;
use serde::{Deserialize, Serialize};
use std::{fmt::Display, hash::Hash};

use crate::util::integer_decode;

use super::DateTime;

#[cfg(feature = "typescript")]
mod typedefs {
    use typescript_type_def::{
        type_expr::{Ident, NativeTypeInfo, TypeExpr, TypeInfo, TypeName},
        TypeDef,
    };

    use crate::fields::MetaValue;

    // These two types are workarounds for the fact that there's a circular type
    // in MetaValue. Below, we use MetaMapTypeDef to then define a meta type
    // that will result in a dependency (which will make sure that types that
    // have MetaValues will also define MetaValue itself, which will not happen
    // with the following two types). The underlying issue is tracked here:
    // https://github.com/dbeckwith/rust-typescript-type-def/issues/18#issuecomment-2078469020
    pub struct MetaArrTypeDef;
    impl TypeDef for MetaArrTypeDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            r#ref: TypeExpr::ident(Ident("MetaValue[]")),
        });
    }
    pub struct MetaTypeDef;
    impl TypeDef for MetaTypeDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            r#ref: TypeExpr::ident(Ident("Record<string, MetaValue>")),
        });
    }

    pub struct MetaMapTypeDef;
    impl TypeDef for MetaMapTypeDef {
        const INFO: TypeInfo = TypeInfo::Native(NativeTypeInfo {
            r#ref: TypeExpr::Name(TypeName {
                path: &[],
                name: Ident("Record"),
                generic_args: &[
                    TypeExpr::Ref(&String::INFO),
                    TypeExpr::Ref(&MetaValue::INFO),
                ],
            }),
        });
    }
}

pub type MetaMap = OrderMap<String, MetaValue>;

#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Hash)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub struct Meta(
    #[cfg_attr(feature = "typescript", type_def(type_of = "typedefs::MetaMapTypeDef"))] pub MetaMap,
);

impl Display for Meta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.to_toml())
    }
}

impl Meta {
    pub fn to_toml(&self) -> toml::map::Map<std::string::String, toml::Value> {
        let mut m = toml::map::Map::new();
        for (k, v) in &self.0 {
            m.insert(k.to_string(), v.to_toml());
        }
        m
    }
    pub fn get_value(&self, key: &str) -> Option<&MetaValue> {
        self.0.get(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_liquid(&self) -> liquid::model::Value {
        let mut m = liquid::Object::new();
        for (k, v) in &self.0 {
            m.insert(k.into(), v.into());
        }
        m.into()
    }
}

impl From<&serde_json::Value> for MetaValue {
    fn from(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Bool(b) => MetaValue::Boolean(*b),
            serde_json::Value::Number(n) => MetaValue::Number(n.as_f64().unwrap()),
            serde_json::Value::String(s) => MetaValue::String(s.into()),
            serde_json::Value::Array(a) => MetaValue::Array(a.iter().map(|i| i.into()).collect()),
            serde_json::Value::Object(o) => MetaValue::Map(o.into()),
            serde_json::Value::Null => {
                todo!("null values unsupported when converting from json to MetaValue")
            }
        }
    }
}

impl From<&serde_json::Map<String, serde_json::Value>> for Meta {
    fn from(value: &serde_json::Map<String, serde_json::Value>) -> Self {
        let mut meta = Self::default();
        for (k, v) in value {
            meta.0.insert(k.to_string(), v.into());
        }
        meta
    }
}

impl From<&MetaValue> for model::Value {
    fn from(value: &MetaValue) -> Self {
        match value {
            MetaValue::String(s) => model::Value::scalar(s.to_string()),
            MetaValue::Number(v) => model::Value::scalar(*v),
            MetaValue::Boolean(v) => model::Value::scalar(*v),
            MetaValue::DateTime(d) => model::Value::scalar(d.as_liquid_datetime()),
            MetaValue::Array(v) => model::Value::array(
                v.iter()
                    .map(model::Value::from)
                    .collect::<Vec<model::Value>>(),
            ),
            MetaValue::Map(m) => model::Value::Object(model::Object::from(m)),
        }
    }
}
impl From<&Meta> for model::Object {
    fn from(value: &Meta) -> Self {
        let mut m = model::Object::new();
        for (k, v) in &value.0 {
            m.insert(k.into(), v.into());
        }
        m
    }
}

impl From<&Meta> for serde_json::Value {
    fn from(value: &Meta) -> Self {
        let mut m = serde_json::Map::<String, serde_json::Value>::new();
        for (k, v) in &value.0 {
            m.insert(k.to_string(), v.into());
        }
        serde_json::Value::Object(m)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[cfg_attr(feature = "typescript", derive(typescript_type_def::TypeDef))]
pub enum MetaValue {
    String(String),
    Number(f64),
    Boolean(bool),
    DateTime(DateTime),
    Array(
        #[cfg_attr(feature = "typescript", type_def(type_of = "typedefs::MetaArrTypeDef"))]
        Vec<MetaValue>,
    ),
    Map(#[cfg_attr(feature = "typescript", type_def(type_of = "typedefs::MetaTypeDef"))] Meta),
}

impl Hash for MetaValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            MetaValue::Number(n) => integer_decode(*n).hash(state),
            v => v.hash(state),
        }
    }
}

impl MetaValue {
    pub fn to_toml(&self) -> toml::Value {
        match self {
            Self::String(s) => toml::Value::String(s.to_string()),
            Self::Number(v) => toml::Value::Float(*v),
            Self::Boolean(v) => toml::Value::Boolean(*v),
            Self::DateTime(d) => {
                let dt = *d.as_liquid_datetime();
                let date = dt.date();
                let time = dt.time();
                let offset = dt.offset();
                toml::Value::Datetime(toml_datetime::Datetime {
                    date: Some(toml_datetime::Date {
                        year: date.year() as u16,
                        month: date.month() as u8,
                        day: date.day(),
                    }),
                    time: Some(toml_datetime::Time {
                        hour: time.hour(),
                        minute: time.minute(),
                        second: Some(time.second()),
                        nanosecond: Some(time.nanosecond()),
                    }),
                    offset: Some(toml_datetime::Offset::Custom {
                        minutes: offset.whole_minutes(),
                    }),
                })
            }
            Self::Array(v) => toml::Value::Array(v.iter().map(|n| n.to_toml()).collect()),
            Self::Map(m) => toml::Value::Table(m.to_toml()),
        }
    }
}

// JSON

impl From<&MetaValue> for serde_json::Value {
    fn from(value: &MetaValue) -> Self {
        match value {
            MetaValue::String(s) => s.to_string().into(),
            MetaValue::Number(v) => (*v).into(),
            MetaValue::Boolean(v) => (*v).into(),
            MetaValue::DateTime(d) => d.to_string().into(),
            MetaValue::Array(v) => v.iter().collect::<serde_json::Value>(),
            MetaValue::Map(m) => m.into(),
        }
    }
}

// TOML

impl From<&toml::Value> for MetaValue {
    fn from(value: &toml::Value) -> Self {
        match value {
            toml::Value::String(v) => MetaValue::String(v.to_string()),
            toml::Value::Integer(v) => MetaValue::Number(*v as f64),
            toml::Value::Float(v) => MetaValue::Number(*v),
            toml::Value::Array(v) => MetaValue::Array(v.iter().map(|n| n.into()).collect()),
            toml::Value::Boolean(v) => MetaValue::Boolean(*v),
            toml::Value::Table(v) => MetaValue::Map(v.into()),
            toml::Value::Datetime(v) => MetaValue::DateTime(DateTime::from_toml(v).unwrap()),
        }
    }
}

impl From<&toml::map::Map<String, toml::Value>> for Meta {
    fn from(value: &toml::map::Map<String, toml::Value>) -> Self {
        let mut meta = OrderMap::new();
        for (k, v) in value {
            meta.insert(k.to_string(), MetaValue::from(v));
        }
        Self(meta)
    }
}

// Liquid

impl model::ValueView for Meta {
    fn as_scalar(&self) -> Option<model::ScalarCow<'_>> {
        None
    }

    fn is_scalar(&self) -> bool {
        false
    }

    fn as_array(&self) -> Option<&dyn model::ArrayView> {
        None
    }

    fn is_array(&self) -> bool {
        false
    }

    fn as_object(&self) -> Option<&dyn model::ObjectView> {
        Some(self)
    }

    fn is_object(&self) -> bool {
        true
    }

    fn as_state(&self) -> Option<model::State> {
        None
    }

    fn is_state(&self) -> bool {
        false
    }

    fn is_nil(&self) -> bool {
        false
    }

    fn as_debug(&self) -> &dyn std::fmt::Debug {
        &self.0
    }

    fn render(&self) -> model::DisplayCow<'_> {
        todo!()
    }

    fn source(&self) -> model::DisplayCow<'_> {
        todo!()
    }

    fn type_name(&self) -> &'static str {
        "meta"
    }

    fn query_state(&self, _state: model::State) -> bool {
        false
    }

    fn to_kstr(&self) -> KStringCow<'_> {
        format!("{:?}", self.0).into()
    }

    fn to_value(&self) -> model::Value {
        let mut m = model::Object::new();
        for (k, v) in &self.0 {
            m.insert(k.into(), v.into());
        }
        model::Value::Object(m)
    }
}

impl model::ObjectView for Meta {
    fn as_value(&self) -> &dyn model::ValueView {
        self
    }

    fn size(&self) -> i64 {
        self.0.len() as i64
    }

    fn keys<'k>(&'k self) -> Box<dyn Iterator<Item = KStringCow<'k>> + 'k> {
        Box::new(self.0.keys().map(|k| k.into()))
    }

    fn values<'k>(&'k self) -> Box<dyn Iterator<Item = &'k dyn model::ValueView> + 'k> {
        Box::new(self.keys().map(|k| self.get(&k).unwrap()))
    }

    fn iter<'k>(
        &'k self,
    ) -> Box<dyn Iterator<Item = (KStringCow<'k>, &'k dyn model::ValueView)> + 'k> {
        todo!()
    }

    fn contains_key(&self, index: &str) -> bool {
        self.0.contains_key(index)
    }

    fn get<'s>(&'s self, index: &str) -> Option<&'s dyn model::ValueView> {
        if let Some(o) = self.0.get(index) {
            Some(o)
        } else {
            None
        }
    }
}

impl model::ArrayView for MetaValue {
    fn first(&self) -> Option<&dyn model::ValueView> {
        self.get(0)
    }

    fn last(&self) -> Option<&dyn model::ValueView> {
        self.get(-1)
    }

    fn as_value(&self) -> &dyn model::ValueView {
        self
    }

    fn size(&self) -> i64 {
        if let MetaValue::Array(arr) = self {
            arr.len() as i64
        } else {
            0
        }
    }

    fn values<'k>(&'k self) -> Box<dyn Iterator<Item = &'k dyn model::ValueView> + 'k> {
        if let MetaValue::Array(arr) = self {
            Box::new(arr.values())
        } else {
            panic!("values called on non-array MetaValue")
        }
    }

    fn contains_key(&self, index: i64) -> bool {
        if let MetaValue::Array(arr) = self {
            arr.contains_key(index)
        } else {
            false
        }
    }

    fn get(&self, index: i64) -> Option<&dyn model::ValueView> {
        if let MetaValue::Array(arr) = self {
            arr.get(index)
        } else {
            None
        }
    }
}

impl model::ValueView for MetaValue {
    fn as_scalar(&self) -> Option<model::ScalarCow<'_>> {
        match self {
            MetaValue::String(s) => Some(s.to_string().into()),
            MetaValue::Number(v) => Some((*v).into()),
            MetaValue::Boolean(v) => Some((*v).into()),
            MetaValue::DateTime(d) => Some(d.as_liquid_datetime().into()),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&dyn model::ArrayView> {
        if let MetaValue::Array(v) = self {
            Some(v)
        } else {
            None
        }
    }
    fn as_object(&self) -> Option<&dyn model::ObjectView> {
        if let MetaValue::Map(m) = self {
            Some(m)
        } else {
            None
        }
    }

    fn as_debug(&self) -> &dyn std::fmt::Debug {
        self
    }

    fn render(&self) -> model::DisplayCow<'_> {
        match self {
            MetaValue::String(s) => s.render(),
            MetaValue::Number(v) => v.render(),
            MetaValue::Boolean(v) => v.render(),
            MetaValue::DateTime(d) => d.borrowed_as_datetime().render(),
            _ => todo!("MetaValue render not implemented for non-scalar values"),
        }
    }

    fn source(&self) -> model::DisplayCow<'_> {
        todo!("MetaValue source not implemented")
    }

    fn type_name(&self) -> &'static str {
        match self {
            MetaValue::String(_) => "meta:string",
            MetaValue::Number(_) => "meta:number",
            MetaValue::Boolean(_) => "meta:boolean",
            MetaValue::DateTime(_) => "meta:datetime",
            MetaValue::Array(_) => "meta:array",
            MetaValue::Map(_) => "meta:map",
        }
    }

    fn query_state(&self, _state: model::State) -> bool {
        false
    }

    fn to_kstr(&self) -> KStringCow<'_> {
        format!("{:?}", self).into()
    }

    fn to_value(&self) -> model::Value {
        self.into()
    }
}
