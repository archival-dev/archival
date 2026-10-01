pub mod archival_proto {
    include!(concat!(env!("OUT_DIR"), "/archival.v1.rs"));
}

use crate::events::ArchivalEvent;
use crate::fields::field_type::OneofOption;
use crate::fields::meta::Meta;
use crate::fields::{DateTime, DisplayType, FieldType, FieldValue, File, MetaValue, RenderedFile};
use crate::object::{Object, ObjectEntry, ObjectMap};
use crate::object_definition::{FieldDefinition, FieldsMap, ObjectDefinition};
use crate::value_path::{ValuePath, ValuePathComponent};
use ordermap::OrderMap;
use std::collections::BTreeMap;

// DisplayType
impl From<archival_proto::DisplayType> for DisplayType {
    fn from(proto: archival_proto::DisplayType) -> Self {
        match proto {
            archival_proto::DisplayType::Image => DisplayType::Image,
            archival_proto::DisplayType::Video => DisplayType::Video,
            archival_proto::DisplayType::Audio => DisplayType::Audio,
            archival_proto::DisplayType::Download => DisplayType::Download,
            archival_proto::DisplayType::Unspecified => DisplayType::Download,
        }
    }
}

// DisplayType -> proto
impl From<DisplayType> for archival_proto::DisplayType {
    fn from(dt: DisplayType) -> Self {
        match dt {
            DisplayType::Image => archival_proto::DisplayType::Image,
            DisplayType::Video => archival_proto::DisplayType::Video,
            DisplayType::Audio => archival_proto::DisplayType::Audio,
            DisplayType::Download => archival_proto::DisplayType::Download,
        }
    }
}

// File
impl From<archival_proto::File> for File {
    fn from(proto: archival_proto::File) -> Self {
        File {
            display_type: archival_proto::DisplayType::try_from(proto.display_type)
                .ok()
                .unwrap_or(archival_proto::DisplayType::Download)
                .into(),
            filename: proto.filename,
            sha: proto.sha,
            mime: proto.mime,
            name: if proto.name.is_empty() {
                None
            } else {
                Some(proto.name)
            },
            description: if proto.description.is_empty() {
                None
            } else {
                Some(proto.description)
            },
        }
    }
}

// File -> proto
impl From<File> for archival_proto::File {
    fn from(f: File) -> Self {
        archival_proto::File {
            display_type: archival_proto::DisplayType::from(f.display_type) as i32,
            filename: f.filename,
            sha: f.sha,
            mime: f.mime,
            name: f.name.unwrap_or_default(),
            description: f.description.unwrap_or_default(),
        }
    }
}

// RenderedFile
impl From<archival_proto::RenderedFile> for RenderedFile {
    fn from(proto: archival_proto::RenderedFile) -> Self {
        RenderedFile {
            display_type: archival_proto::DisplayType::try_from(proto.display_type)
                .ok()
                .unwrap_or(archival_proto::DisplayType::Download)
                .into(),
            filename: proto.filename,
            sha: proto.sha,
            mime: proto.mime,
            name: if proto.name.is_empty() {
                None
            } else {
                Some(proto.name)
            },
            description: if proto.description.is_empty() {
                None
            } else {
                Some(proto.description)
            },
            url: proto.url,
        }
    }
}

// RenderedFile -> proto
impl From<RenderedFile> for archival_proto::RenderedFile {
    fn from(f: RenderedFile) -> Self {
        archival_proto::RenderedFile {
            display_type: archival_proto::DisplayType::from(f.display_type) as i32,
            filename: f.filename,
            sha: f.sha,
            mime: f.mime,
            name: f.name.unwrap_or_default(),
            description: f.description.unwrap_or_default(),
            url: f.url,
        }
    }
}

// MetaValue
impl From<archival_proto::MetaValue> for MetaValue {
    fn from(proto: archival_proto::MetaValue) -> Self {
        match proto.value {
            Some(archival_proto::meta_value::Value::String(s)) => MetaValue::String(s),
            Some(archival_proto::meta_value::Value::Number(n)) => MetaValue::Number(n),
            Some(archival_proto::meta_value::Value::Boolean(b)) => MetaValue::Boolean(b),
            Some(archival_proto::meta_value::Value::DateString(d)) => {
                MetaValue::DateTime(DateTime::from(&d).unwrap_or_else(|_| DateTime::now()))
            }
            Some(archival_proto::meta_value::Value::Array(arr)) => {
                MetaValue::Array(arr.values.into_iter().map(|v| v.into()).collect())
            }
            Some(archival_proto::meta_value::Value::Map(m)) => MetaValue::Map(m.into()),
            None => MetaValue::String(String::new()),
        }
    }
}

