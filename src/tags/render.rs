use crate::liquid_kstring::{KString, KStringCow, KStringRef};
use crate::named_lookup::NamedLookup;
use crate::tags::args::{binding_name, parse_binding, parse_vars_from, Binding};
use liquid_core::error::ResultLiquidExt;
use liquid_core::model::{
    find, try_find, DisplayCow, Object, ObjectView, ScalarCow, State, Value, ValueCow, ValueView,
};
use liquid_core::runtime::{Interrupt, InterruptRegister, PartialStore, Registers};
use liquid_core::Expression;
use liquid_core::Language;
use liquid_core::Renderable;
use liquid_core::Runtime;
use liquid_core::{Error, Result};
use liquid_core::{ParseTag, TagReflection, TagTokenIter};
use liquid_lib::stdlib::ForloopObject;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::io::Write;

/// The key [`RenderContext`] answers with itself. `render` renders a partial in
/// an isolated scope, so it cannot reach the render context the way `include`
/// does; it looks the context up under this key instead. Contexts that are not
/// wrapped in [`RenderContext`] render partials with no globals at all.
pub(crate) const GLOBALS_KEY: &str = "__archival_globals";

/// Wraps a render context so `{% render %}` can find it. Answers
/// [`GLOBALS_KEY`] with itself, so a partial that renders another partial still
/// reaches the globals, and hides the key from iteration so it never shows up
/// in `roots()`, `for` loops or debug output.
pub(crate) struct RenderContext<'a> {
    inner: &'a dyn ObjectView,
}

impl<'a> RenderContext<'a> {
    pub(crate) fn new(inner: &'a dyn ObjectView) -> Self {
        Self { inner }
    }
}

impl fmt::Debug for RenderContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.as_debug().fmt(f)
    }
}

impl ValueView for RenderContext<'_> {
    fn as_debug(&self) -> &dyn fmt::Debug {
        self
    }
    fn render(&self) -> DisplayCow<'_> {
        self.inner.render()
    }
    fn source(&self) -> DisplayCow<'_> {
        self.inner.source()
    }
    fn type_name(&self) -> &'static str {
        self.inner.type_name()
    }
    fn query_state(&self, state: State) -> bool {
        self.inner.query_state(state)
    }
    fn to_kstr(&self) -> KStringCow<'_> {
        self.inner.to_kstr()
    }
    fn to_value(&self) -> Value {
        self.inner.to_value()
    }
    fn as_object(&self) -> Option<&dyn ObjectView> {
        Some(self)
    }
}

impl ObjectView for RenderContext<'_> {
    fn as_value(&self) -> &dyn ValueView {
        self
    }
    fn size(&self) -> i64 {
        self.inner.size()
    }
    fn keys<'k>(&'k self) -> Box<dyn Iterator<Item = KStringCow<'k>> + 'k> {
        self.inner.keys()
    }
    fn values<'k>(&'k self) -> Box<dyn Iterator<Item = &'k dyn ValueView> + 'k> {
        self.inner.values()
    }
    fn iter<'k>(&'k self) -> Box<dyn Iterator<Item = (KStringCow<'k>, &'k dyn ValueView)> + 'k> {
        self.inner.iter()
    }
    fn contains_key(&self, index: &str) -> bool {
        index == GLOBALS_KEY || self.inner.contains_key(index)
    }
    fn get<'s>(&'s self, index: &str) -> Option<&'s dyn ValueView> {
        if index == GLOBALS_KEY {
            Some(self.as_value())
        } else {
            self.inner.get(index)
        }
    }
}

/// The scope a partial is rendered in. Unlike `include`'s [`StackFrame`], the
/// parent's variables are not visible: a partial sees only what was passed to
/// it plus the site globals, and its own assignments do not escape.
///
/// [`StackFrame`]: liquid_core::runtime::StackFrame
struct RenderFrame<'a> {
    parent: &'a dyn Runtime,
    globals: Option<ValueCow<'a>>,
    passed: HashMap<KStringRef<'a>, ValueCow<'a>>,
    assigned: RefCell<Object>,
}

impl<'a> RenderFrame<'a> {
    fn new(
        parent: &'a dyn Runtime,
        globals: Option<ValueCow<'a>>,
        passed: HashMap<KStringRef<'a>, ValueCow<'a>>,
    ) -> Self {
        Self {
            parent,
            globals,
            passed,
            assigned: RefCell::new(Object::new()),
        }
    }

    fn globals(&self) -> Option<&dyn ValueView> {
        self.globals.as_ref().map(|g| g.as_view())
    }
}

