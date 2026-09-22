use crate::liquid_kstring::KString;
use crate::named_lookup::NamedLookup;
use crate::tags::args::{binding_name, parse_binding, parse_vars_from, Binding};
use liquid_core::error::ResultLiquidExt;
use liquid_core::runtime::{Interrupt, InterruptRegister, StackFrame};
use liquid_core::Expression;
use liquid_core::Language;
use liquid_core::Renderable;
use liquid_core::Runtime;
use liquid_core::ValueView;
use liquid_core::{Error, Result};
use liquid_core::{ParseTag, TagReflection, TagTokenIter};
use liquid_lib::stdlib::ForloopObject;
use std::collections::HashMap;
use std::io::Write;

/// Replaces `liquid_lib::stdlib::IncludeTag`, which supports neither Shopify's
/// comma-separated arguments nor its `with`/`for` clauses. Scope handling is
/// unchanged: the partial still sees the including template's variables.
#[derive(Copy, Clone, Debug, Default)]
pub struct IncludeTag;

impl TagReflection for IncludeTag {
    fn tag(&self) -> &'static str {
        "include"
    }

    fn description(&self) -> &'static str {
        "Renders a partial with the current scope"
    }
}

impl ParseTag for IncludeTag {
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

        Ok(Box::new(Include {
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
struct Include {
    partial: Expression,
    binding: Option<Binding>,
    /// Name a `with`/`for` value is bound to. `None` means the `as` clause was
    /// omitted, and the partial's own name is used.
    binding_name: Option<KString>,
    vars: Vec<(KString, Expression)>,
}

impl Include {
    fn render_partial(
        &self,
        writer: &mut dyn Write,
        scope: &dyn Runtime,
        name: &str,
    ) -> Result<()> {
        let partial = scope
            .partials()
            .get(name)
            .trace_with(|| format!("{{% include {} %}}", self.partial).into())?;

        partial
            .render_to(writer, &NamedLookup::new(scope))
            .trace_with(|| format!("{{% include {} %}}", self.partial).into())
            .context_key_with(|| self.partial.to_string().into())
            .value_with(|| name.to_string().into())
    }
}

fn evaluate<'a>(
    expression: &'a Expression,
    runtime: &'a dyn Runtime,
) -> Result<liquid_core::ValueCow<'a>> {
    expression
        .try_evaluate(runtime)
        .ok_or_else(|| Error::with_msg("failed to evaluate value"))
}

impl Renderable for Include {
    fn render_to(&self, writer: &mut dyn Write, runtime: &dyn Runtime) -> Result<()> {
        let value = self.partial.evaluate(runtime)?;
        if !value.is_scalar() {
            return Error::with_msg("Can only `include` strings")
                .context("partial", format!("{}", value.source()))
                .into_err();
        }
        let name = value.to_kstr().into_owned();

        let binding_name = binding_name(self.binding_name.as_ref(), &name);

        match &self.binding {
            Some(Binding::For(range)) => {
                let range = range
                    .evaluate(runtime)
                    .trace_with(|| format!("{{% include {} %}}", self.partial).into())?;
                let array = range.evaluate()?;
                let len = array.len();

                for (i, item) in array.into_iter().enumerate() {
                    let forloop = ForloopObject::new(i, len);
                    let mut scope_vars = HashMap::new();
                    for (id, val) in &self.vars {
                        scope_vars.insert(id.as_ref(), evaluate(val, runtime)?);
                    }
                    scope_vars.insert("forloop".into(), liquid_core::ValueCow::Borrowed(&forloop));
                    scope_vars.insert(binding_name.into(), item);

                    let scope = StackFrame::new(runtime, &scope_vars);
                    self.render_partial(writer, &scope, &name)?;

                    // The innermost loop owns an interrupt, so consume it here
                    // rather than letting it escape to the including template.
                    let interrupt = scope.registers().get_mut::<InterruptRegister>().reset();
                    if let Some(Interrupt::Break) = interrupt {
                        break;
                    }
                }
                Ok(())
            }
            binding => {
                let mut scope_vars = HashMap::new();
                for (id, val) in &self.vars {
                    scope_vars.insert(id.as_ref(), evaluate(val, runtime)?);
                }
                if let Some(Binding::With(value)) = binding {
                    scope_vars.insert(binding_name.into(), evaluate(value, runtime)?);
                }

                let scope = StackFrame::new(runtime, &scope_vars);
                self.render_partial(writer, &scope, &name)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
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
                "color",
                "posts/author",
                "posts/byline",
                "breaker",
            ]
        }

        fn try_get<'a>(&'a self, name: &str) -> Option<Cow<'a, str>> {
            match name {
                "vars" => Some("[{{a}}|{{b}}]".into()),
                "plain" => Some("plain".into()),
                "color" => Some("<{{color}}>".into()),
                "posts/author" => Some("<{{author}}>".into()),
                "posts/byline" => Some("<{{byline}}#{{forloop.index}}>".into()),
                "breaker" => Some("{{item}}{% if item == 2 %}{% break %}{% endif %}".into()),
                _ => None,
            }
        }
    }

