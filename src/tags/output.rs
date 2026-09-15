//! Archival's replacement for liquid's `{{ … }}` output statement.
//!
//! Object field values may themselves contain liquid. Rendering the page and
//! then re-rendering the whole output (the approach archival used to take)
//! loses everything the template built along the way — `{% assign %}`
//! variables, `{% for %}` loop variables, `forloop`, `{% layout %}` passthrough
//! vars — because the second render starts from a fresh runtime.
//!
//! Instead, `crate::liquid_rewrite` rewrites every output statement into
//! [`OUTPUT_TAG`], and this tag renders liquid found in a value *in place*,
//! against the runtime that is already rendering the page. Locals are visible
//! because it is literally the same runtime.

use crate::liquid_rewrite::{rewrite_template, OUTPUT_TAG};
use liquid_core::error::{ResultLiquidExt, ResultLiquidReplaceExt};
use liquid_core::parser::FilterChain;
use liquid_core::runtime;
use liquid_core::Language;
use liquid_core::{Error, Result};
use liquid_core::{ParseTag, Renderable, Runtime, TagReflection, TagTokenIter};
use seahash::SeaHasher;
use std::collections::HashMap;
use std::hash::Hasher;
use std::io::Write;
use std::sync::{Arc, OnceLock, RwLock, Weak};

/// How many times a value's liquid may itself produce more liquid. One nested
/// render matches the guarantee archival made when it re-rendered the whole
/// page once, and keeps rebuilds fast; beyond it, values are written out
/// literally rather than rendered, so a self-referential field terminates
/// instead of erroring.
const MAX_VALUE_RENDER_DEPTH: usize = 1;

/// Field values repeat across pages (a body rendered both on its own page and
/// in an index), so parsed values are memoized. Bounded like the site's
/// template cache.
const NESTED_CACHE_MAX_ENTRIES: usize = 256;

/// Truncation applied to a value before it is quoted in an error message.
const SNIPPET_LEN: usize = 120;

/// State shared between the tag and the parser that owns it.
#[derive(Default)]
pub(crate) struct OutputContext {
    /// The `Language` values are parsed with, captured while the parser is
    /// built (see `liquid_parser::build_with_partials`).
    ///
    /// Weak on purpose: the `Language` owns this tag, which owns this cell, so
    /// holding an `Arc` would leak one `Language` per parser rebuild — and the
    /// dev server rebuilds the parser every time a partial changes. The
    /// `Language` is kept alive by the `liquid_parser::Parser`, which always outlives
    /// rendering of the templates it parsed.
    language: OnceLock<Weak<Language>>,
    nested: RwLock<HashMap<u64, Arc<runtime::Template>>>,
}

impl std::fmt::Debug for OutputContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputContext")
            .field("language", &self.language.get().is_some())
            .field(
                "nested",
                &self.nested.read().map(|n| n.len()).unwrap_or_default(),
            )
            .finish()
    }
}

impl OutputContext {
    pub(crate) fn set_language(&self, language: &Arc<Language>) {
        let _ = self.language.set(Arc::downgrade(language));
    }

    fn language(&self) -> Result<Arc<Language>> {
        self.language
            .get()
            .and_then(|l| l.upgrade())
            .ok_or_else(|| {
                Error::with_msg("internal: liquid language unavailable (was the parser dropped?)")
            })
    }

    /// Parses liquid found inside a value, reusing a previously parsed copy of
    /// the same value.
    fn nested_template(&self, value: &str) -> Result<Arc<runtime::Template>> {
        let mut hasher = SeaHasher::new();
        hasher.write(value.as_bytes());
        let key = hasher.finish();
        if let Some(template) = self.nested.read().unwrap().get(&key) {
            return Ok(template.clone());
        }
        let language = self.language()?;
        let _span = tracing::trace_span!("nested_parse").entered();
        let template = Arc::new(runtime::Template::new(liquid_core::parser::parse(
            &rewrite_template(value),
            &language,
        )?));
        let mut nested = self.nested.write().unwrap();
        if nested.len() >= NESTED_CACHE_MAX_ENTRIES {
            nested.clear();
        }
        nested.insert(key, template.clone());
        Ok(template)
    }

