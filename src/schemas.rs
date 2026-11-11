//! The JSON Schema files published to <https://archival.dev/schemas/> and
//! registered with SchemaStore, embedded so that dependents get the schemas
//! belonging to the exact archival version they pin.
//!
//! This exists mainly for archival-editor, which owns the types
//! [`ARCHIVAL_EDITOR`] describes but is a private repo, so the schema is
//! published from here. Reading it out of the crate rather than fetching it
//! means its tests can assert the two agree without a network call and without a
//! second copy to keep current — see the tests in `src/schema_files.rs` for the
//! guards on this side.

/// Schema for `archival.toml`.
pub const ARCHIVAL: &str = include_str!("../archival.schema.json");
/// Schema for `archival_objects.toml` (or whatever `object_file` names).
pub const ARCHIVAL_OBJECTS: &str = include_str!("../archival_objects.schema.json");
/// Schema for `archival_editor.toml`.
pub const ARCHIVAL_EDITOR: &str = include_str!("../archival_editor.schema.json");
/// Schema for `archival_template.toml`.
pub const ARCHIVAL_TEMPLATE: &str = include_str!("../archival_template.schema.json");
/// Schema for the social snapshot `archival build --social` reads.
pub const ARCHIVAL_SOCIAL: &str = include_str!("../archival_social.schema.json");