// MetaValue -> proto
impl From<MetaValue> for archival_proto::MetaValue {
    fn from(mv: MetaValue) -> Self {
        let value = match mv {
            MetaValue::String(s) => archival_proto::meta_value::Value::String(s),
            MetaValue::Number(n) => archival_proto::meta_value::Value::Number(n),
            MetaValue::Boolean(b) => archival_proto::meta_value::Value::Boolean(b),
            MetaValue::DateTime(dt) => {
                archival_proto::meta_value::Value::DateString(dt.to_string())
            }
            MetaValue::Array(arr) => {
                archival_proto::meta_value::Value::Array(archival_proto::MetaArray {
                    values: arr.into_iter().map(|v| v.into()).collect(),
                })
            }
            MetaValue::Map(map) => archival_proto::meta_value::Value::Map(map.into()),
        };
        archival_proto::MetaValue { value: Some(value) }
    }
}

// Meta
impl From<archival_proto::Meta> for Meta {
    fn from(proto: archival_proto::Meta) -> Self {
        let mut meta_map = OrderMap::new();
        for entry in proto.entries {
            if let Some(value) = entry.value {
                meta_map.insert(entry.name, value.into());
            }
        }
        Meta(meta_map)
    }
}

// Meta -> proto
impl From<Meta> for archival_proto::Meta {
    fn from(meta: Meta) -> Self {
        let mut entries = Vec::new();
        for (name, value) in meta.0.into_iter() {
            entries.push(archival_proto::meta::Entry {
                name,
                value: Some(value.into()),
            });
        }
        archival_proto::Meta { entries }
    }
}

// FieldValue
impl From<archival_proto::FieldValue> for FieldValue {
    fn from(proto: archival_proto::FieldValue) -> Self {
        match proto.value {
            Some(archival_proto::field_value::Value::String(s)) => FieldValue::String(s),
            Some(archival_proto::field_value::Value::Secret(s)) => FieldValue::Secret(s),
            Some(archival_proto::field_value::Value::Enum(e)) => FieldValue::Enum(e),
            Some(archival_proto::field_value::Value::Markdown(m)) => FieldValue::Markdown(m),
            Some(archival_proto::field_value::Value::Number(n)) => FieldValue::Number(n),
            Some(archival_proto::field_value::Value::DateString(d)) => {
                FieldValue::Date(DateTime::from(&d).unwrap_or_else(|_| DateTime::now()))
            }
            Some(archival_proto::field_value::Value::Objects(obj)) => {
                let mut fields = BTreeMap::new();
                for f in obj.fields {
                    fields.insert(
                        f.name,
                        f.value.map(|v| v.into()).unwrap_or(FieldValue::Null),
                    );
                }
                // FieldValue::Objects expects Vec<ObjectValues>
                FieldValue::Objects(vec![fields])
            }
            Some(archival_proto::field_value::Value::Oneof(val)) => {
                FieldValue::Oneof((val.name, Box::new(val.value.map(|f| FieldValue::from(*f)))))
            }
            Some(archival_proto::field_value::Value::Boolean(b)) => FieldValue::Boolean(b),
            Some(archival_proto::field_value::Value::File(f)) => FieldValue::File(f.into()),
            Some(archival_proto::field_value::Value::Meta(m)) => FieldValue::Meta(m.into()),
            Some(archival_proto::field_value::Value::Null(_)) => FieldValue::Null,
            None => FieldValue::Null,
        }
    }
}

// FieldValue -> proto
impl From<FieldValue> for archival_proto::FieldValue {
    fn from(fv: FieldValue) -> Self {
        let value = match fv {
            FieldValue::String(s) => archival_proto::field_value::Value::String(s),
            FieldValue::Secret(s) => archival_proto::field_value::Value::Secret(s),
            FieldValue::Enum(e) => archival_proto::field_value::Value::Enum(e),
            FieldValue::Markdown(m) => archival_proto::field_value::Value::Markdown(m),
            FieldValue::Number(n) => archival_proto::field_value::Value::Number(n),
            FieldValue::Date(d) => archival_proto::field_value::Value::DateString(d.to_string()),
            FieldValue::Objects(objs) => {
                let mut fields = Vec::new();
                // FieldValue::Objects is Vec<BTreeMap<String, FieldValue>>; proto expects one ObjectValues
                if let Some(first) = objs.into_iter().next() {
                    for (name, val) in first.into_iter() {
                        fields.push(archival_proto::object_values::Field {
                            name,
                            value: Some(val.into()),
                        });
                    }
                }
                archival_proto::field_value::Value::Objects(archival_proto::ObjectValues { fields })
            }
            FieldValue::Oneof((name, val)) => {
                archival_proto::field_value::Value::Oneof(Box::new(archival_proto::OneofValue {
                    name,
                    value: val.map(|f| Box::new(archival_proto::FieldValue::from(f))),
                }))
            }
            FieldValue::Boolean(b) => archival_proto::field_value::Value::Boolean(b),
            FieldValue::File(file) => archival_proto::field_value::Value::File(file.into()),
            FieldValue::Meta(m) => archival_proto::field_value::Value::Meta(m.into()),
            FieldValue::Null => archival_proto::field_value::Value::Null(()),
        };
        archival_proto::FieldValue { value: Some(value) }
    }
}

