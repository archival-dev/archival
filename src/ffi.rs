//! What archival's types need to cross UniFFI beyond the derives on the event structs,
//! `File`, `DisplayType` and `ManifestField`: the types whose Rust shape the FFI cannot
//! carry, lowered to a string or to a twin built from what every binding can express.
//!
//! A path and a date cross as their string forms. The value types are recursive and keep
//! ordered maps, which UniFFI has no type for, so each lowers to an `Ffi*` twin whose
//! recursion rides `Vec` and `HashMap`: a oneof's value is a list of zero or one, and a
//! meta map is a list of entries in its own order. A binding sees `FieldValue` as a type
//! alias of `FfiFieldValue`.

use std::collections::HashMap;

use crate::{
    fields::{DateTime, File, Meta, MetaValue, ObjectValues},
    FieldValue, ValuePath,
};

uniffi::custom_type!(ValuePath, String, {
    lower: |path| path.to_string(),
    try_lift: |value| Ok(ValuePath::from_string(&value)),
});

uniffi::custom_type!(DateTime, String, {
    lower: |date| date.to_string(),
    try_lift: |value| DateTime::from(&value).map_err(|error| anyhow::anyhow!("{error}")),
});

uniffi::custom_type!(FieldValue, FfiFieldValue, {
    lower: |value| value.into(),
    try_lift: |value| Ok(value.into()),
});

uniffi::custom_type!(MetaValue, FfiMetaValue, {
    lower: |value| value.into(),
    try_lift: |value| Ok(value.into()),
});

uniffi::custom_type!(Meta, Vec<FfiMetaEntry>, {
    lower: |meta| meta_entries(meta),
    try_lift: |entries| Ok(meta_from_entries(entries)),
});

/// `FieldValue` as the FFI carries it.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum FfiFieldValue {
    String(String),
    Secret(String),
    Enum(String),
    Markdown(String),
    Number(f64),
    Date(DateTime),
    Objects(Vec<HashMap<String, FfiFieldValue>>),
    /// `value` holds one element when the variant carries a value and none when it does
    /// not; a second element is ignored.
    Oneof {
        name: String,
        value: Vec<FfiFieldValue>,
    },
    Boolean(bool),
    File(File),
    Meta(Vec<FfiMetaEntry>),
    Null,
}

/// `MetaValue` as the FFI carries it.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum FfiMetaValue {
    String(String),
    Number(f64),
    Boolean(bool),
    DateTime(DateTime),
    Array(Vec<FfiMetaValue>),
    Map(Vec<FfiMetaEntry>),
}

/// One entry of a meta map, in the map's own order.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiMetaEntry {
    pub key: String,
    pub value: FfiMetaValue,
}

fn meta_entries(meta: Meta) -> Vec<FfiMetaEntry> {
    meta.0
        .into_iter()
        .map(|(key, value)| FfiMetaEntry {
            key,
            value: value.into(),
        })
        .collect()
}

fn meta_from_entries(entries: Vec<FfiMetaEntry>) -> Meta {
    let mut meta = Meta::default();
    for entry in entries {
        meta.0.insert(entry.key, entry.value.into());
    }
    meta
}

impl From<FieldValue> for FfiFieldValue {
    fn from(value: FieldValue) -> Self {
        match value {
            FieldValue::String(s) => Self::String(s),
            FieldValue::Secret(s) => Self::Secret(s),
            FieldValue::Enum(s) => Self::Enum(s),
            FieldValue::Markdown(s) => Self::Markdown(s),
            FieldValue::Number(n) => Self::Number(n),
            FieldValue::Date(date) => Self::Date(date),
            FieldValue::Objects(objects) => Self::Objects(
                objects
                    .into_iter()
                    .map(|values| {
                        values
                            .into_iter()
                            .map(|(key, value)| (key, value.into()))
                            .collect()
                    })
                    .collect(),
            ),
            FieldValue::Oneof((name, value)) => Self::Oneof {
                name,
                value: value.map(Self::from).into_iter().collect(),
            },
            FieldValue::Boolean(b) => Self::Boolean(b),
            FieldValue::File(file) => Self::File(file),
            FieldValue::Meta(meta) => Self::Meta(meta_entries(meta)),
            FieldValue::Null => Self::Null,
        }
    }
}

