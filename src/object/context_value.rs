//! The value tree a template renders against.
//!
//! Liquid's `Value` holds only liquid's own types, and an object renders as its
//! keys and values run together with no separator. A file field has to render
//! as its url while still answering `.url`, `.name` and the rest, which no
//! `Value` variant can do - so the context is built out of these, whose leaves
//! are `&dyn ValueView` and can therefore carry a [`RenderedFile`].
//!
//! [`ContextValue::to_value`] is the way back to a plain liquid value, and a
//! file converts to an object there: json consumers see every key, and only
//! rendering differs.

use crate::fields::RenderedFile;
use liquid::{
    model::{DisplayCow, KString, KStringCow, Object, ScalarCow, State, Value},
    ObjectView, ValueView,
};
use ordermap::OrderMap;
use std::fmt;

#[derive(Debug, Clone)]
pub enum ContextValue {
    /// Anything liquid can represent on its own.
    Liquid(Value),
    File(RenderedFile),
    Array(Vec<ContextValue>),
    Object(ContextObject),
}

impl ContextValue {
    pub fn nil() -> Self {
        Self::Liquid(Value::Nil)
    }
    pub fn array(values: impl IntoIterator<Item = ContextValue>) -> Self {
        Self::Array(values.into_iter().collect())
    }
    pub fn as_object(&self) -> Option<&ContextObject> {
        match self {
            Self::Object(o) => Some(o),
            _ => None,
        }
    }
}

impl From<Value> for ContextValue {
    fn from(value: Value) -> Self {
        Self::Liquid(value)
    }
}

impl From<ContextObject> for ContextValue {
    fn from(object: ContextObject) -> Self {
        Self::Object(object)
    }
}

impl ValueView for ContextValue {
    fn as_debug(&self) -> &dyn fmt::Debug {
        self
    }
    fn render(&self) -> DisplayCow<'_> {
        match self {
            Self::Liquid(v) => v.render(),
            Self::File(f) => f.render(),
            Self::Array(a) => a.render(),
            Self::Object(o) => o.render(),
        }
    }
    fn source(&self) -> DisplayCow<'_> {
        match self {
            Self::Liquid(v) => v.source(),
            Self::File(f) => f.source(),
            Self::Array(a) => a.source(),
            Self::Object(o) => o.source(),
        }
    }
    fn type_name(&self) -> &'static str {
        match self {
            Self::Liquid(v) => v.type_name(),
            Self::File(f) => f.type_name(),
            Self::Array(a) => a.type_name(),
            Self::Object(o) => o.type_name(),
        }
    }
    fn query_state(&self, state: State) -> bool {
        match self {
            Self::Liquid(v) => v.query_state(state),
            Self::File(f) => f.query_state(state),
            Self::Array(a) => a.query_state(state),
            Self::Object(o) => o.query_state(state),
        }
    }
    fn to_kstr(&self) -> KStringCow<'_> {
        match self {
            Self::Liquid(v) => v.to_kstr(),
            Self::File(f) => f.to_kstr(),
            Self::Array(a) => a.to_kstr(),
            Self::Object(o) => o.to_kstr(),
        }
    }
    fn as_scalar(&self) -> Option<ScalarCow<'_>> {
        match self {
            Self::Liquid(v) => v.as_scalar(),
            Self::File(f) => f.as_scalar(),
            Self::Array(_) | Self::Object(_) => None,
        }
    }
    fn is_scalar(&self) -> bool {
        self.as_scalar().is_some()
    }
    fn as_array(&self) -> Option<&dyn liquid::model::ArrayView> {
        match self {
            Self::Liquid(v) => v.as_array(),
            Self::Array(a) => Some(a),
            Self::File(_) | Self::Object(_) => None,
        }
    }
    fn as_object(&self) -> Option<&dyn ObjectView> {
        match self {
            Self::Liquid(v) => v.as_object(),
            Self::File(f) => f.as_object(),
            Self::Array(_) => None,
            Self::Object(o) => Some(o),
        }
    }
    fn as_state(&self) -> Option<State> {
        match self {
            Self::Liquid(v) => v.as_state(),
            _ => None,
        }
    }
    fn is_nil(&self) -> bool {
        matches!(self, Self::Liquid(Value::Nil))
    }
    fn to_value(&self) -> Value {
        match self {
            Self::Liquid(v) => v.clone(),
            Self::File(f) => f.to_value(),
            Self::Array(a) => Value::Array(a.iter().map(|v| v.to_value()).collect()),
            Self::Object(o) => o.to_value(),
        }
    }
}