impl Runtime for RenderFrame<'_> {
    fn partials(&self) -> &dyn PartialStore {
        self.parent.partials()
    }

    fn name(&self) -> Option<KStringRef<'_>> {
        self.parent.name()
    }

    fn roots(&self) -> BTreeSet<KStringCow<'_>> {
        let mut roots = BTreeSet::new();
        roots.extend(self.passed.keys().map(|k| k.to_owned().into()));
        roots.extend(self.assigned.borrow().keys().map(|k| k.clone().into()));
        if let Some(globals) = self.globals().and_then(|g| g.as_object()) {
            roots.extend(globals.keys().map(|k| k.into_owned().into()));
        }
        roots
    }

    fn try_get(&self, path: &[ScalarCow<'_>]) -> Option<ValueCow<'_>> {
        let key = path.first()?.to_kstr();
        {
            let assigned = self.assigned.borrow();
            if assigned.contains_key(key.as_str()) {
                return try_find(assigned.as_value(), path).map(|v| v.into_owned().into());
            }
        }
        if ObjectView::contains_key(&self.passed, key.as_str()) {
            return try_find(self.passed.as_value(), path);
        }
        self.globals().and_then(|globals| try_find(globals, path))
    }

    fn get(&self, path: &[ScalarCow<'_>]) -> Result<ValueCow<'_>> {
        let key = path
            .first()
            .ok_or_else(|| {
                Error::with_msg("Unknown variable").context("requested variable", "nil")
            })?
            .to_kstr();
        {
            let assigned = self.assigned.borrow();
            if assigned.contains_key(key.as_str()) {
                return find(assigned.as_value(), path).map(|v| v.into_owned().into());
            }
        }
        if ObjectView::contains_key(&self.passed, key.as_str()) {
            return find(self.passed.as_value(), path);
        }
        match self.globals() {
            Some(globals) if globals.as_object().is_some_and(|g| g.contains_key(&key)) => {
                find(globals, path)
            }
            _ => Err(Error::with_msg("Unknown variable")
                .context("requested variable", key.into_owned())
                .context(
                    "note",
                    "`render` only sees variables passed to it and site globals; use `include` to share the caller's scope",
                )),
        }
    }

    fn set_global(&self, name: KString, val: Value) -> Option<Value> {
        self.assigned.borrow_mut().insert(name, val)
    }

    fn set_index(&self, name: KString, val: Value) -> Option<Value> {
        self.parent.set_index(name, val)
    }

    fn get_index<'i>(&'i self, name: &str) -> Option<ValueCow<'i>> {
        self.parent.get_index(name)
    }

    /// Shared with the caller, unlike liquid's `SandboxedStackFrame`, so that
    /// `output`'s render-depth guard still sees the depth it was entered with.
    fn registers(&self) -> &Registers {
        self.parent.registers()
    }
}

/// Replaces `liquid_lib::stdlib::RenderTag`, which renders partials in a scope
/// so isolated that a site's own objects are unreachable, and which rejects the
/// whitespace-separated argument form archival's other tags accept.
#[derive(Copy, Clone, Debug, Default)]
pub struct RenderTag;

impl TagReflection for RenderTag {
    fn tag(&self) -> &'static str {
        "render"
    }

    fn description(&self) -> &'static str {
        "Renders a partial in an isolated scope"
    }
}

impl ParseTag for RenderTag {
    fn parse(
        &self,
        mut arguments: TagTokenIter<'_>,
        _options: &Language,
    ) -> Result<Box<dyn Renderable>> {
        let partial = arguments
            .expect_next("Identifier or literal expected.")?
            .expect_value()
            .into_result()?;

        let first = arguments.next();
        let (binding, binding_name, token) = parse_binding(first, &mut arguments)?;
        let vars = parse_vars_from(token, &mut arguments)?;

        Ok(Box::new(Render {
            partial,
            binding,
            binding_name,
            vars,
        }))
    }

    fn reflection(&self) -> &dyn TagReflection {
        self
    }
}

#[derive(Debug)]
struct Render {
    partial: Expression,
    binding: Option<Binding>,
    /// Name a `with`/`for` value is bound to. `None` means the `as` clause was
    /// omitted, and the partial's own name is used.
    binding_name: Option<KString>,
    vars: Vec<(KString, Expression)>,
}

impl Render {
    fn trace(&self) -> KString {
        format!("{{% render {} %}}", self.partial).into()
    }