// ObjectValues -> BTreeMap<String, RustFieldValue>
impl From<archival_proto::ObjectValues> for BTreeMap<String, FieldValue> {
    fn from(proto: archival_proto::ObjectValues) -> Self {
        proto
            .fields
            .into_iter()
            .map(|f| {
                (
                    f.name,
                    f.value.map(|v| v.into()).unwrap_or(FieldValue::Null),
                )
            })
            .collect()
    }
}

// Object
impl From<archival_proto::Object> for Object {
    fn from(proto: archival_proto::Object) -> Self {
        let order = match proto.order {
            Some(archival_proto::object::Order::None(_)) => None,
            Some(archival_proto::object::Order::Some(o)) => Some(o),
            None => None,
        };
        Object {
            filename: proto.filename,
            object_name: proto.object_name,
            order,
            values: proto.values.map(|v| v.into()).unwrap_or_default(),
        }
    }
}

// Object -> proto
impl From<Object> for archival_proto::Object {
    fn from(obj: Object) -> Self {
        let order = match obj.order {
            None => Some(archival_proto::object::Order::None(())),
            Some(o) => Some(archival_proto::object::Order::Some(o)),
        };
        archival_proto::Object {
            filename: obj.filename,
            object_name: obj.object_name,
            order,
            values: Some(obj.values.into()),
        }
    }
}

// ObjectEntry
impl From<archival_proto::ObjectEntry> for ObjectEntry {
    fn from(proto: archival_proto::ObjectEntry) -> Self {
        match proto.entry {
            Some(archival_proto::object_entry::Entry::List(list)) => {
                ObjectEntry::List(list.entries.into_iter().map(|o| o.into()).collect())
            }
            Some(archival_proto::object_entry::Entry::Object(obj)) => {
                ObjectEntry::Object(obj.into())
            }
            None => ObjectEntry::Object(Object {
                filename: String::new(),
                object_name: String::new(),
                order: None,
                values: BTreeMap::new(),
            }),
        }
    }
}

// ObjectEntry -> proto
impl From<ObjectEntry> for archival_proto::ObjectEntry {
    fn from(entry: ObjectEntry) -> Self {
        match entry {
            ObjectEntry::List(list) => archival_proto::ObjectEntry {
                entry: Some(archival_proto::object_entry::Entry::List(
                    archival_proto::ObjectList {
                        entries: list.into_iter().map(|o| o.into()).collect(),
                    },
                )),
            },
            ObjectEntry::Object(obj) => archival_proto::ObjectEntry {
                entry: Some(archival_proto::object_entry::Entry::Object(obj.into())),
            },
        }
    }
}

// ObjectMap
impl From<archival_proto::ObjectMap> for ObjectMap {
    fn from(proto: archival_proto::ObjectMap) -> Self {
        proto
            .entries
            .into_iter()
            .map(|e| {
                (
                    e.name,
                    e.object.map(|o| o.into()).unwrap_or_else(|| {
                        ObjectEntry::Object(Object {
                            filename: String::new(),
                            object_name: String::new(),
                            order: None,
                            values: BTreeMap::new(),
                        })
                    }),
                )
            })
            .collect()
    }
}

// ObjectMap -> proto
impl From<ObjectMap> for archival_proto::ObjectMap {
    fn from(map: ObjectMap) -> Self {
        let entries = map
            .into_iter()
            .map(|(name, object_entry)| archival_proto::object_map::Entry {
                name,
                object: Some(object_entry.into()),
            })
            .collect();
        archival_proto::ObjectMap { entries }
    }
}

// ValuePathComponent
impl From<archival_proto::ValuePathComponent> for ValuePathComponent {
    fn from(proto: archival_proto::ValuePathComponent) -> Self {
        match proto.component {
            Some(archival_proto::value_path_component::Component::Key(k)) => {
                ValuePathComponent::Key(k)
            }
            Some(archival_proto::value_path_component::Component::Index(i)) => {
                ValuePathComponent::Index(i as usize)
            }
            None => ValuePathComponent::Key(String::new()),
        }
    }
}

// ValuePathComponent -> proto
impl From<ValuePathComponent> for archival_proto::ValuePathComponent {
    fn from(c: ValuePathComponent) -> Self {
        let component = match c {
            ValuePathComponent::Key(k) => archival_proto::value_path_component::Component::Key(k),
            ValuePathComponent::Index(i) => {
                archival_proto::value_path_component::Component::Index(i as u32)
            }
        };
        archival_proto::ValuePathComponent {
            component: Some(component),
        }
    }
}

