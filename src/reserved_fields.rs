use std::{error::Error, fmt};

// These fields may not be used as keys in object definitions or as the names of
// objects.
pub const TEMPLATE: &str = "template";
pub const ORDER: &str = "order";
pub const PATH: &str = "path";
pub const OBJECTS: &str = "objects";
pub const OBJECT_NAME: &str = "object_name";
pub const PAGE: &str = "page";
pub const PAGE_NAME: &str = "page_name";

/// The canonical set of reserved names. objects.schema.json forbids these as
/// object and field names, and the schema_files tests assert the two agree.
pub const RESERVED_FIELDS: [&str; 7] =
    [OBJECT_NAME, ORDER, OBJECTS, PAGE_NAME, PAGE, PATH, TEMPLATE];

#[derive(Debug, Clone)]
pub struct ReservedFieldError {
    pub field: &'static str,
}
impl Error for ReservedFieldError {}
impl fmt::Display for ReservedFieldError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} is a reserved field", self.field)
    }
}

pub fn reserved_field_from_str(field: &str) -> &'static str {
    match field {
        OBJECT_NAME => OBJECT_NAME,
        ORDER => ORDER,
        PATH => PATH,
        PAGE_NAME => PAGE_NAME,
        TEMPLATE => TEMPLATE,
        OBJECTS => OBJECTS,
        PAGE => PAGE,
        _ => panic!("{} is not a reserved field", field),
    }
}

pub fn is_reserved_field(field: &str) -> bool {
    RESERVED_FIELDS.contains(&field)
}
