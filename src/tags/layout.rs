use crate::liquid_kstring::KString;
use crate::named_lookup::NamedLookup;
use crate::tags::args::parse_vars;
use liquid_core::error::ResultLiquidExt;
use liquid_core::Expression;
use liquid_core::Language;
use liquid_core::Renderable;
use liquid_core::ValueView;
use liquid_core::{runtime::StackFrame, Runtime};
use liquid_core::{Error, Result};
use liquid_core::{ParseTag, TagReflection, TagTokenIter};
use once_cell::sync::Lazy;
use regex::Regex;
use std::cell::RefCell;
use std::io::Write;

thread_local! {
    static ACTIVE_LAYOUT_STACK: RefCell<Vec<String>> = const { RefCell::new(vec![]) };
}

struct LayoutStackGuard;
impl Drop for LayoutStackGuard {
    fn drop(&mut self) {
        ACTIVE_LAYOUT_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            let _ = stack.pop();
        });
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct LayoutTag;

impl LayoutTag {}

impl TagReflection for LayoutTag {
    fn tag(&self) -> &'static str {
        "layout"
    }

    fn description(&self) -> &'static str {
        "Renders a layout with the current template as the content"
    }
}

impl ParseTag for LayoutTag {
    fn parse(
        &self,
        mut arguments: TagTokenIter<'_>,
        _options: &Language,
    ) -> Result<Box<dyn Renderable>> {
        let partial = arguments
            .expect_next("Identifier or literal expected.")?
            .expect_value()
            .into_result()?;
        let vars = parse_vars(&mut arguments)?;
        Ok(Box::new(Layout { partial, vars }))
    }

    fn reflection(&self) -> &dyn TagReflection {
        self
    }
}

#[derive(Debug)]
struct Layout {
    partial: Expression,
    vars: Vec<(KString, Expression)>,
}

static CONTENT_SENTINEL: &str = "___LAYOUT_CONTENT___";
static SNIP_SENTINEL: &str = "___LAYOUT_SNIP___";

impl Renderable for Layout {
    fn render_to(&self, writer: &mut dyn Write, runtime: &dyn Runtime) -> Result<()> {
        runtime.set_global(
            "page_content".into(),
            liquid_core::Value::Scalar(CONTENT_SENTINEL.into()),
        );
        let value = self.partial.evaluate(runtime)?;
        if !value.is_scalar() {
            return Error::with_msg("Can only use `layout` with strings")
                .context("partial", format!("{}", value.source()))
                .into_err();
        }
        let name = value.to_kstr().into_owned();

        let cycle = ACTIVE_LAYOUT_STACK.with(|stack| {
            let stack = stack.borrow();
            stack.iter().any(|entry| entry == name.as_str())
        });
        if cycle {
            let chain = ACTIVE_LAYOUT_STACK.with(|stack| {
                let mut names = stack.borrow().clone();
                names.push(name.to_string());
                names.join(" -> ")
            });
            return Error::with_msg("Recursive layout include detected")
                .context("layout_chain", chain)
                .into_err();
        }
        ACTIVE_LAYOUT_STACK.with(|stack| {
            stack.borrow_mut().push(name.to_string());
        });
        let _layout_guard = LayoutStackGuard;

        {
            // if there our additional variables creates a layout object to access all the variables
            // from e.g. { layout "image.html" "path" => "foo.png" }
            // then in image.html you could have <img src="{{layout.path}}" />
            let mut pass_through = std::collections::HashMap::new();
            if !self.vars.is_empty() {
                for (id, val) in &self.vars {
                    let value = val
                        .try_evaluate(runtime)
                        .ok_or_else(|| Error::with_msg("failed to evaluate value"))?;

                    pass_through.insert(id.as_ref(), value);
                }
            }

            let scope = StackFrame::new(runtime, &pass_through);
            let partial = scope
                .partials()
                .get(&name)
                .trace_with(|| format!("{{% layout {} %}}", self.partial).into())?;

            partial
                .render_to(writer, &NamedLookup::new(&scope))
                .trace_with(|| format!("{{% layout {} %}}", self.partial).into())
                .context_key_with(|| self.partial.to_string().into())
                .value_with(|| name.to_string().into())?;

            writer.write_all(SNIP_SENTINEL.as_bytes()).unwrap();
        }

        Ok(())
    }
}