// ValuePath - convert from proto to rust by building components
impl From<archival_proto::ValuePath> for ValuePath {
    fn from(proto: archival_proto::ValuePath) -> Self {
        let mut path = ValuePath::empty();
        for component_proto in proto.path {
            let component: ValuePathComponent = component_proto.into();
            path = path.append(component);
        }
        path
    }
}

// ValuePath -> proto
impl From<ValuePath> for archival_proto::ValuePath {
    fn from(vp: ValuePath) -> Self {
        archival_proto::ValuePath {
            path: vp.into_iter().map(|c| c.into()).collect(),
        }
    }
}
// Also ok to store these as strings
impl From<ValuePath> for String {
    fn from(value: ValuePath) -> Self {
        value.to_string()
    }
}
impl From<String> for ValuePath {
    fn from(value: String) -> Self {
        ValuePath::from_string(&value)
    }
}

// FieldType - convert from proto struct's oneof field to rust enum
impl From<archival_proto::FieldType> for FieldType {
    fn from(proto: archival_proto::FieldType) -> Self {
        match proto.r#type {
            Some(archival_proto::field_type::Type::String(())) => FieldType::String,
            Some(archival_proto::field_type::Type::Secret(())) => FieldType::Secret,
            Some(archival_proto::field_type::Type::Number(())) => FieldType::Number,
            Some(archival_proto::field_type::Type::Date(())) => FieldType::Date,
            Some(archival_proto::field_type::Type::Enum(values)) => FieldType::Enum(values.values),
            Some(archival_proto::field_type::Type::Markdown(())) => FieldType::Markdown,
            Some(archival_proto::field_type::Type::Boolean(())) => FieldType::Boolean,
            Some(archival_proto::field_type::Type::Oneof(opts)) => FieldType::Oneof(
                opts.options
                    .into_iter()
                    .map(|option| OneofOption {
                        name: option.name,
                        r#type: option.r#type.expect("missing type on oneof option").into(),
                    })
                    .collect(),
            ),
            Some(archival_proto::field_type::Type::Image(())) => FieldType::Image,
            Some(archival_proto::field_type::Type::Video(())) => FieldType::Video,
            Some(archival_proto::field_type::Type::Upload(())) => FieldType::Upload,
            Some(archival_proto::field_type::Type::Audio(())) => FieldType::Audio,
            Some(archival_proto::field_type::Type::Meta(())) => FieldType::Meta,
            Some(archival_proto::field_type::Type::Alias(alias)) => {
                // Convert AliasType to FieldType - alias.r#type is Option<Box<proto_gen::FieldType>>
                if let Some(boxed_type) = alias.r#type {
                    let converted: FieldType = (*boxed_type).into();
                    FieldType::Alias(Box::new((converted, alias.name)))
                } else {
                    FieldType::String
                }
            }
            None => FieldType::String,
        }
    }
}

// FieldsMap from proto
impl From<archival_proto::FieldsMap> for FieldsMap {
    fn from(proto: archival_proto::FieldsMap) -> Self {
        let mut map = FieldsMap::new();
        for field in proto.fields {
            let field_type = field.r#type.map(|t| t.into()).unwrap_or(FieldType::String);
            map.insert(
                field.name,
                FieldDefinition::new(field_type, non_empty(field.description)),
            );
        }
        map
    }
}

// FieldsMap -> proto FieldsMap
impl From<FieldsMap> for archival_proto::FieldsMap {
    fn from(map: FieldsMap) -> Self {
        let fields = map
            .into_iter()
            .map(|(name, field)| archival_proto::fields_map::Field {
                name,
                r#type: Some(field.r#type.into()),
                description: field.description.unwrap_or_default(),
            })
            .collect();
        archival_proto::FieldsMap { fields }
    }
}

/// proto3 has no null, so this file uses the empty string for absent optional
/// strings (as `ObjectDefinition::template` already did).
fn non_empty(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

// FieldType -> proto
impl From<FieldType> for archival_proto::FieldType {
    fn from(ft: FieldType) -> Self {
        use archival_proto::field_type::Type;
        let r#type = match ft {
            FieldType::String => Type::String(()),
            FieldType::Secret => Type::Secret(()),
            FieldType::Number => Type::Number(()),
            FieldType::Date => Type::Date(()),
            FieldType::Enum(vals) => Type::Enum(archival_proto::EnumFieldType { values: vals }),
            FieldType::Markdown => Type::Markdown(()),
            FieldType::Boolean => Type::Boolean(()),
            FieldType::Oneof(opts) => Type::Oneof(archival_proto::OneofFieldType {
                options: opts
                    .into_iter()
                    .map(|option| archival_proto::OneofFieldTypeOption {
                        name: option.name,
                        r#type: Some(option.r#type.into()),
                    })
                    .collect(),
            }),
            FieldType::Image => Type::Image(()),
            FieldType::Video => Type::Video(()),
            FieldType::Upload => Type::Upload(()),
            FieldType::Audio => Type::Audio(()),
            FieldType::Meta => Type::Meta(()),
            FieldType::Alias(boxed) => {
                let (inner, name) = *boxed;
                Type::Alias(Box::new(archival_proto::AliasType {
                    r#type: Some(Box::new(archival_proto::FieldType::from(inner))),
                    name,
                }))
            }
        };
        archival_proto::FieldType {
            r#type: Some(r#type),
        }
    }
}

