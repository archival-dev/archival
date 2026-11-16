//! A payment account's catalog, handed to a build as a snapshot and rendered
//! under `archival.payments`.
//!
//! The file is the one `archival run` reads from `ARCHIVAL_PAYMENTS`: an
//! `account`, and a `catalog` whose `items` are products with their variants,
//! as `site.payments` answers them. `archival.payments` answers what
//! `site.payments` does without `stock` or `available`, which change with every
//! sale, so a page reads them from a carrier when it is viewed.
//!
//! Without a snapshot, or with one whose `account` is null, `archival.payments`
//! reads as a site with no payment account connected.

use crate::archival_global::{liquid_value, ArchivalView};
use liquid_core::Value;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// The key of the `archival` global this snapshot fills.
pub(crate) const SECTION: &str = "payments";

/// The account a site sells from. What a snapshot's file leaves out of it is
/// filled in as `archival run` fills it in, so a local file with no `account`
/// still reads as connected; only `"account": null` reads as not connected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentAccount {
    pub provider: String,
    pub name: Option<String>,
    pub livemode: bool,
    pub currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentProduct {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub images: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub variants: Vec<PaymentVariant>,
}

/// A variant as a build sees it. A file may hold `stock` and `available`, as a
/// carrier's answer does; they are not read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaymentVariant {
    pub id: String,
    /// The product it is a variant of, which is always the one that lists it.
    #[serde(default)]
    pub product: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub sku: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub amount: Option<u64>,
    pub currency: String,
    #[serde(default)]
    pub recurring: Option<Recurring>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recurring {
    pub interval: String,
}

/// Present, which tells a null apart from a missing key.
fn present<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(deserializer).map(Some)
}

/// Present, and possibly null.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(deserializer)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PaymentsSnapshot {
    /// `None` when no account is connected, which sells nothing.
    pub account: Option<PaymentAccount>,
    /// Every product, in id order, which is the order `catalog()` pages them in.
    pub products: Vec<PaymentProduct>,
}

#[derive(Deserialize)]
struct PaymentsFile {
    #[serde(default, deserialize_with = "present")]
    account: Option<serde_json::Value>,
    #[serde(default)]
    catalog: Option<CatalogFile>,
}

#[derive(Deserialize)]
struct CatalogFile {
    #[serde(default)]
    items: Vec<PaymentProduct>,
}

impl PaymentsSnapshot {
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        let file: PaymentsFile = serde_json::from_str(json)?;
        let mut products = file.catalog.map(|c| c.items).unwrap_or_default();
        let given = match file.account {
            Some(serde_json::Value::Null) => {
                return Ok(Self {
                    account: None,
                    products: vec![],
                })
            }
            Some(serde_json::Value::Object(given)) => given,
            Some(_) => {
                return Err(serde::de::Error::custom(
                    "account must be an object, or null for no account",
                ))
            }
            None => serde_json::Map::new(),
        };
        products.sort_by(|a, b| a.id.cmp(&b.id));
        for product in &mut products {
            for variant in &mut product.variants {
                variant.product = product.id.clone();
            }
        }
        let currency = products
            .iter()
            .flat_map(|product| &product.variants)
            .map(|variant| variant.currency.clone())
            .next()
            .unwrap_or_else(|| "usd".to_string());
        let mut account = serde_json::json!({
            "provider": "stripe",
            "name": "Local payments",
            "livemode": false,
            "currency": currency,
        });
        if let Some(defaults) = account.as_object_mut() {
            defaults.extend(given);
        }
        Ok(Self {
            account: Some(serde_json::from_value(account)?),
            products,
        })
    }
}