impl From<FfiFieldValue> for FieldValue {
    fn from(value: FfiFieldValue) -> Self {
        match value {
            FfiFieldValue::String(s) => Self::String(s),
            FfiFieldValue::Secret(s) => Self::Secret(s),
            FfiFieldValue::Enum(s) => Self::Enum(s),
            FfiFieldValue::Markdown(s) => Self::Markdown(s),
            FfiFieldValue::Number(n) => Self::Number(n),
            FfiFieldValue::Date(date) => Self::Date(date),
            FfiFieldValue::Objects(objects) => Self::Objects(
                objects
                    .into_iter()
                    .map(|values| {
                        values
                            .into_iter()
                            .map(|(key, value)| (key, value.into()))
                            .collect::<ObjectValues>()
                    })
                    .collect(),
            ),
            FfiFieldValue::Oneof { name, value } => {
                Self::Oneof((name, Box::new(value.into_iter().next().map(Self::from))))
            }
            FfiFieldValue::Boolean(b) => Self::Boolean(b),
            FfiFieldValue::File(file) => Self::File(file),
            FfiFieldValue::Meta(entries) => Self::Meta(meta_from_entries(entries)),
            FfiFieldValue::Null => Self::Null,
        }
    }
}

impl From<MetaValue> for FfiMetaValue {
    fn from(value: MetaValue) -> Self {
        match value {
            MetaValue::String(s) => Self::String(s),
            MetaValue::Number(n) => Self::Number(n),
            MetaValue::Boolean(b) => Self::Boolean(b),
            MetaValue::DateTime(date) => Self::DateTime(date),
            MetaValue::Array(items) => Self::Array(items.into_iter().map(Self::from).collect()),
            MetaValue::Map(meta) => Self::Map(meta_entries(meta)),
        }
    }
}

impl From<FfiMetaValue> for MetaValue {
    fn from(value: FfiMetaValue) -> Self {
        match value {
            FfiMetaValue::String(s) => Self::String(s),
            FfiMetaValue::Number(n) => Self::Number(n),
            FfiMetaValue::Boolean(b) => Self::Boolean(b),
            FfiMetaValue::DateTime(date) => Self::DateTime(date),
            FfiMetaValue::Array(items) => Self::Array(items.into_iter().map(Self::from).collect()),
            FfiMetaValue::Map(entries) => Self::Map(meta_from_entries(entries)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::DisplayType;

    fn sample() -> FieldValue {
        let mut contact = Meta::default();
        contact
            .0
            .insert("zip".into(), MetaValue::String("94110".into()));
        contact.0.insert("alpha".into(), MetaValue::Boolean(true));
        let mut meta = Meta::default();
        meta.0.insert("contact".into(), MetaValue::Map(contact));
        meta.0.insert(
            "tags".into(),
            MetaValue::Array(vec![MetaValue::String("news".into())]),
        );

        let mut child = ObjectValues::new();
        child.insert("title".into(), FieldValue::String("Hello".into()));
        child.insert(
            "hero".into(),
            FieldValue::File(File {
                display_type: DisplayType::Image,
                filename: "hero.png".into(),
                sha: "abc".into(),
                mime: "image/png".into(),
                name: None,
                description: None,
            }),
        );
        child.insert(
            "link".into(),
            FieldValue::Oneof((
                "url".into(),
                Box::new(Some(FieldValue::String("https://x".into()))),
            )),
        );
        child.insert(
            "audio".into(),
            FieldValue::Oneof(("none".into(), Box::new(None))),
        );
        child.insert(
            "when".into(),
            FieldValue::Date(DateTime::from("2026-02-16 10:00:00 +0000").unwrap()),
        );
        child.insert("meta".into(), FieldValue::Meta(meta));

        FieldValue::Objects(vec![child])
    }

    #[test]
    fn a_value_round_trips_through_its_twin() {
        let value = sample();
        let twin = FfiFieldValue::from(value.clone());
        assert_eq!(FieldValue::from(twin), value);
    }

    #[test]
    fn a_oneof_without_a_value_lowers_to_an_empty_list() {
        let twin = FfiFieldValue::from(FieldValue::Oneof(("none".into(), Box::new(None))));
        assert!(
            matches!(twin, FfiFieldValue::Oneof { ref name, ref value } if name == "none" && value.is_empty())
        );
    }

    #[test]
    fn meta_keeps_its_order_through_the_entry_list() {
        let mut meta = Meta::default();
        meta.0.insert("zip".into(), MetaValue::Number(1.0));
        meta.0.insert("alpha".into(), MetaValue::Number(2.0));
        let entries = meta_entries(meta.clone());
        let keys: Vec<&str> = entries.iter().map(|entry| entry.key.as_str()).collect();
        assert_eq!(keys, vec!["zip", "alpha"]);
        assert_eq!(meta_from_entries(entries), meta);
    }

    #[test]
    fn a_path_and_a_date_round_trip_as_strings() {
        let path = ValuePath::from_string("sections.1.links");
        assert_eq!(ValuePath::from_string(&path.to_string()), path);
        let date = DateTime::from("2026-02-16 10:00:00 +0000").unwrap();
        assert_eq!(DateTime::from(&date.to_string()).unwrap(), date);
    }
}