    fn render(template: &str) -> Result<String> {
        let parser = liquid::ParserBuilder::with_stdlib()
            .tag(IncludeTag)
            .partials(EagerCompiler::new(TestSource))
            .build()
            .unwrap();
        parser
            .parse(template)?
            .render(&liquid::object!({ "outer": "o", "obj": { "name": "n" } }))
    }

    #[test]
    fn shopify_comma_separated_args() {
        assert_eq!(render("{% include 'vars', a: 1, b: 2 %}").unwrap(), "[1|2]");
    }

    #[test]
    fn shopify_comma_separated_args_trailing_comma() {
        assert_eq!(
            render("{% include 'vars', a: 1, b: 2, %}").unwrap(),
            "[1|2]"
        );
    }

    #[test]
    fn stdlib_whitespace_separated_args() {
        assert_eq!(render("{% include 'vars' a: 1, b: 2 %}").unwrap(), "[1|2]");
    }

    #[test]
    fn no_args() {
        assert_eq!(render("{% include 'plain' %}").unwrap(), "plain");
    }

    #[test]
    fn missing_separator_is_an_error() {
        assert!(render("{% include 'vars' a: 1 b: 2 %}").is_err());
    }

    #[test]
    fn no_file() {
        assert!(render("{% include 'nope' %}").is_err());
    }

    #[test]
    fn with_binds_to_the_partial_name() {
        assert_eq!(render("{% include 'color' with 'red' %}").unwrap(), "<red>");
    }

    #[test]
    fn with_binds_to_the_last_path_segment() {
        assert_eq!(
            render("{% include 'posts/author' with 'Jesse' %}").unwrap(),
            "<Jesse>"
        );
    }

    #[test]
    fn with_as_binds_to_an_alias() {
        assert_eq!(
            render("{% include 'vars' with 1 as a, b: 2 %}").unwrap(),
            "[1|2]"
        );
    }

    #[test]
    fn for_iterates_an_array() {
        assert_eq!(
            render("{% assign names = 'a,b' | split: ',' %}{% include 'posts/byline' for names %}")
                .unwrap(),
            "<a#1><b#2>"
        );
    }

    #[test]
    fn for_iterates_a_range() {
        assert_eq!(
            render("{% include 'vars' for (1..3) as a, b: 'x' %}").unwrap(),
            "[1|x][2|x][3|x]"
        );
    }

    #[test]
    fn for_provides_forloop() {
        assert_eq!(
            render("{% include 'posts/byline' for (1..2) %}").unwrap(),
            "<1#1><2#2>"
        );
    }

    #[test]
    fn for_stops_at_a_break_in_the_partial() {
        assert_eq!(
            render("{% include 'breaker' for (1..5) as item %}").unwrap(),
            "12"
        );
    }

    /// `include` shares the caller's scope; `with`/`for` must not change that.
    #[test]
    fn with_and_for_still_see_outer_scope() {
        assert_eq!(
            render("{% include 'vars' with 1 as a, b: outer %}").unwrap(),
            "[1|o]"
        );
        assert_eq!(
            render("{% include 'vars' for (1..1) as a, b: outer %}").unwrap(),
            "[1|o]"
        );
    }

    #[test]
    fn with_binds_a_variable() {
        assert_eq!(render("{% include 'color' with outer %}").unwrap(), "<o>");
        assert_eq!(
            render("{% include 'color' with obj.name %}").unwrap(),
            "<n>"
        );
    }
}