    #[cfg(test)]
    pub(crate) fn nested_len(&self) -> usize {
        self.nested.read().unwrap().len()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OutputTag {
    ctx: Arc<OutputContext>,
}

impl OutputTag {
    pub(crate) fn new(ctx: Arc<OutputContext>) -> Self {
        Self { ctx }
    }
}

impl TagReflection for OutputTag {
    fn tag(&self) -> &'static str {
        OUTPUT_TAG
    }

    fn description(&self) -> &'static str {
        "Internal: an output statement that also renders liquid contained in the value"
    }
}

impl ParseTag for OutputTag {
    fn parse(
        &self,
        mut arguments: TagTokenIter<'_>,
        options: &Language,
    ) -> Result<Box<dyn Renderable>> {
        let chain = arguments
            .expect_next("Expected a value.")?
            .expect_filter_chain(options)
            .into_result()?;
        arguments.expect_nothing()?;
        Ok(Box::new(OutputStatement {
            chain,
            ctx: self.ctx.clone(),
        }))
    }

    fn reflection(&self) -> &dyn TagReflection {
        self
    }
}

#[derive(Debug)]
struct OutputStatement {
    chain: FilterChain,
    ctx: Arc<OutputContext>,
}

impl Renderable for OutputStatement {
    fn render_to(&self, writer: &mut dyn Write, runtime: &dyn Runtime) -> Result<()> {
        let value = self.chain.evaluate(runtime)?;
        let view = value.as_view();
        // Only strings can carry liquid, and `type_name` is a `&'static str`
        // comparison, so numbers, dates, arrays and objects take the same path
        // they would in stock liquid.
        if view.type_name() == "string" {
            let text = view.to_kstr();
            if depth(runtime) < MAX_VALUE_RENDER_DEPTH && may_contain_liquid(&text) {
                return self.render_value(&text, writer, runtime);
            }
            writer
                .write_all(text.as_bytes())
                .replace("Failed to render")?;
        } else {
            write!(writer, "{}", view.render()).replace("Failed to render")?;
        }
        Ok(())
    }
}

impl OutputStatement {
    /// Renders liquid contained in a value against `runtime` — the same runtime
    /// rendering the enclosing page, which is what makes template-local
    /// variables visible.
    fn render_value(
        &self,
        text: &str,
        writer: &mut dyn Write,
        runtime: &dyn Runtime,
    ) -> Result<()> {
        let _depth = DepthGuard::enter(runtime);
        let template = self
            .ctx
            .nested_template(text)
            .trace_with(|| self.trace().into())
            .context_key("value")
            .value_with(|| snippet(text).into())?;
        template
            .render_to(writer, runtime)
            .trace_with(|| self.trace().into())
    }

    /// The output statement as the author wrote it, for error traces.
    /// `FilterChain`'s `Display` always appends a `|` separator, even when
    /// there are no filters, so an unfiltered chain needs it trimmed back off.
    fn trace(&self) -> String {
        let chain = self.chain.to_string();
        format!(
            "{{{{ {} }}}}",
            chain.trim_end().trim_end_matches('|').trim_end()
        )
    }
}

/// Every rendered string is checked, most of them large, so this makes a single
/// `memchr`-backed pass over the value rather than one per delimiter.
fn may_contain_liquid(value: &str) -> bool {
    let bytes = value.as_bytes();
    value
        .match_indices('{')
        .any(|(i, _)| matches!(bytes.get(i + 1), Some(b'{' | b'%')))
}

fn snippet(value: &str) -> String {
    match value.char_indices().nth(SNIPPET_LEN) {
        Some((end, _)) => format!("{}…", &value[..end]),
        None => value.to_owned(),
    }
}

/// How many values deep the current render is. Lives in the runtime's registers
/// so it is scoped to a single page render rather than to the thread.
#[derive(Default)]
struct OutputDepth(usize);

fn depth(runtime: &dyn Runtime) -> usize {
    runtime.registers().get_mut::<OutputDepth>().0
}

struct DepthGuard<'a> {
    runtime: &'a dyn Runtime,
}

impl<'a> DepthGuard<'a> {
    /// The borrow of the registers must not outlive this call: `Registers` is a
    /// single `RefCell`, and `runtime::Template::render_to` takes it after
    /// every element to check for interrupts.
    fn enter(runtime: &'a dyn Runtime) -> Self {
        runtime.registers().get_mut::<OutputDepth>().0 += 1;
        Self { runtime }
    }
}

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.runtime.registers().get_mut::<OutputDepth>().0 -= 1;
    }
}

#[cfg(test)]
mod tests {
    use crate::liquid_parser;