/// The snapshot's part of the `archival` global: `account`, `catalog` and
/// lookups by id under `products` and `variants`, where an id the account does
/// not sell reads as nil, as `products(ids)` and `variants(ids)` answer it.
pub(crate) fn view(snapshot: Option<&PaymentsSnapshot>) -> ArchivalView {
    let (account, products) = match snapshot {
        Some(PaymentsSnapshot {
            account: Some(account),
            products,
        }) => (liquid_value(account), &products[..]),
        _ => (Value::Nil, &[][..]),
    };
    let mut by_product = liquid::Object::new();
    let mut by_variant = liquid::Object::new();
    for product in products {
        by_product.insert(product.id.clone().into(), liquid_value(product));
        for variant in &product.variants {
            by_variant.insert(variant.id.clone().into(), liquid_value(variant));
        }
    }
    let mut catalog = liquid::Object::new();
    catalog.insert("items".into(), liquid_value(&products));
    catalog.insert("total".into(), Value::scalar(products.len() as i64));
    catalog.insert("next".into(), Value::Nil);
    let mut fields = liquid::Object::new();
    fields.insert("account".into(), account);
    fields.insert("catalog".into(), Value::Object(catalog));
    ArchivalView {
        children: vec![
            ("products", ArchivalView::keyed(by_product, Value::Nil)),
            ("variants", ArchivalView::keyed(by_variant, Value::Nil)),
        ],
        fields,
        fallback: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        constants::{MANIFEST_FILE_NAME, OBJECT_DEFINITION_FILE_NAME},
        site::Site,
        BuildOptions, FileSystemAPI, MemoryFileSystem,
    };
    use liquid::ValueView;
    use std::path::{Path, PathBuf};

    const FULL: &str = include_str!("../tests/schemas/positive/archival_payments/full.json");

    fn render(template: &str, payments: Option<PaymentsSnapshot>) -> String {
        let mut fs = MemoryFileSystem::default();
        fs.write_str(Path::new(MANIFEST_FILE_NAME), String::new())
            .unwrap();
        fs.write_str(
            Path::new(OBJECT_DEFINITION_FILE_NAME),
            "[print]\nname = \"string\"\nvariant_id = \"string\"\ntemplate = \"print\"\n"
                .to_string(),
        )
        .unwrap();
        fs.write_str(
            Path::new("objects/print/harbour.toml"),
            "name = \"Harbour\"\nvariant_id = \"price_harbour_a3\"\n".to_string(),
        )
        .unwrap();
        fs.write_str(Path::new("pages/print.liquid"), template.to_string())
            .unwrap();
        let site = Site::load(&fs, Some("test")).unwrap();
        site.set_payments(payments);
        site.build(&mut fs, BuildOptions::default()).unwrap();
        fs.read_to_string(site.manifest.build_dir.join("print/harbour.html"))
            .unwrap()
            .unwrap()
            .trim()
            .to_string()
    }

    fn full() -> Option<PaymentsSnapshot> {
        Some(PaymentsSnapshot::from_json(FULL).unwrap())
    }

    #[test]
    fn the_catalog_renders_as_site_payments_answers_it() {
        let rendered = render(
            "{{ archival.payments.account.name }}|\
             {{ archival.payments.account.provider }}|\
             {{ archival.payments.catalog.total }}|\
             {% for product in archival.payments.catalog.items %}{{ product.id }};{% endfor %}|\
             {{ archival.payments.catalog.next | default: \"end\" }}|\
             {% assign variant = archival.payments.variants[print.variant_id] %}\
             {{ variant.amount }} {{ variant.currency }} {{ variant.product }}|\
             {{ archival.payments.products[variant.product].name }}|\
             {{ archival.payments.variants[\"price_membership\"].recurring.interval }}",
            full(),
        );
        assert_eq!(
            rendered,
            "Harbour Prints|stripe|2|prod_harbour;prod_membership;|end|\
             6000 usd prod_harbour|Harbour, 6am|month"
        );
    }

    #[test]
    fn stock_is_left_to_carriers() {
        let Value::Object(payments) = view(full().as_ref()).to_value() else {
            panic!("payments is an object");
        };
        let lookup = |key: &str| match payments.get(key) {
            Some(Value::Object(entries)) => entries.clone(),
            _ => panic!("{key} is an object"),
        };
        let mut variants: Vec<Value> = lookup("variants").into_iter().map(|(_, v)| v).collect();
        for (_, product) in lookup("products") {
            if let Some(Value::Array(listed)) = product
                .as_object()
                .and_then(|p| p.get("variants"))
                .map(|v| v.to_value())
            {
                variants.extend(listed);
            }
        }
        assert_eq!(variants.len(), 6);
        for variant in variants {
            let variant = variant.as_object().unwrap();
            assert!(variant.contains_key("amount"));
            assert!(!variant.contains_key("stock") && !variant.contains_key("available"));
        }
    }

    #[test]
    fn no_snapshot_reads_as_no_account() {
        let rendered = render(
            "{{ archival.payments.account | default: \"none\" }}|\
             {{ archival.payments.catalog.total }}|\
             {{ archival.payments.catalog.items.size }}|\
             {% if archival.payments.variants[print.variant_id] %}found{% else %}missing{% endif %}",
            None,
        );
        assert_eq!(rendered, "none|0|0|missing");
        assert_eq!(
            render(
                "{% if archival.payments.products[\"prod_gone\"] %}found{% else %}missing{% endif %}",
                full()
            ),
            "missing"
        );
    }

    #[test]
    fn a_file_fills_in_what_archival_run_fills_in() {
        let snapshot = PaymentsSnapshot::from_json(
            r#"{"catalog": {"items": [{"id": "b", "name": "B", "variants": [
                {"id": "b1", "amount": 500, "currency": "eur", "stock": 2, "available": false}
            ]}, {"id": "a", "name": "A", "variants": []}]}}"#,
        )
        .unwrap();
        assert_eq!(
            snapshot.account,
            Some(PaymentAccount {
                provider: "stripe".to_string(),
                name: Some("Local payments".to_string()),
                livemode: false,
                currency: Some("eur".to_string()),
            })
        );
        assert_eq!(
            snapshot
                .products
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        let variant = &snapshot.products[1].variants[0];
        assert_eq!(variant.product, "b");
        assert_eq!(
            (variant.name.clone(), variant.recurring.clone()),
            (None, None)
        );
        let empty = PaymentsSnapshot::from_json("{}").unwrap();
        assert_eq!(
            empty
                .account
                .and_then(|account| account.currency)
                .as_deref(),
            Some("usd")
        );
        assert!(empty.products.is_empty());
    }

    #[test]
    fn a_null_account_reads_as_no_account_and_sells_nothing() {
        let disconnected = PaymentsSnapshot::from_json(
            r#"{"account": null, "catalog": {"items": [{"id": "a", "name": "A", "variants": [
                {"id": "a1", "amount": 500, "currency": "usd"}
            ]}]}}"#,
        )
        .unwrap();
        assert_eq!(
            disconnected,
            PaymentsSnapshot {
                account: None,
                products: vec![]
            }
        );
        let template = "{{ archival.payments.account | default: \"none\" }}|\
             {{ archival.payments.catalog.total }}";
        assert_eq!(render(template, Some(disconnected)), "none|0");
        assert_eq!(render(template, None), "none|0");
        assert!(PaymentsSnapshot::from_json(r#"{"account": "stripe"}"#).is_err());
    }

    #[test]
    fn the_snapshot_parser_agrees_with_the_schema_fixtures() {
        let fixtures = |kind: &str| -> Vec<PathBuf> {
            let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/schemas")
                .join(kind)
                .join("archival_payments");
            std::fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect()
        };
        for path in fixtures("positive") {
            let json = std::fs::read_to_string(&path).unwrap();
            if let Err(e) = PaymentsSnapshot::from_json(&json) {
                panic!("{} should parse: {e}", path.display());
            }
        }
        for path in fixtures("negative") {
            let json = std::fs::read_to_string(&path).unwrap();
            assert!(
                PaymentsSnapshot::from_json(&json).is_err(),
                "{} should not parse",
                path.display()
            );
        }
    }
}
