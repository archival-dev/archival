//! Lets templates address an object in a list by its filename, so that
//! `menus.main` finds `objects/menus/main.toml`.
//!
//! liquid resolves only integers, `first`, `last` and `size` on an array, and
//! offers no hook for more, so [`NamedLookup`] wraps a runtime and retries a
//! lookup liquid could not resolve with a walk that also matches array items by
//! name. Retrying only on failure is what lets liquid's own indexes win when a
//! filename collides with one.

use crate::liquid_kstring::{KString, KStringCow, KStringRef};
use liquid_core::model::{try_find, ScalarCow, Value, ValueCow, ValueView};
use liquid_core::runtime::{PartialStore, Registers};
use liquid_core::{Result, Runtime};
use std::collections::BTreeSet;

pub(crate) struct NamedLookup<R> {
    inner: R,
}

impl<R: Runtime> NamedLookup<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self { inner }
    }

    fn find_named(&self, path: &[ScalarCow<'_>]) -> Option<ValueCow<'_>> {
        let (root, rest) = path.split_first()?;
        if rest.is_empty() {
            return None;
        }
        walk(self.inner.try_get(std::slice::from_ref(root))?, rest)
    }
}

fn walk<'v>(value: ValueCow<'v>, path: &[ScalarCow<'_>]) -> Option<ValueCow<'v>> {
    let Some((index, rest)) = path.split_first() else {
        return Some(value);
    };
    let child = match value {
        ValueCow::Borrowed(value) => index_named(value, index)?,
        ValueCow::Owned(value) => ValueCow::Owned(index_named(&value, index)?.into_owned()),
    };
    walk(child, rest)
}

fn index_named<'v>(value: &'v dyn ValueView, index: &ScalarCow<'_>) -> Option<ValueCow<'v>> {
    if let Some(child) = try_find(value, std::slice::from_ref(index)) {
        return Some(child);
    }
    let name = index.to_kstr();
    value
        .as_array()?
        .values()
        .find(|item| is_named(*item, name.as_str()))
        .map(ValueCow::Borrowed)
}

/// An object's `path` is `<object type>/<filename>`.
fn is_named(item: &dyn ValueView, name: &str) -> bool {
    item.as_object()
        .and_then(|object| object.get("path"))
        .and_then(|path| path.as_scalar())
        .is_some_and(|path| path.to_kstr().rsplit('/').next() == Some(name))
}

impl<R: Runtime> Runtime for NamedLookup<R> {
    fn partials(&self) -> &dyn PartialStore {
        self.inner.partials()
    }

    fn name(&self) -> Option<KStringRef<'_>> {
        self.inner.name()
    }

    fn roots(&self) -> BTreeSet<KStringCow<'_>> {
        self.inner.roots()
    }

    fn try_get(&self, path: &[ScalarCow<'_>]) -> Option<ValueCow<'_>> {
        self.inner.try_get(path).or_else(|| self.find_named(path))
    }

    fn get(&self, path: &[ScalarCow<'_>]) -> Result<ValueCow<'_>> {
        match self.inner.get(path) {
            Ok(value) => Ok(value),
            Err(err) => self.find_named(path).ok_or(err),
        }
    }

    fn set_global(&self, name: KString, val: Value) -> Option<Value> {
        self.inner.set_global(name, val)
    }

    fn set_index(&self, name: KString, val: Value) -> Option<Value> {
        self.inner.set_index(name, val)
    }

    fn get_index<'a>(&'a self, name: &str) -> Option<ValueCow<'a>> {
        self.inner.get_index(name)
    }

    fn registers(&self) -> &Registers {
        self.inner.registers()
    }
}

#[cfg(test)]
mod tests {
    use crate::liquid_parser;

    fn render(template: &str) -> Result<String, String> {
        let parser = liquid_parser::build_with_partials(Default::default()).unwrap();
        let globals = liquid::object!({
            "menus": [
                { "path": "menus/main", "title": "Main" },
                { "path": "menus/first", "title": "Named first" },
                { "path": "menus/size", "title": "Named size" },
                { "path": "menus/0", "title": "Named zero" },
            ],
            "objects": {
                "menus": [{ "path": "menus/footer", "title": "Footer" }],
            },
        });
        liquid_parser::parse(&parser, template)
            .and_then(|t| t.render(&globals))
            .map_err(|e| e.to_string())
    }

    #[test]
    fn list_items_are_addressable_by_name() {
        assert_eq!(render("{{ menus.main.title }}").unwrap(), "Main");
        assert_eq!(
            render("{{ objects.menus.footer.title }}").unwrap(),
            "Footer"
        );
        assert_eq!(
            render("{% assign n = 'main' %}{{ menus[n].title }}").unwrap(),
            "Main"
        );
        assert_eq!(render("{% if menus.main %}yes{% endif %}").unwrap(), "yes");
    }

    #[test]
    fn builtin_indexes_win_over_names() {
        assert_eq!(
            render(
                "{{ menus.first.title }}|{{ menus.size }}|{{ menus[0].title }}|{{ menus[\"0\"].title }}"
            )
            .unwrap(),
            "Main|4|Main|Main"
        );
    }

    #[test]
    fn names_survive_filters_and_assignment() {
        assert_eq!(
            render("{% assign r = menus | reverse %}{{ r.main.title }}").unwrap(),
            "Main"
        );
    }

    #[test]
    fn unknown_names_still_error() {
        let err = render("{{ menus.nope.title }}").unwrap_err();
        assert!(err.contains("Unknown index"), "unexpected error: {err}");
    }
}