    fn render(template: &str, globals: &liquid::Object) -> Result<String, String> {
        let parser = liquid_parser::build_with_partials(Default::default()).unwrap();
        liquid_parser::parse(&parser, template)
            .and_then(|t| t.render(globals))
            .map_err(|e| e.to_string())
    }

    #[test]
    fn values_without_liquid_render_like_stock_output() {
        let globals = liquid::object!({
            "s": "plain", "n": 42, "f": 1.5, "b": true, "list": ["a", "b"],
            "obj": { "k": "v" },
        });
        for expression in ["s", "n", "f", "b", "list", "obj", "missing", "s | upcase"] {
            let parser = liquid_parser::build_with_partials(Default::default()).unwrap();
            let stock = parser
                .parse(&format!("{{{{ {expression} }}}}"))
                .unwrap()
                .render(&globals);
            let ours = render(&format!("{{{{ {expression} }}}}"), &globals);
            assert_eq!(
                stock.map_err(|e| e.to_string()),
                ours,
                "output differs for {expression}"
            );
        }
    }

    #[test]
    fn liquid_in_a_value_is_rendered() {
        let globals = liquid::object!({ "body": "hello {{ name }}", "name": "world" });
        assert_eq!(render("{{ body }}", &globals).unwrap(), "hello world");
    }

    /// Values are rewritten on the way in too, so they accept the same syntax a
    /// template does.
    #[test]
    fn a_value_may_use_the_liquid_tag() {
        let globals = liquid::object!({
            "body": "{% liquid\n  assign who = name | upcase\n  echo who\n%}",
            "name": "world",
        });
        assert_eq!(render("{{ body }}", &globals).unwrap(), "WORLD");
    }

    #[test]
    fn tags_in_a_value_are_rendered() {
        let globals =
            liquid::object!({ "body": "{% if name %}yes{% else %}no{% endif %}", "name": "x" });
        assert_eq!(render("{{ body }}", &globals).unwrap(), "yes");
    }

    #[test]
    fn locals_are_visible_to_values() {
        let globals = liquid::object!({ "body": "by {{ author }}" });
        assert_eq!(
            render("{% assign author = 'Ada' %}{{ body }}", &globals).unwrap(),
            "by Ada"
        );
    }

    #[test]
    fn loop_variables_are_visible_to_values() {
        let globals = liquid::object!({
            "posts": [
                { "title": "One", "body": "{{ p.title }} #{{ forloop.index }}" },
                { "title": "Two", "body": "{{ p.title }} #{{ forloop.index }}" },
            ]
        });
        assert_eq!(
            render("{% for p in posts %}{{ p.body }};{% endfor %}", &globals).unwrap(),
            "One #1;Two #2;"
        );
    }

    #[test]
    fn raw_inside_a_value_is_literal() {
        let globals = liquid::object!({ "body": "{% raw %}{{ name }}{% endraw %}", "name": "x" });
        assert_eq!(render("{{ body }}", &globals).unwrap(), "{{ name }}");
    }

    #[test]
    fn nesting_stops_at_the_depth_limit() {
        let globals = liquid::object!({ "a": "[{{ b }}]", "b": "[{{ c }}]", "c": "deep" });
        // One nested render: `a`'s liquid resolves, but liquid `b` contributes
        // is written out literally.
        assert_eq!(render("{{ a }}", &globals).unwrap(), "[[{{ c }}]]");
    }

    #[test]
    fn self_referential_values_terminate() {
        let globals = liquid::object!({ "a": "loop {{ a }}" });
        assert_eq!(render("{{ a }}", &globals).unwrap(), "loop loop {{ a }}");
    }

    #[test]
    fn broken_liquid_in_a_value_reports_the_output_statement() {
        let globals = liquid::object!({ "body": "{% if %}" });
        let error = render("{{ body }}", &globals).unwrap_err();
        assert!(error.contains("{{ body }}"), "missing trace: {error}");
        assert!(error.contains("{% if %}"), "missing value: {error}");
    }

    #[test]
    fn repeated_values_are_parsed_once() {
        let globals = liquid::object!({ "body": "hi {{ name }}", "name": "x" });
        let (parser, ctx) = liquid_parser::build_with_output_context(Default::default()).unwrap();
        let template = liquid_parser::parse(&parser, "{{ body }}{{ body }}").unwrap();
        assert_eq!(template.render(&globals).unwrap(), "hi xhi x");
        assert_eq!(ctx.nested_len(), 1);
    }
}
