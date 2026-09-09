//! The third argument every carrier receives: the site's objects, secrets
//! included, in the same shape `archival types` describes.

use crate::{
    fields::FieldValue,
    file_system::FileSystemAPI,
    object::{to_liquid::ToLiquidOptions, ObjectEntry},
    site::Site,
    Object,
};
use anyhow::Result;
use liquid_core::{
    model::{Scalar, Value},
    ValueView,
};
use serde::Serialize;
use serde_json::{Map, Number};
use std::collections::BTreeSet;
use time::{format_description::well_known::Rfc3339, macros::format_description, UtcOffset};

const CARRIER_OPTIONS: ToLiquidOptions = ToLiquidOptions {
    include_secrets: true,
};

/// One of the site's uploaded files, as `objects.UPLOADS.list()` reports it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct UploadEntry {
    pub sha: String,
    pub filename: String,
}

/// Everything the sidecar needs to answer a carrier request.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CarrierPayload {
    pub objects: Map<String, serde_json::Value>,
    pub uploads: Vec<UploadEntry>,
    pub site_url: String,
    pub uploads_url: String,
    pub upload_prefix: String,
}

pub(crate) fn build_payload<F: FileSystemAPI>(
    site: &Site,
    fs: &F,
    site_url: &str,
) -> Result<CarrierPayload> {
    let mut objects = Map::new();
    let mut uploads = BTreeSet::new();
    for (name, entry) in site.get_objects_sorted(fs, Some(order_of))? {
        let definition = site
            .object_definitions
            .get(&name)
            .unwrap_or_else(|| panic!("missing object definition {}", name));
        let value = match &entry {
            ObjectEntry::List(objects) => Value::array(objects.iter().map(|o| {
                o.liquid_object_with(definition, &site.field_config, CARRIER_OPTIONS)
                    .to_value()
            })),
            ObjectEntry::Object(o) => o
                .liquid_object_with(definition, &site.field_config, CARRIER_OPTIONS)
                .to_value(),
        };
        for object in entry.into_iter() {
            collect_uploads(object, &mut uploads);
        }
        objects.insert(name.to_string(), liquid_to_json(&value));
    }
    Ok(CarrierPayload {
        objects,
        uploads: uploads.into_iter().collect(),
        site_url: site_url.to_string(),
        uploads_url: site.field_config.uploads_url.to_owned(),
        upload_prefix: site.field_config.upload_prefix.to_owned(),
    })
}

fn order_of(a: &Object, b: &Object) -> std::cmp::Ordering {
    a.order
        .partial_cmp(&b.order)
        .unwrap_or(std::cmp::Ordering::Equal)
}

/// Walks an object for uploaded files. `UPLOADS.list()` cannot be derived from
/// the serialized objects: a file and a `meta` table holding `sha`/`filename`
/// keys are indistinguishable once both are plain JSON.
fn collect_uploads(object: &Object, into: &mut BTreeSet<UploadEntry>) {
    for value in object.values.values() {
        collect_uploads_in_value(value, into);
    }
}

fn collect_uploads_in_value(value: &FieldValue, into: &mut BTreeSet<UploadEntry>) {
    match value {
        FieldValue::File(file) => {
            if file.is_valid() && !file.filename.is_empty() {
                into.insert(UploadEntry {
                    sha: file.sha.to_owned(),
                    filename: file.filename.to_owned(),
                });
            }
        }
        FieldValue::Objects(children) => {
            for child in children {
                for value in child.values() {
                    collect_uploads_in_value(value, into);
                }
            }
        }
        FieldValue::Oneof((_, inner)) => {
            if let Some(inner) = inner.as_ref() {
                collect_uploads_in_value(inner, into);
            }
        }
        _ => {}
    }
}

/// Liquid values derive `Serialize`, but a `DateTime` serializes through
/// liquid's own display format ("2022-12-22 00:00:00 +0000"). Carriers are
/// typed against ISO 8601 dates, so the conversion is written out by hand.
pub(crate) fn liquid_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Nil | Value::State(_) => serde_json::Value::Null,
        Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(liquid_to_json).collect())
        }
        Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .map(|(k, v)| (k.to_string(), liquid_to_json(v)))
                .collect(),
        ),
        Value::Scalar(scalar) => scalar_to_json(scalar),
    }
}