static SNIP_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(&format!("{}(?<content>[\\s\\S]*)", SNIP_SENTINEL)).unwrap());
pub fn post_process(mut rendered: String) -> String {
    if let Some(captured) = SNIP_RE.captures(&rendered) {
        rendered = rendered.replace(CONTENT_SENTINEL, &captured["content"]);
    }
    SNIP_RE.replace(&rendered, "").into()
}

#[cfg(test)]
mod test {
    use anyhow::Result;
    use std::borrow;

    use liquid_core::partials::PartialCompiler;
    use liquid_core::runtime::RuntimeBuilder;
    use liquid_core::Value;
    use liquid_core::{parser, partials, runtime, Language, Template};
    use liquid_lib::stdlib;

    use super::*;

    pub trait ToTemplate {
        fn to_template(&self, options: &Language) -> Result<Template>;
    }
    impl ToTemplate for &str {
        fn to_template(&self, options: &Language) -> Result<Template> {
            Ok(parser::parse(self, options).map(runtime::Template::new)?)
        }
    }

    fn options() -> Language {
        let mut options = Language::default();
        options
            .tags
            .register("layout".to_string(), LayoutTag.into());
        options
            .blocks
            .register("comment".to_string(), stdlib::CommentBlock.into());
        options
            .blocks
            .register("if".to_string(), stdlib::IfBlock.into());
        options
    }

    #[derive(Default, Debug, Clone, Copy)]
    struct TestSource;

    impl partials::PartialSource for TestSource {
        fn contains(&self, _name: &str) -> bool {
            true
        }

        fn names(&self) -> Vec<&str> {
            vec![]
        }

