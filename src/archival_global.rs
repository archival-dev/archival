//! The `archival` global: what Archival knows about a site that its objects do
//! not hold, handed to a build as snapshots. Each section answers in the shape
//! the matching carrier call does, with keys in snake_case.

use crate::{liquid_kstring::KStringCow, payments::PaymentsSnapshot, social::SocialSnapshot};
use liquid::{ObjectView, ValueView};
use liquid_core::Value;
use serde::Serialize;
use std::fmt;

/// The global the snapshots render under.
pub const ARCHIVAL_GLOBAL: &str = "archival";

pub(crate) fn liquid_value(value: &impl Serialize) -> Value {
    liquid::model::to_value(value).expect("snapshots are plain data")
}

pub(crate) fn liquid_object(value: &impl Serialize) -> liquid::Object {
    match liquid_value(value) {
        Value::Object(object) => object,
        _ => liquid::Object::new(),
    }
}

/// An object in the `archival` global. Keys resolve to a child view, then to a
/// plain value, then to `fallback`, which is how an unknown key reads as the
/// carrier's answer for it instead of as a missing value.
#[derive(Debug, Default)]
pub(crate) struct ArchivalView {
    pub(crate) children: Vec<(&'static str, ArchivalView)>,
    pub(crate) fields: liquid::Object,
    pub(crate) fallback: Option<Value>,
}

impl ArchivalView {
    pub(crate) fn new(
        social: Option<&SocialSnapshot>,
        payments: Option<&PaymentsSnapshot>,
    ) -> Self {
        let mut view = crate::social::view(social);
        view.children
            .push((crate::payments::SECTION, crate::payments::view(payments)));
        view
    }

    pub(crate) fn keyed(entries: liquid::Object, fallback: Value) -> Self {
        Self {
            fields: entries,
            fallback: Some(fallback),
            ..Default::default()
        }
    }

    fn child(&self, index: &str) -> Option<&ArchivalView> {
        self.children
            .iter()
            .find(|(key, _)| *key == index)
            .map(|(_, view)| view)
    }

    fn materialize(&self) -> liquid::Object {
        let mut object = self.fields.clone();
        for (key, child) in &self.children {
            object.insert((*key).into(), Value::Object(child.materialize()));
        }
        object
    }
}

impl fmt::Display for ArchivalView {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for (k, v) in ObjectView::iter(self) {
            write!(f, "{}: {} ", k, v.render())?;
        }
        Ok(())
    }
}

impl ValueView for ArchivalView {
    fn as_debug(&self) -> &dyn fmt::Debug {
        self
    }
    fn render(&self) -> liquid::model::DisplayCow<'_> {
        liquid::model::DisplayCow::Owned(Box::new(self))
    }
    fn source(&self) -> liquid::model::DisplayCow<'_> {
        liquid::model::DisplayCow::Owned(Box::new(self))
    }
    fn type_name(&self) -> &'static str {
        "object"
    }
    fn query_state(&self, state: liquid::model::State) -> bool {
        match state {
            liquid::model::State::Truthy => true,
            liquid::model::State::DefaultValue
            | liquid::model::State::Empty
            | liquid::model::State::Blank => ObjectView::size(self) == 0,
        }
    }
    fn to_kstr(&self) -> KStringCow<'_> {
        KStringCow::from_string(self.to_string())
    }
    fn to_value(&self) -> Value {
        Value::Object(self.materialize())
    }
    fn as_object(&self) -> Option<&dyn ObjectView> {
        Some(self)
    }
}

impl ObjectView for ArchivalView {
    fn as_value(&self) -> &dyn ValueView {
        self
    }
    fn size(&self) -> i64 {
        (self.children.len() + self.fields.len()) as i64
    }
    fn keys<'k>(&'k self) -> Box<dyn Iterator<Item = KStringCow<'k>> + 'k> {
        Box::new(
            self.children
                .iter()
                .map(|(key, _)| KStringCow::from_static(key))
                .chain(self.fields.keys().map(|k| k.as_ref().into())),
        )
    }
    fn values<'k>(&'k self) -> Box<dyn Iterator<Item = &'k dyn ValueView> + 'k> {
        Box::new(ObjectView::iter(self).map(|(_, v)| v))
    }
    fn iter<'k>(&'k self) -> Box<dyn Iterator<Item = (KStringCow<'k>, &'k dyn ValueView)> + 'k> {
        Box::new(
            self.children
                .iter()
                .map(|(key, view)| (KStringCow::from_static(key), view as &dyn ValueView))
                .chain(
                    self.fields
                        .iter()
                        .map(|(k, v)| (k.as_ref().into(), v.as_view())),
                ),
        )
    }
    fn contains_key(&self, index: &str) -> bool {
        self.child(index).is_some() || self.fields.contains_key(index)
    }
    fn get<'s>(&'s self, index: &str) -> Option<&'s dyn ValueView> {
        if let Some(child) = self.child(index) {
            return Some(child);
        }
        self.fields
            .get(index)
            .or(self.fallback.as_ref())
            .map(|v| v.as_view())
    }
}