// ObjectDefinition
impl From<archival_proto::ObjectDefinition> for ObjectDefinition {
    fn from(proto: archival_proto::ObjectDefinition) -> Self {
        let fields: FieldsMap = proto.fields.map(|f| f.into()).unwrap_or_default();

        let mut children = OrderMap::new();
        if let Some(cd) = proto.children {
            for child in cd.children {
                let child_def: ObjectDefinition = child
                    .definition
                    .unwrap_or_else(|| archival_proto::ObjectDefinition {
                        name: String::new(),
                        fields: None,
                        template: String::new(),
                        children: None,
                        description: String::new(),
                    })
                    .into();
                children.insert(child.name, child_def);
            }
        }

        ObjectDefinition {
            name: proto.name,
            fields,
            template: non_empty(proto.template),
            children,
            description: non_empty(proto.description),
        }
    }
}

// ObjectDefinition -> proto
impl From<ObjectDefinition> for archival_proto::ObjectDefinition {
    fn from(def: ObjectDefinition) -> Self {
        let fields = if def.fields.is_empty() {
            None
        } else {
            Some(def.fields.into())
        };

        let children = if def.children.is_empty() {
            None
        } else {
            Some(archival_proto::ChildDefinitions {
                children: def
                    .children
                    .into_iter()
                    .map(
                        |(name, child_def)| archival_proto::child_definitions::Child {
                            name,
                            definition: Some(child_def.into()),
                        },
                    )
                    .collect(),
            })
        };

        archival_proto::ObjectDefinition {
            name: def.name,
            fields,
            template: def.template.unwrap_or_default(),
            children,
            description: def.description.unwrap_or_default(),
        }
    }
}

// BTreeMap<String, FieldValue> -> ObjectValues proto
impl From<BTreeMap<String, FieldValue>> for archival_proto::ObjectValues {
    fn from(map: BTreeMap<String, FieldValue>) -> Self {
        let fields = map
            .into_iter()
            .map(|(name, value)| archival_proto::object_values::Field {
                name,
                value: Some(value.into()),
            })
            .collect();
        archival_proto::ObjectValues { fields }
    }
}

// ArchivalEvent
impl From<archival_proto::ArchivalEvent> for ArchivalEvent {
    fn from(value: archival_proto::ArchivalEvent) -> Self {
        value
            .event
            .map(|ev| match ev {
                archival_proto::archival_event::Event::RenameObject(rename_object_event) => {
                    ArchivalEvent::RenameObject(rename_object_event.into())
                }
                archival_proto::archival_event::Event::AddObject(add_object_event) => {
                    ArchivalEvent::AddObject(add_object_event.into())
                }
                archival_proto::archival_event::Event::AddRootObject(add_root_object_event) => {
                    ArchivalEvent::AddRootObject(add_root_object_event.into())
                }
                archival_proto::archival_event::Event::DeleteObject(delete_object_event) => {
                    ArchivalEvent::DeleteObject(delete_object_event.into())
                }
                archival_proto::archival_event::Event::EditField(edit_field_event) => {
                    ArchivalEvent::EditField(edit_field_event.into())
                }
                archival_proto::archival_event::Event::EditOrder(edit_order_event) => {
                    ArchivalEvent::EditOrder(edit_order_event.into())
                }
                archival_proto::archival_event::Event::AddChild(add_child_event) => {
                    ArchivalEvent::AddChild(add_child_event.into())
                }
                archival_proto::archival_event::Event::RemoveChild(remove_child_event) => {
                    ArchivalEvent::RemoveChild(remove_child_event.into())
                }
            })
            .unwrap_or_else(|| panic!("Invalid archival event proto: missing event field"))
    }
}

// Event Types
use crate::events;