fn scalar_to_json(scalar: &Scalar) -> serde_json::Value {
    // Discriminating by type_name is deliberate: to_integer and to_date_time
    // parse string values, so a "42" field would silently become a number.
    match scalar.type_name() {
        "whole number" => scalar
            .to_integer()
            .map(|i| serde_json::Value::Number(i.into()))
            .unwrap_or(serde_json::Value::Null),
        "fractional number" => scalar
            .to_float()
            .and_then(Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        "boolean" => scalar
            .to_bool()
            .map(serde_json::Value::Bool)
            .unwrap_or(serde_json::Value::Null),
        "date time" => scalar
            .to_date_time()
            .and_then(|dt| dt.to_offset(UtcOffset::UTC).format(&Rfc3339).ok())
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
        "date" => scalar
            .to_date()
            .and_then(|d| d.format(format_description!("[year]-[month]-[day]")).ok())
            .map(serde_json::Value::String)
            .unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::String(scalar.to_kstr().into_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{file_system::FileSystemAPI, MemoryFileSystem};
    use liquid_core::model::DateTime;
    use std::path::Path;

    fn json(value: Value) -> serde_json::Value {
        liquid_to_json(&value)
    }

    const DEFINITIONS: &str = r#"
        [settings]
        api_key = "secret"
        menu = "upload"

        [artist]
        name = "string"
        [artist.tracks]
        token = "secret"
        audio = "audio"
        "#;

    fn site() -> (Site, MemoryFileSystem) {
        let mut fs = MemoryFileSystem::default();
        fs.write_str(Path::new("archival_objects.toml"), DEFINITIONS.to_string())
            .unwrap();
        fs.write_str(
            Path::new("objects/settings.toml"),
            format!(
                r#"
                api_key = "hunter2"
                [menu]
                sha = "{sha}"
                filename = "menu.pdf"
                mime = "application/pdf"
                "#,
                sha = "a".repeat(64)
            ),
        )
        .unwrap();
        fs.write_str(
            Path::new("objects/artist/tormenta-rey.toml"),
            format!(
                r#"
                name = "Tormenta Rey"
                [[tracks]]
                token = "child-secret"
                [tracks.audio]
                sha = "{sha}"
                filename = "one.mp3"
                mime = "audio/mpeg"
                "#,
                sha = "b".repeat(64)
            ),
        )
        .unwrap();
        let site = Site::load(&fs, Some("prefix/")).unwrap();
        (site, fs)
    }

    fn payload() -> CarrierPayload {
        let (site, fs) = site();
        build_payload(&site, &fs, "http://localhost:3000").unwrap()
    }

    #[test]
    fn secrets_reach_carriers() {
        let payload = payload();
        assert_eq!(payload.objects["settings"]["api_key"], "hunter2");
        assert_eq!(
            payload.objects["artist"][0]["tracks"][0]["token"],
            "child-secret"
        );
    }

    #[test]
    fn root_objects_are_values_and_lists_are_arrays() {
        let payload = payload();
        assert!(payload.objects["settings"].is_object());
        assert!(payload.objects["artist"].is_array());
        // Every object read from its own file carries path and order.
        assert_eq!(payload.objects["artist"][0]["path"], "artist/tormenta-rey");
        assert!(payload.objects["artist"][0].get("order").is_some());
    }

    #[test]
    fn uploads_are_collected_from_every_depth() {
        let payload = payload();
        assert_eq!(
            payload.uploads,
            vec![
                UploadEntry {
                    sha: "a".repeat(64),
                    filename: "menu.pdf".to_string(),
                },
                UploadEntry {
                    sha: "b".repeat(64),
                    filename: "one.mp3".to_string(),
                },
            ]
        );
    }

    #[test]
    fn uploads_config_comes_from_the_site() {
        let payload = payload();
        assert_eq!(payload.upload_prefix, "prefix/");
        assert_eq!(payload.site_url, "http://localhost:3000");
    }

    #[test]
    fn numbers_keep_their_type() {
        assert_eq!(json(Value::scalar(42i64)), serde_json::json!(42));
        assert_eq!(json(Value::scalar(1.5f64)), serde_json::json!(1.5));
    }

    #[test]
    fn a_numeric_string_stays_a_string() {
        // to_integer() would parse this into 42.
        assert_eq!(json(Value::scalar("42")), serde_json::json!("42"));
    }

    #[test]
    fn a_date_looking_string_stays_a_string() {
        assert_eq!(
            json(Value::scalar("2022-12-22")),
            serde_json::json!("2022-12-22")
        );
    }

    #[test]
    fn booleans_and_nil_map_directly() {
        assert_eq!(json(Value::scalar(true)), serde_json::json!(true));
        assert_eq!(json(Value::Nil), serde_json::Value::Null);
    }

    #[test]
    fn datetimes_are_iso_8601() {
        let dt = DateTime::from_ymd(2022, 12, 22);
        assert_eq!(
            json(Value::scalar(dt)),
            serde_json::json!("2022-12-22T00:00:00Z")
        );
    }

    #[test]
    fn nested_values_recurse() {
        let value = liquid::object!({
            "name": "Tormenta Rey",
            "keys": vec![liquid::object!({ "child": 1i64 })],
            "missing": Value::Nil,
        });
        assert_eq!(
            json(Value::Object(value)),
            serde_json::json!({
                "name": "Tormenta Rey",
                "keys": [{ "child": 1 }],
                "missing": null,
            })
        );
    }
}