/// An insertion-ordered map of context values. Ordered because a definition
/// declares its fields in an order and `{% for %}` over an object should follow
/// it; liquid's own object is a hash map.
#[derive(Debug, Clone, Default)]
pub struct ContextObject(OrderMap<KString, ContextValue>);

impl ContextObject {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn insert(&mut self, key: KString, value: ContextValue) -> Option<ContextValue> {
        self.0.insert(key, value)
    }
    pub fn get(&self, key: &str) -> Option<&ContextValue> {
        self.0.get(key)
    }
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn entries(&self) -> impl Iterator<Item = (&KString, &ContextValue)> {
        self.0.iter()
    }
    pub fn key_strs(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(|k| k.as_str())
    }
    pub fn extend(&mut self, other: impl IntoIterator<Item = (KString, ContextValue)>) {
        self.0.extend(other);
    }
    pub fn to_object(&self) -> Object {
        self.0
            .iter()
            .map(|(k, v)| (k.clone(), v.to_value()))
            .collect()
    }
}

impl FromIterator<(KString, ContextValue)> for ContextObject {
    fn from_iter<I: IntoIterator<Item = (KString, ContextValue)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// Liquid's object rendering: every key immediately followed by its value.
struct ObjectRender<'a>(&'a ContextObject);
impl fmt::Display for ObjectRender<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (key, value) in self.0.entries() {
            write!(f, "{}{}", key, value.render())?;
        }
        Ok(())
    }
}

struct ObjectSource<'a>(&'a ContextObject);
impl fmt::Display for ObjectSource<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        for (i, (key, value)) in self.0.entries().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "\"{}\": {}", key, value.source())?;
        }
        write!(f, "}}")
    }
}

impl ValueView for ContextObject {
    fn as_debug(&self) -> &dyn fmt::Debug {
        self
    }
    fn render(&self) -> DisplayCow<'_> {
        DisplayCow::Owned(Box::new(ObjectRender(self)))
    }
    fn source(&self) -> DisplayCow<'_> {
        DisplayCow::Owned(Box::new(ObjectSource(self)))
    }
    fn type_name(&self) -> &'static str {
        "object"
    }
    fn query_state(&self, state: State) -> bool {
        match state {
            State::Truthy => true,
            State::DefaultValue | State::Empty | State::Blank => self.is_empty(),
        }
    }
    fn to_kstr(&self) -> KStringCow<'_> {
        KStringCow::from_string(self.render().to_string())
    }
    fn to_value(&self) -> Value {
        Value::Object(self.to_object())
    }
    fn as_object(&self) -> Option<&dyn ObjectView> {
        Some(self)
    }
}

impl ObjectView for ContextObject {
    fn as_value(&self) -> &dyn ValueView {
        self
    }
    fn size(&self) -> i64 {
        self.0.len() as i64
    }
    fn keys<'k>(&'k self) -> Box<dyn Iterator<Item = KStringCow<'k>> + 'k> {
        Box::new(self.0.keys().map(|k| k.as_ref().into()))
    }
    fn values<'k>(&'k self) -> Box<dyn Iterator<Item = &'k dyn ValueView> + 'k> {
        Box::new(self.0.values().map(|v| v as &dyn ValueView))
    }
    fn iter<'k>(&'k self) -> Box<dyn Iterator<Item = (KStringCow<'k>, &'k dyn ValueView)> + 'k> {
        Box::new(
            self.0
                .iter()
                .map(|(k, v)| (k.as_ref().into(), v as &dyn ValueView)),
        )
    }
    fn contains_key(&self, index: &str) -> bool {
        self.0.contains_key(index)
    }
    fn get<'s>(&'s self, index: &str) -> Option<&'s dyn ValueView> {
        self.0.get(index).map(|v| v as &dyn ValueView)
    }
}