// AddObjectValue (proto) -> events::AddObjectValue (rust)
impl From<archival_proto::AddObjectValue> for events::AddObjectValue {
    fn from(value: archival_proto::AddObjectValue) -> Self {
        events::AddObjectValue {
            path: value
                .path
                .map(|p| p.into())
                .unwrap_or_else(ValuePath::empty),
            value: value.value.map(|v| v.into()).unwrap_or(FieldValue::Null),
        }
    }
}
impl From<archival_proto::RenameObjectEvent> for events::RenameObjectEvent {
    fn from(value: archival_proto::RenameObjectEvent) -> Self {
        events::RenameObjectEvent {
            object: value.object,
            from: value.from,
            to: value.to,
        }
    }
}
impl From<archival_proto::AddObjectEvent> for events::AddObjectEvent {
    fn from(value: archival_proto::AddObjectEvent) -> Self {
        let order = match value.order {
            Some(archival_proto::add_object_event::Order::None(_)) => None,
            Some(archival_proto::add_object_event::Order::Some(o)) => Some(o),
            None => None,
        };
        events::AddObjectEvent {
            object: value.object,
            filename: value.filename,
            order,
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}
impl From<archival_proto::AddRootObjectEvent> for events::AddRootObjectEvent {
    fn from(value: archival_proto::AddRootObjectEvent) -> Self {
        events::AddRootObjectEvent {
            object: value.object,
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}
impl From<archival_proto::DeleteObjectEvent> for events::DeleteObjectEvent {
    fn from(value: archival_proto::DeleteObjectEvent) -> Self {
        events::DeleteObjectEvent {
            object: value.object,
            filename: value.filename,
            source: if value.source.is_empty() {
                None
            } else {
                Some(value.source)
            },
        }
    }
}
impl From<archival_proto::EditFieldEvent> for events::EditFieldEvent {
    fn from(value: archival_proto::EditFieldEvent) -> Self {
        events::EditFieldEvent {
            object: value.object,
            filename: value.filename,
            path: value
                .path
                .map(|p| p.into())
                .unwrap_or_else(ValuePath::empty),
            field: value.field,
            value: value.value.map(|v| v.into()),
            source: if value.source.is_empty() {
                None
            } else {
                Some(value.source)
            },
        }
    }
}
impl From<archival_proto::EditOrderEvent> for events::EditOrderEvent {
    fn from(value: archival_proto::EditOrderEvent) -> Self {
        let order = match value.order {
            Some(archival_proto::edit_order_event::Order::None(_)) => None,
            Some(archival_proto::edit_order_event::Order::Some(o)) => Some(o),
            None => None,
        };
        events::EditOrderEvent {
            object: value.object,
            filename: value.filename,
            order,
            source: if value.source.is_empty() {
                None
            } else {
                Some(value.source)
            },
        }
    }
}
impl From<archival_proto::AddChildEvent> for events::AddChildEvent {
    fn from(value: archival_proto::AddChildEvent) -> Self {
        let idx = match value.index {
            Some(archival_proto::add_child_event::Index::None(_)) => None,
            Some(archival_proto::add_child_event::Index::Some(i)) => Some(i),
            None => None,
        };
        events::AddChildEvent {
            object: value.object,
            filename: value.filename,
            path: value
                .path
                .map(|p| p.into())
                .unwrap_or_else(ValuePath::empty),
            values: value.values.into_iter().map(|v| v.into()).collect(),
            index: idx,
        }
    }
}
impl From<archival_proto::RemoveChildEvent> for events::RemoveChildEvent {
    fn from(value: archival_proto::RemoveChildEvent) -> Self {
        events::RemoveChildEvent {
            object: value.object,
            filename: value.filename,
            path: value
                .path
                .map(|p| p.into())
                .unwrap_or_else(ValuePath::empty),
            source: if value.source.is_empty() {
                None
            } else {
                Some(value.source)
            },
        }
    }
}

// ArchivalEvent -> proto
impl From<ArchivalEvent> for archival_proto::ArchivalEvent {
    fn from(value: ArchivalEvent) -> Self {
        let event = match value {
            ArchivalEvent::RenameObject(rename_object_event) => {
                archival_proto::archival_event::Event::RenameObject(rename_object_event.into())
            }
            ArchivalEvent::AddObject(add_object_event) => {
                archival_proto::archival_event::Event::AddObject(add_object_event.into())
            }
            ArchivalEvent::AddRootObject(add_root_object_event) => {
                archival_proto::archival_event::Event::AddRootObject(add_root_object_event.into())
            }
            ArchivalEvent::DeleteObject(delete_object_event) => {
                archival_proto::archival_event::Event::DeleteObject(delete_object_event.into())
            }
            ArchivalEvent::EditField(edit_field_event) => {
                archival_proto::archival_event::Event::EditField(edit_field_event.into())
            }
            ArchivalEvent::EditOrder(edit_order_event) => {
                archival_proto::archival_event::Event::EditOrder(edit_order_event.into())
            }
            ArchivalEvent::AddChild(add_child_event) => {
                archival_proto::archival_event::Event::AddChild(add_child_event.into())
            }
            ArchivalEvent::RemoveChild(remove_child_event) => {
                archival_proto::archival_event::Event::RemoveChild(remove_child_event.into())
            }
        };
        archival_proto::ArchivalEvent { event: Some(event) }
    }
}

// Event Types (rust -> proto)
impl From<events::AddObjectValue> for archival_proto::AddObjectValue {
    fn from(value: events::AddObjectValue) -> Self {
        archival_proto::AddObjectValue {
            path: Some(value.path.into()),
            value: Some(value.value.into()),
        }
    }
}
impl From<events::RenameObjectEvent> for archival_proto::RenameObjectEvent {
    fn from(value: events::RenameObjectEvent) -> Self {
        archival_proto::RenameObjectEvent {
            object: value.object,
            from: value.from,
            to: value.to,
        }
    }
}
impl From<events::AddObjectEvent> for archival_proto::AddObjectEvent {
    fn from(value: events::AddObjectEvent) -> Self {
        let order = match value.order {
            None => Some(archival_proto::add_object_event::Order::None(())),
            Some(o) => Some(archival_proto::add_object_event::Order::Some(o)),
        };
        archival_proto::AddObjectEvent {
            object: value.object,
            filename: value.filename,
            order,
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}
impl From<events::AddRootObjectEvent> for archival_proto::AddRootObjectEvent {
    fn from(value: events::AddRootObjectEvent) -> Self {
        archival_proto::AddRootObjectEvent {
            object: value.object,
            values: value.values.into_iter().map(|v| v.into()).collect(),
        }
    }
}
impl From<events::DeleteObjectEvent> for archival_proto::DeleteObjectEvent {
    fn from(value: events::DeleteObjectEvent) -> Self {
        archival_proto::DeleteObjectEvent {
            object: value.object,
            filename: value.filename,
            source: value.source.unwrap_or_default(),
        }
    }
}
impl From<events::EditFieldEvent> for archival_proto::EditFieldEvent {
    fn from(value: events::EditFieldEvent) -> Self {
        archival_proto::EditFieldEvent {
            object: value.object,
            filename: value.filename,
            path: Some(value.path.into()),
            field: value.field,
            value: value.value.map(|v| v.into()),
            source: value.source.unwrap_or_default(),
        }
    }
}
impl From<events::EditOrderEvent> for archival_proto::EditOrderEvent {
    fn from(value: events::EditOrderEvent) -> Self {
        let order = match value.order {
            None => Some(archival_proto::edit_order_event::Order::None(())),
            Some(o) => Some(archival_proto::edit_order_event::Order::Some(o)),
        };
        archival_proto::EditOrderEvent {
            object: value.object,
            filename: value.filename,
            order,
            source: value.source.unwrap_or_default(),
        }
    }
}
impl From<events::AddChildEvent> for archival_proto::AddChildEvent {
    fn from(value: events::AddChildEvent) -> Self {
        let index = match value.index {
            None => Some(archival_proto::add_child_event::Index::None(())),
            Some(i) => Some(archival_proto::add_child_event::Index::Some(i)),
        };
        archival_proto::AddChildEvent {
            object: value.object,
            filename: value.filename,
            path: Some(value.path.into()),
            values: value.values.into_iter().map(|v| v.into()).collect(),
            index,
        }
    }
}
impl From<events::RemoveChildEvent> for archival_proto::RemoveChildEvent {
    fn from(value: events::RemoveChildEvent) -> Self {
        archival_proto::RemoveChildEvent {
            object: value.object,
            filename: value.filename,
            path: Some(value.path.into()),
            source: value.source.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod proto_tests {
    use crate::{
        archival_proto, events, fields, object, FieldDefinition, FieldValue, FieldsMap,
        ObjectDefinition, ObjectDefinitions, ObjectMap,
    };
    use prost::Message;
    macro_rules! proto_test {
        ($proto:ty => $typ:ty, $test_name:ident { $($example:block);* $(;)? }) => {
            #[test]
            fn $test_name() {
                $(
                    let val = $example;
                    let mut buf = Vec::new();
                    <$proto>::from(val.clone())
                        .encode(&mut buf)
                        .expect("encode failed");
                    assert_eq!(
                        <$proto>::decode(buf.as_slice())
                            .map(|proto| <$typ>::from(proto))
                            .expect("decode failed"),
                        val
                    );
                )*
            }
        };
        ($proto:ty => $typ:ty, $test_name:ident { $($example:expr);* $(;)? }) => {
            proto_test!($proto => $typ, $test_name { $( { $example } );* });
        };
    }

    proto_test!(archival_proto::ArchivalEvent => events::ArchivalEvent, archival_event_test {
        events::ArchivalEvent::AddObject(events::AddObjectEvent { object: "object".to_string(), filename: "foo".to_string(), order: None, values: vec![] });
        events::ArchivalEvent::AddRootObject(events::AddRootObjectEvent { object: "root".to_string(), values: vec![] });
        events::ArchivalEvent::DeleteObject(events::DeleteObjectEvent { object: "object".to_string(), filename: "foo".to_string(), source: None });
        events::ArchivalEvent::EditField(events::EditFieldEvent { object: "object".to_string(), filename: "foo".to_string(), path: object::ValuePath::from_string("field/0"), field: "title".to_string(), value: Some(FieldValue::String("updated".to_string())), source: Some("script".to_string()) });
        events::ArchivalEvent::EditOrder(events::EditOrderEvent { object: "object".to_string(), filename: "foo".to_string(), order: Some(12.34), source: None });
        events::ArchivalEvent::AddChild(events::AddChildEvent { object: "parent".to_string(), filename: "parent_file".to_string(), path: object::ValuePath::from_string("children/0"), values: vec![], index: Some(0) });
        events::ArchivalEvent::RemoveChild(events::RemoveChildEvent { object: "parent".to_string(), filename: "parent_file".to_string(), path: object::ValuePath::from_string("children/0"), source: Some("user".to_string()) });
        events::ArchivalEvent::RenameObject(events::RenameObjectEvent { object: "object".to_string(), from: "old_name".to_string(), to: "new_name".to_string() });
    });
    proto_test!(archival_proto::File => fields::File, file_test {
        fields::File::download();
    });

    proto_test!(archival_proto::RenderedFile => fields::RenderedFile, rendered_file_test {
        fields::RenderedFile {
            display_type: fields::DisplayType::Image,
            filename: "photo.jpg".to_string(),
            sha: "abc123".to_string(),
            mime: "image/jpeg".to_string(),
            name: Some("cover".to_string()),
            description: Some("Hero image".to_string()),
            url: "/uploads/photo.jpg".to_string(),
        };
    });

    proto_test!(archival_proto::MetaValue => fields::MetaValue, meta_value_test {
        fields::MetaValue::Map(fields::Meta::from(serde_json::json!({"foo": {"bar": ["baz"], "fill": 22}}).as_object().unwrap()));
    });

    proto_test!(archival_proto::Meta => fields::Meta, meta_test {
        fields::Meta::from(serde_json::json!({"foo": {"bar": ["baz"], "fill": 22}}).as_object().unwrap());
    });

    proto_test!(archival_proto::FieldValue => FieldValue, field_value_test {
        FieldValue::String("Test".to_string());
        FieldValue::Markdown("**test**".to_string());
    });

    proto_test!(archival_proto::Object => object::Object, object_test {
        object::Object {
            filename: "testing".to_string(),
            object_name: "foo".to_string(),
            order: Some(23.90),
            values: fields::ObjectValues::from([("test".to_string(), FieldValue::String("Test".to_string()))])
        };
    });
    proto_test!(archival_proto::ObjectEntry => object::ObjectEntry, object_entry_test {
        object::ObjectEntry::from_vec(vec![object::Object {
            filename: "testing".to_string(),
            object_name: "foo".to_string(),
            order: Some(23.90),
            values: fields::ObjectValues::from([("test".to_string(), FieldValue::String("Test".to_string()))])
        }]);
    });
    proto_test!(archival_proto::ObjectMap => ObjectMap, object_map_test {
        ObjectMap::from([("name".to_string(), object::ObjectEntry::from_vec(vec![object::Object {
            filename: "testing".to_string(),
            object_name: "foo".to_string(),
            order: Some(23.90),
            values: fields::ObjectValues::from([("test".to_string(), FieldValue::String("Test".to_string()))])
        }]))]);
    });
    proto_test!(archival_proto::ValuePathComponent => object::ValuePathComponent, value_path_component_test {
        object::ValuePathComponent::Index(2);
        object::ValuePathComponent::Key("hello".to_string());
    });
    proto_test!(archival_proto::ValuePath => object::ValuePath, value_path_test {
        object::ValuePath::from_string("foo/bar/0/sdsdsd/22");
    });
    proto_test!(archival_proto::FieldType => fields::FieldType, field_type_test {
        fields::FieldType::Number;
        fields::FieldType::Enum(vec!["sjdklasd".to_string(), "blue".to_string()]);
        fields::FieldType::Alias(Box::new((fields::FieldType::Number, "numeric".to_string())));
        fields::FieldType::Oneof(vec![
            fields::OneofOption {
                name: "test".to_string(),
                r#type: fields::FieldType::Number,
            },
            fields::OneofOption {
                name: "test2".to_string(),
                r#type: fields::FieldType::String,
            }
        ]);
    });
    proto_test!(archival_proto::ObjectDefinition => ObjectDefinition, object_definition_test {
        ObjectDefinition {
            name: "object".to_string(),
            fields: FieldsMap::from([(
                "something".to_string(),
                FieldDefinition::new(fields::FieldType::Number, Some("How many.".to_string())),
            )]),
            template: None,
            children: ObjectDefinitions::from([
                ("child".to_string(), ObjectDefinition {
                    name: "object".to_string(),
                    fields: FieldsMap::from([
                        ("something".to_string(), fields::FieldType::Number.into())]
                    ),
                    template: None,
                    children: ObjectDefinitions::new(),
                    description: Some("A child object.".to_string()),
                })
            ]),
            description: None,
        };
    });
}
