use crate::liquid_kstring::KString;
use liquid_core::parser::{TagToken, TryMatchToken};
use liquid_core::Expression;
use liquid_core::Result;
use liquid_core::TagTokenIter;
use liquid_lib::stdlib::RangeExpression;

/// A Shopify `with`/`for` clause: one value, or a sequence to render once per
/// item.
#[derive(Debug)]
pub(crate) enum Binding {
    With(Expression),
    For(RangeExpression),
}

/// Parses the optional `with <value>` / `for <range>` clause and its optional
/// `as <name>` alias, shared by `include` and `render`.
///
/// Takes the token following the partial name and returns the first token that
/// was not part of the clause, ready to hand to [`parse_vars_from`].
pub(crate) fn parse_binding<'a>(
    first: Option<TagToken<'a>>,
    arguments: &mut TagTokenIter<'a>,
) -> Result<(Option<Binding>, Option<KString>, Option<TagToken<'a>>)> {
    let mut token = first;
    let binding = match token.as_ref().map(|t| t.as_str()) {
        Some("with") => {
            let value = arguments
                .expect_next("expected value")?
                .expect_value()
                .into_result()?;
            token = arguments.next();
            Some(Binding::With(value))
        }
        Some("for") => {
            let range = arguments.expect_next("Array or range expected.")?;
            let range = match range.expect_value() {
                TryMatchToken::Matches(array) => RangeExpression::Array(array),
                TryMatchToken::Fails(range) => match range.expect_range() {
                    TryMatchToken::Matches((start, stop)) => RangeExpression::Counted(start, stop),
                    TryMatchToken::Fails(range) => return range.raise_error().into_err(),
                },
            };
            token = arguments.next();
            Some(Binding::For(range))
        }
        _ => None,
    };

    let mut name = None;
    if binding.is_some() {
        if let Some(t) = token {
            match t.expect_str("as") {
                TryMatchToken::Matches(()) => {
                    name = Some(
                        arguments
                            .expect_next("Identifier expected.")?
                            .expect_identifier()
                            .into_result()?
                            .to_owned()
                            .into(),
                    );
                    token = arguments.next();
                }
                TryMatchToken::Fails(t) => token = Some(t),
            }
        }
    }

    Ok((binding, name, token))
}

/// Shopify binds a `with`/`for` value to a variable named after the partial
/// when `as` is omitted. Archival partials are named by path, so only the final
/// segment is a usable identifier.
pub(crate) fn binding_name<'a>(alias: Option<&'a KString>, partial: &'a str) -> &'a str {
    match alias {
        Some(bound) => bound.as_str(),
        None => partial.rsplit('/').next().unwrap_or(partial),
    }
}

/// Parses the `key: value` argument list shared by `include`, `layout` and
/// `render`.
///
/// Shopify separates the partial name from its first argument with a comma
/// (`{% include 'a', k: v %}`) where stdlib liquid uses only whitespace
/// (`{% include 'a' k: v %}`). Both are accepted, as is a trailing comma.
pub(crate) fn parse_vars(arguments: &mut TagTokenIter<'_>) -> Result<Vec<(KString, Expression)>> {
    let first = arguments.next();
    parse_vars_from(first, arguments)
}

/// [`parse_vars`] for callers that have already pulled the token following the
/// partial name (to check for a `with`/`for` clause).
pub(crate) fn parse_vars_from<'a>(
    first: Option<TagToken<'a>>,
    arguments: &mut TagTokenIter<'a>,
) -> Result<Vec<(KString, Expression)>> {
    let mut vars: Vec<(KString, Expression)> = Vec::new();
    let mut token = match first {
        Some(t) => match t.expect_str(",") {
            TryMatchToken::Matches(()) => arguments.next(),
            TryMatchToken::Fails(t) => Some(t),
        },
        None => None,
    };

    while let Some(t) = token {
        let id = t.expect_identifier().into_result()?.to_owned();

        arguments
            .expect_next("\":\" expected.")?
            .expect_str(":")
            .into_result_custom_msg("expected \":\" to be used for the assignment")?;

        vars.push((
            id.into(),
            arguments
                .expect_next("expected value")?
                .expect_value()
                .into_result()?,
        ));

        token = match arguments.next() {
            Some(comma) => match comma.expect_str(",") {
                TryMatchToken::Matches(()) => arguments.next(),
                TryMatchToken::Fails(t) => {
                    return t
                        .raise_custom_error("`,` is needed to separate variables")
                        .into_err()
                }
            },
            None => None,
        };
    }

    arguments.expect_nothing()?;

    Ok(vars)
}