        fn try_get<'a>(&'a self, name: &str) -> Option<borrow::Cow<'a, str>> {
            match name {
                "example.liquid" => Some(r#"{{'whooo'}}{%comment%}What happens{%endcomment%} {%if num < numTwo%}wat{%else%}wot{%endif%} {%if num > numTwo%}wat{%else%}wot{%endif%}{{ page_content }}"#.into()),
                "example_var.liquid" => Some(r#"{{example_var}}{{ page_content }}"#.into()),
                "example_multi_var.liquid" => Some(r#"{{example_var}} {{example}}{{ page_content }}"#.into()),
                "example_json.json" => Some(r#"{"var": "{{example_var}}", "content": "{{example}}{{ page_content }}"}"#.into()),
                _ => None
            }
        }
    }

    #[test]
    fn layout_tag_quotes() -> Result<()> {
        let options = options();
        let template = "{% layout 'example.liquid' %}\ntest test".to_template(&options)?;

        let partials = partials::OnDemandCompiler::<TestSource>::empty()
            .compile(::std::sync::Arc::new(options))
            .unwrap();
        let runtime = RuntimeBuilder::new()
            .set_partials(partials.as_ref())
            .build();
        runtime.set_global("num".into(), Value::scalar(5f64));
        runtime.set_global("numTwo".into(), Value::scalar(10f64));
        let output = post_process(template.render(&runtime).unwrap());
        assert_eq!(output, "whooo wat wot\ntest test");
        Ok(())
    }

    #[test]
    fn layout_variable() -> Result<()> {
        let options = options();
        let template =
            "{% layout 'example_var.liquid' example_var:\"hello\" %}".to_template(&options)?;

        let partials = partials::OnDemandCompiler::<TestSource>::empty()
            .compile(::std::sync::Arc::new(options))
            .unwrap();
        let runtime = RuntimeBuilder::new()
            .set_partials(partials.as_ref())
            .build();
        let output = post_process(template.render(&runtime).unwrap());
        assert_eq!(output, "hello");
        Ok(())
    }

    #[test]
    fn layout_multiple_variables() -> Result<()> {
        let options = options();
        let template =
            "{% layout 'example_multi_var.liquid' example_var:\"hello\", example:\"world\" %}"
                .to_template(&options)?;

        let partials = partials::OnDemandCompiler::<TestSource>::empty()
            .compile(::std::sync::Arc::new(options))
            .unwrap();
        let runtime = RuntimeBuilder::new()
            .set_partials(partials.as_ref())
            .build();
        let output = post_process(template.render(&runtime).unwrap());
        assert_eq!(output, "hello world");
        Ok(())
    }

    #[test]
    fn layout_multiple_variables_trailing_comma() -> Result<()> {
        let options = options();
        let template =
            "{% layout 'example_multi_var.liquid' example_var:\"hello\", example:\"dogs\", %}"
                .to_template(&options)?;

        let partials = partials::OnDemandCompiler::<TestSource>::empty()
            .compile(::std::sync::Arc::new(options))
            .unwrap();
        let runtime = RuntimeBuilder::new()
            .set_partials(partials.as_ref())
            .build();
        let output = post_process(template.render(&runtime).unwrap());
        assert_eq!(output, "hello dogs");
        Ok(())
    }

    #[test]
    fn json() -> Result<()> {
        let options = options();
        let template = "{% layout 'example_json.json' example_var:\"hello\", example:\"dogs\", %}"
            .to_template(&options)?;

        let partials = partials::OnDemandCompiler::<TestSource>::empty()
            .compile(::std::sync::Arc::new(options))
            .unwrap();
        let runtime = RuntimeBuilder::new()
            .set_partials(partials.as_ref())
            .build();
        let output = post_process(template.render(&runtime).unwrap());
        let value: serde_json::Value = serde_json::from_str(&output)?;
        println!("rendered JSON: {:?}", value);
        assert_eq!(value["var"], "hello");
        assert_eq!(value["content"], "dogs");
        Ok(())
    }

    #[test]
    fn no_file() -> Result<()> {
        let options = options();
        let template = "{% layout 'file_does_not_exist.liquid' %}".to_template(&options)?;

        let partials = partials::OnDemandCompiler::<TestSource>::empty()
            .compile(::std::sync::Arc::new(options))
            .unwrap();
        let runtime = RuntimeBuilder::new()
            .set_partials(partials.as_ref())
            .build();
        runtime.set_global("num".into(), Value::scalar(5f64));
        runtime.set_global("numTwo".into(), Value::scalar(10f64));
        let output = template.render(&runtime);
        assert!(output.is_err());
        Ok(())
    }

    #[test]
    fn recursive_layout_fails_fast() -> Result<()> {
        let options = options();
        let template = "{% layout 'loop.liquid' %}".to_template(&options)?;

        #[derive(Default, Debug, Clone, Copy)]
        struct RecursiveSource;

        impl partials::PartialSource for RecursiveSource {
            fn contains(&self, _name: &str) -> bool {
                true
            }

            fn names(&self) -> Vec<&str> {
                vec![]
            }

            fn try_get<'a>(&'a self, name: &str) -> Option<borrow::Cow<'a, str>> {
                match name {
                    "loop.liquid" => Some("{% layout 'loop.liquid' %}".into()),
                    _ => None,
                }
            }
        }

        let partials = partials::OnDemandCompiler::<RecursiveSource>::empty()
            .compile(::std::sync::Arc::new(options))
            .unwrap();
        let runtime = RuntimeBuilder::new()
            .set_partials(partials.as_ref())
            .build();
        let output = template.render(&runtime);
        assert!(output.is_err());
        Ok(())
    }
}