    /// Arguments are evaluated against the caller, then handed to the isolated
    /// scope; only their values cross the boundary.
    fn passed<'a>(
        &'a self,
        runtime: &'a dyn Runtime,
    ) -> Result<HashMap<KStringRef<'a>, ValueCow<'a>>> {
        let mut passed = HashMap::new();
        for (id, val) in &self.vars {
            let value = val
                .try_evaluate(runtime)
                .ok_or_else(|| Error::with_msg("failed to evaluate value"))?;
            passed.insert(id.as_ref(), value);
        }
        Ok(passed)
    }

    fn render_partial(
        &self,
        writer: &mut dyn Write,
        scope: &RenderFrame<'_>,
        name: &str,
    ) -> Result<()> {
        let partial = scope.partials().get(name).trace_with(|| self.trace())?;

        partial
            .render_to(writer, &NamedLookup::new(scope))
            .trace_with(|| self.trace())
            .context_key_with(|| self.partial.to_string().into())
            .value_with(|| name.to_string().into())
    }
}

impl Renderable for Render {
    fn render_to(&self, writer: &mut dyn Write, runtime: &dyn Runtime) -> Result<()> {
        let value = self.partial.evaluate(runtime)?;
        if !value.is_scalar() {
            return Error::with_msg("Can only `render` strings")
                .context("partial", format!("{}", value.source()))
                .into_err();
        }
        let name = value.to_kstr().into_owned();
        let globals = runtime.try_get(&[ScalarCow::new(GLOBALS_KEY)]);
        let bound = KStringRef::from_ref(binding_name(self.binding_name.as_ref(), &name));

        match &self.binding {
            Some(Binding::For(range)) => {
                let range = range.evaluate(runtime).trace_with(|| self.trace())?;
                let items = range.evaluate()?;
                let len = items.len();

                for (i, item) in items.into_iter().enumerate() {
                    let forloop = ForloopObject::new(i, len);
                    let mut passed = self.passed(runtime)?;
                    passed.insert(
                        KStringRef::from_ref("forloop"),
                        ValueCow::Borrowed(&forloop),
                    );
                    passed.insert(bound, item);
                    let scope = RenderFrame::new(runtime, globals.clone(), passed);

                    self.render_partial(writer, &scope, &name)
                        .context_key("index")
                        .value_with(|| format!("{}", i + 1).into())?;

                    // The innermost loop owns an interrupt, so consume it here
                    // rather than letting it escape to the calling template.
                    let interrupt = scope.registers().get_mut::<InterruptRegister>().reset();
                    let broke = matches!(interrupt, Some(Interrupt::Break));
                    if broke {
                        break;
                    }
                }
                Ok(())
            }
            binding => {
                let mut passed = self.passed(runtime)?;
                if let Some(Binding::With(value)) = binding {
                    passed.insert(
                        bound,
                        value
                            .try_evaluate(runtime)
                            .ok_or_else(|| Error::with_msg("failed to evaluate value"))?,
                    );
                }
                let scope = RenderFrame::new(runtime, globals, passed);
                self.render_partial(writer, &scope, &name)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::tags::include::IncludeTag;
    use liquid_core::partials::{EagerCompiler, PartialSource};
    use std::borrow::Cow;

    #[derive(Default, Debug, Clone, Copy)]
    struct TestSource;

    impl PartialSource for TestSource {
        fn contains(&self, _name: &str) -> bool {
            true
        }

        fn names(&self) -> Vec<&str> {
            vec![
                "vars",
                "plain",
                "global",
                "local",
                "assigns",
                "card",
                "dir/nested",
                "loop",
                "breaker",
                "outer",
                "menu",
            ]
        }

        fn try_get<'a>(&'a self, name: &str) -> Option<Cow<'a, str>> {
            match name {
                "vars" => Some("[{{a}}|{{b}}]".into()),
                "plain" => Some("plain".into()),
                "global" => Some("{{site_url}}".into()),
                "local" => Some("{{local}}".into()),
                "assigns" => Some("{% assign leaked = 'inner' %}{{leaked}}".into()),
                "card" => Some("{{card.name}}".into()),
                "dir/nested" => Some("{{nested.name}}".into()),
                "loop" => Some("{{forloop.index}}:{{item}} ".into()),
                "breaker" => Some("{{item}}{% if item == 2 %}{% break %}{% endif %}".into()),
                "outer" => Some("{% render 'global' %}".into()),
                "menu" => Some("{{menus.main.title}}".into()),
                _ => None,
            }
        }
    }

    fn globals() -> liquid::Object {
        liquid::object!({
            "site_url": "https://example.com",
            "items": [1, 2, 3],
            "product": { "name": "widget" },
            "menus": [{ "path": "menus/main", "title": "Main" }],
        })
    }

    fn render_with(tag_render: bool, template: &str, globals: &liquid::Object) -> Result<String> {
        let builder = liquid::ParserBuilder::with_stdlib().partials(EagerCompiler::new(TestSource));
        let parser = if tag_render {
            builder.tag(RenderTag)
        } else {
            builder.tag(IncludeTag)
        }
        .build()
        .unwrap();
        parser
            .parse(template)?
            .render(&RenderContext::new(globals as &dyn ObjectView))
    }

    fn render(template: &str) -> Result<String> {
        render_with(true, template, &globals())
    }

    #[test]
    fn globals_are_visible() {
        assert_eq!(
            render("{% render 'global' %}").unwrap(),
            "https://example.com"
        );
    }

    #[test]
    fn nested_render_still_sees_globals() {
        assert_eq!(
            render("{% render 'outer' %}").unwrap(),
            "https://example.com"
        );
    }

    #[test]
    fn globals_are_addressable_by_name() {
        assert_eq!(render("{% render 'menu' %}").unwrap(), "Main");
    }

    #[test]
    fn caller_locals_are_not_visible() {
        let err = render("{% assign local = 'outer' %}{% render 'local' %}").unwrap_err();
        assert!(
            err.to_string().contains("Unknown variable"),
            "expected an unknown variable error, got: {err}"
        );
    }

    #[test]
    fn assignments_do_not_leak_to_the_caller() {
        assert_eq!(
            render("{% render 'assigns' %}|{% if leaked %}{{leaked}}{% else %}none{% endif %}")
                .unwrap(),
            "inner|none"
        );
    }

    #[test]
    fn with_as_binds_a_name() {
        assert_eq!(
            render("{% render 'vars' with 1 as a, b: 2 %}").unwrap(),
            "[1|2]"
        );
    }

    #[test]
    fn with_defaults_to_the_partial_name() {
        assert_eq!(
            render("{% render 'card' with product %}").unwrap(),
            "widget"
        );
    }

    #[test]
    fn with_default_name_ignores_the_partial_directory() {
        assert_eq!(
            render("{% render 'dir/nested' with product %}").unwrap(),
            "widget"
        );
    }

    #[test]
    fn for_as_iterates_with_a_forloop() {
        assert_eq!(
            render("{% render 'loop' for items as item %}").unwrap(),
            "1:1 2:2 3:3 "
        );
    }

    #[test]
    fn for_supports_break() {
        assert_eq!(
            render("{% render 'breaker' for items as item %}").unwrap(),
            "12"
        );
    }

    #[test]
    fn for_takes_extra_args() {
        assert_eq!(
            render("{% render 'vars' for items as a, b: 9 %}").unwrap(),
            "[1|9][2|9][3|9]"
        );
    }

    #[test]
    fn no_globals_context_renders_without_them() {
        let parser = liquid::ParserBuilder::with_stdlib()
            .tag(RenderTag)
            .partials(EagerCompiler::new(TestSource))
            .build()
            .unwrap();
        let out = parser
            .parse("{% render 'global' %}")
            .unwrap()
            .render(&liquid::object!({ "site_url": "https://example.com" }));
        assert!(out.is_err(), "unwrapped context should not leak globals");
    }

    /// The argument forms `include` and `render` share are parsed by the same
    /// code (`crate::tags::args`), so they have to agree.
    #[test]
    fn argument_forms_match_include() {
        let cases = [
            "{% TAG 'plain' %}",
            "{% TAG 'vars' a: 1, b: 2 %}",
            "{% TAG 'vars', a: 1, b: 2 %}",
            "{% TAG 'vars', a: 1, b: 2, %}",
            "{% TAG 'vars' a: 1 b: 2 %}",
            "{% TAG 'nope' %}",
        ];
        let globals = globals();
        for case in cases {
            let rendered = render_with(true, &case.replace("TAG", "render"), &globals);
            let included = render_with(false, &case.replace("TAG", "include"), &globals);
            assert_eq!(
                rendered.is_ok(),
                included.is_ok(),
                "render and include disagreed on `{case}`: {rendered:?} vs {included:?}"
            );
            if let (Ok(rendered), Ok(included)) = (rendered, included) {
                assert_eq!(rendered, included, "output differed for `{case}`");
            }
        }
    }
}
