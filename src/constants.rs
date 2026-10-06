pub const MIN_COMPAT_VERSION: &str = ">=0.12.0";
pub const MANIFEST_FILE_NAME: &str = "archival.toml";
pub const OBJECT_DEFINITION_FILE_NAME: &str = "archival_objects.toml";
/// Sites created before these files were given archival-specific names still
/// use the generic ones. They are read (and written back to) when present, but
/// the names above are canonical for anything archival creates.
pub const LEGACY_MANIFEST_FILE_NAME: &str = "manifest.toml";
pub const LEGACY_OBJECT_DEFINITION_FILE_NAME: &str = "objects.toml";
pub const PAGES_DIR_NAME: &str = "pages";
pub const OBJECTS_DIR_NAME: &str = "objects";
pub const BUILD_DIR_NAME: &str = "dist";
pub const SCHEMAS_DIR_NAME: &str = "schemas";
pub const STATIC_DIR_NAME: &str = "public";
pub const LAYOUT_DIR_NAME: &str = "layout";
pub const SCRIPTS_DIR_NAME: &str = "scripts";
pub const SCRIPTS_BUILD_DIR_NAME: &str = "js";
pub const NESTED_TYPES: [&str; 5] = ["meta", "upload", "video", "audio", "image"];
#[cfg(debug_assertions)]
pub const UPLOADS_URL: &str = "http://localhost:7777";
#[cfg(not(debug_assertions))]
pub const UPLOADS_URL: &str = "https://uploads.archival.dev";
#[cfg(debug_assertions)]
#[cfg(feature = "binary")]
pub const API_URL: &str = "http://localhost:8777";
#[cfg(not(debug_assertions))]
#[cfg(feature = "binary")]
pub const API_URL: &str = "https://api.archival.dev";
#[cfg(debug_assertions)]
#[cfg(feature = "binary")]
pub const AUTH_URL: &str = "http://localhost:8788/cli-login";
#[cfg(not(debug_assertions))]
#[cfg(feature = "binary")]
pub const AUTH_URL: &str = "https://editor.archival.dev/cli-login";
#[cfg(debug_assertions)]
#[cfg(feature = "binary")]
pub const CLI_TOKEN_PUBLIC_KEY: &str = r#"-----BEGIN PUBLIC KEY-----
MIICIjANBgkqhkiG9w0BAQEFAAOCAg8AMIICCgKCAgEA8QoZ+84kiRHUf2EexGWg
+T2Cmajg1K9NYT4v4mAxtBPnmdmyMUf6nOhdmweVfRMA74/hGZHxLLAWnq6xDpf7
b90I2ypmMv+0uiLZUGJGwYs6gpnk3PuFvHtsPYMnXviC6XtiP16G8LJ32bnv/ICB
nIFew9EFKmdFreh2OqWvyS/gD3C+kaG9rt0za2+MnY6g3JdRD2HKlcThI+bhvXFk
RLvkQdXFgjEa463y+nl2QGQ8my7GJVK0qdpmvld9gtVbpuiPjHcXWjL5qNvxHNFM
ObleQUIzB7A1ETyHNUFJHtjcnHu60jqMu8r2ZeyAVDLdd6u4J7saW34YkE6UyVsh
G9nkkVr2QDbEkcdm9Z/L1ptgtEok8V4g8v6/3KSPlV0sGh9lhGUZFLwKhv1PX+v4
gItvv8XzIcmNHv+MuhW42XKbUpUvA9phQEK5/idcxD15Qf/yzp365yOxzN3rbHy/
X8ZZ9Hds6exALtdidZCo8DvD3m7+SftoDLVBEQgZLFpcOehGVOOaKgZIWiyT5BXU
PnAQcpKroW5lGQWGR+/NBkPU4YBZ7bSrR909/Z4Zx/bSNAKLL3xFmB5L8kW36/tG
5hrnw2Z6nsZY0WVGCRKSEX4cDbMTKI4D0CBPtrF3Efp5Z1H51jhNS+8txmKcpiMU
LK6+Gd4/xCPzLSMLwxOFjE0CAwEAAQ==
-----END PUBLIC KEY-----"#;
#[cfg(not(debug_assertions))]
#[cfg(feature = "binary")]
pub const CLI_TOKEN_PUBLIC_KEY: &str = r#"-----BEGIN PUBLIC KEY-----
MIICIjANBgkqhkiG9w0BAQEFAAOCAg8AMIICCgKCAgEAqxJb2SS+iHU9Cpdq4RVe
dvnswzRop7dpi5az1W7BjGTFJsxxh/tEPzpcRA2/PttX2TPWD+eR8kO7zlX822zp
kHGV7E/LYewSqTQpEnDTtituLChQFIHJ6LD8z15rUAGA3RV+RJMuYD5RH0kddBVD
MEhvDtw7J1tYOleCLvUlQn62qdXuR8ehSUodo5THalCTd3cLgeyr6pJFOS0JrLvG
CdM+zyW9Yos4Ms1ZNVSqueaTZygtFnpQg5FlBZJ8Zmm8slLSllrLvhmJG8CGruTM
Q8mUWZZKhJmV1GnmUwp1rp71CKgi/ekaMBVrL4jqboOkZnwey5EPahbIhfjy8v1Z
YyjnqQl1UDNzP9fK8clrLJ94F5QUUUw4MutJbMQ83AwAtkKhOBS5rO+sfBIRGtb6
97HCbakWIdaZEobRdChDoVv7mTpyIdBXpyYoIDNWCwKeSAd7etWa7KukBBBVmUes
TNcmS6gzzeoKp5F1Xwx3l3yEjMJC136nIrm9f2ixtTdWZLk8Y0JrM+2pjPZGizVs
3bPEMG8mtMXQUtZlXH12zgXqMO+H/XRErPvlQQ2BlvSvVbzCA9KCrKogFUNLzxdu
yhkwAIG+6uu96X/DJiNvJ4Rh/WXAhFb4pUAaLPtIAHl6BWsLRKlZQ93P0SbywP+1
njT01ssu01VM/SbeRW8QsTUCAwEAAQ==
-----END PUBLIC KEY-----"#;
