//! Carries comments from a TOML file across a rewrite by `toml`, which drops them.

use toml_edit::{Array, Decor, DocumentMut, InlineTable, Item, RawString, Table, Value};

/// Carries the comments in `original` over to the same keys, tables and array
/// entries in `formatted`. Comments attached to anything `formatted` no longer
/// contains are dropped.
pub fn preserve_comments(original: &str, formatted: &str) -> String {
    let (Ok(original), Ok(mut output)) = (
        original.parse::<DocumentMut>(),
        formatted.parse::<DocumentMut>(),
    ) else {
        return formatted.to_string();
    };
    copy_table(original.as_table(), output.as_table_mut());
    if let Some(trailing) =
        with_comments(raw(original.trailing()), raw(output.trailing()), "", false)
    {
        output.set_trailing(trailing);
    }
    output.to_string()
}

fn copy_table(from: &Table, to: &mut Table) {
    copy_decor(from.decor(), to.decor_mut(), false);
    for (mut key, item) in to.iter_mut() {
        let Some((from_key, from_item)) = from.get_key_value(key.get()) else {
            continue;
        };
        copy_decor(from_key.leaf_decor(), key.leaf_decor_mut(), false);
        copy_item(from_item, item);
    }
}

fn copy_item(from: &Item, to: &mut Item) {
    match (from, to) {
        (Item::Table(from), Item::Table(to)) => copy_table(from, to),
        (Item::ArrayOfTables(from), Item::ArrayOfTables(to)) => {
            for (from, to) in from.iter().zip(to.iter_mut()) {
                copy_table(from, to);
            }
        }
        (Item::Value(from), Item::Value(to)) => copy_value(from, to),
        // `[a]` and `a = { ... }` hold the same data, so comments survive a
        // change between the two spellings.
        (Item::Table(from), Item::Value(Value::InlineTable(to))) => {
            for (mut key, value) in to.iter_mut() {
                if let Some((from_key, Item::Value(from_value))) = from.get_key_value(key.get()) {
                    copy_decor(from_key.leaf_decor(), key.leaf_decor_mut(), false);
                    copy_value(from_value, value);
                }
            }
        }
        (Item::Value(Value::InlineTable(from)), Item::Table(to)) => {
            copy_inline_into_table(from, to)
        }
        (Item::Value(Value::Array(from)), Item::ArrayOfTables(to)) => {
            for (from, to) in from.iter().zip(to.iter_mut()) {
                if let Value::InlineTable(from) = from {
                    copy_decor(from.decor(), to.decor_mut(), false);
                    copy_inline_into_table(from, to);
                }
            }
        }
        _ => {}
    }
}

fn copy_inline_into_table(from: &InlineTable, to: &mut Table) {
    for (mut key, item) in to.iter_mut() {
        if let Some((from_key, from_item)) = from.get_key_value(key.get()) {
            copy_decor(from_key.leaf_decor(), key.leaf_decor_mut(), false);
            copy_item(from_item, item);
        }
    }
}

fn copy_value(from: &Value, to: &mut Value) {
    copy_decor(from.decor(), to.decor_mut(), true);
    match (from, to) {
        (Value::Array(from), Value::Array(to)) => copy_array(from, to),
        (Value::InlineTable(from), Value::InlineTable(to)) => {
            for (mut key, value) in to.iter_mut() {
                if let Some((from_key, Item::Value(from_value))) = from.get_key_value(key.get()) {
                    copy_decor(from_key.leaf_decor(), key.leaf_decor_mut(), false);
                    copy_value(from_value, value);
                }
            }
        }
        _ => {}
    }
}

fn copy_array(from: &Array, to: &mut Array) {
    for (from, to) in from.iter().zip(to.iter_mut()) {
        copy_value(from, to);
    }
    let indent = to
        .iter()
        .last()
        .and_then(|v| v.decor().prefix())
        .map(raw)
        .map_or("", |p| &p[p.rfind('\n').map_or(0, |i| i + 1)..])
        .to_string();
    if let Some(trailing) = with_comments(raw(from.trailing()), raw(to.trailing()), &indent, true) {
        to.set_trailing(trailing);
    }
}

/// Line comments live in a prefix, trailing comments (`a = 1 # note`) in a
/// suffix. Only the comments are taken; whitespace stays as formatted.
/// `mid_line` is true for a value's prefix, which starts after `=`, `[` or `,`
/// rather than at the start of a line.
fn copy_decor(from: &Decor, to: &mut Decor, mid_line: bool) {
    let existing = to.prefix().map(raw).unwrap_or_default();
    let indent = &existing[existing.rfind('\n').map_or(0, |i| i + 1)..];
    if let Some(prefix) = with_comments(
        from.prefix().map(raw).unwrap_or_default(),
        existing,
        indent,
        mid_line,
    ) {
        to.set_prefix(prefix);
    }
    if let Some(comment) = from
        .suffix()
        .map(raw)
        .and_then(|s| s.find('#').map(|i| &s[i..]))
    {
        let comment = comment.trim_end();
        let existing = to.suffix().map(raw).unwrap_or_default();
        to.set_suffix(format!("{existing} {comment}"));
    }
}

/// Inserts the comments of `from` into the formatted whitespace `to`, or
/// `None` if `from` has none. When `mid_line`, a comment before the first
/// newline of `from` followed the previous item on its line, so it stays there.
fn with_comments(from: &str, to: &str, indent: &str, mid_line: bool) -> Option<String> {
    let (same_line, rest) = match from.find('\n') {
        _ if !mid_line => ("", from),
        Some(i) => (from[..i].trim(), &from[i + 1..]),
        None if from.trim_start().starts_with('#') => (from.trim(), ""),
        None => ("", from),
    };
    let same_line = same_line.starts_with('#').then_some(same_line);

    let mut lines = String::new();
    let mut pending_blank = false;
    for line in rest.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            if pending_blank && !lines.is_empty() {
                lines.push('\n');
            }
            lines.push_str(indent);
            lines.push_str(line);
            lines.push('\n');
            pending_blank = false;
        } else if line.is_empty() {
            pending_blank = true;
        }
    }
    if same_line.is_none() && lines.is_empty() {
        return None;
    }

    let (head, tail) = to
        .rfind('\n')
        .map_or(("", to), |i| (&to[..=i], &to[i + 1..]));
    let mut out = String::new();
    if let Some(comment) = same_line {
        out.push(' ');
        out.push_str(comment);
        out.push('\n');
        // The formatter's own line break is replaced by the one after the comment.
        out.push_str(head.strip_prefix('\n').unwrap_or(head));
    } else {
        out.push_str(head);
    }
    if !lines.is_empty() && !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&lines);
    out.push_str(tail);
    Some(out)
}

fn raw(s: &RawString) -> &str {
    s.as_str().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn keeps_comments_above_keys() {
        let original = "# The title.\n# Shown in listings.\nname=\"Some Content\"\nbody=\"x\"\n";
        let formatted = "name = \"Some Content\"\nbody = \"x\"\n";
        assert_eq!(
            preserve_comments(original, formatted),
            "# The title.\n# Shown in listings.\nname = \"Some Content\"\nbody = \"x\"\n"
        );
    }

    #[test]
    fn keeps_trailing_comments() {
        let original = "name=\"a\"   # note  \nbody=\"x\"\n";
        let formatted = "name = \"a\"\nbody = \"x\"\n";
        assert_eq!(
            preserve_comments(original, formatted),
            "name = \"a\" # note\nbody = \"x\"\n"
        );
    }

    #[test]
    fn keeps_comments_when_keys_are_reordered() {
        let original = "body = \"x\"\n# The name.\nname = \"a\"\n";
        let formatted = "name = \"a\"\nbody = \"x\"\n";
        assert_eq!(
            preserve_comments(original, formatted),
            "# The name.\nname = \"a\"\nbody = \"x\"\n"
        );
    }

    #[test]
    fn keeps_comments_on_tables_and_array_of_tables() {
        let original = r#"name = "a"
# Links.
[[links]]
# Where it goes.
url = "https://a"

# Second.
[[links]]
url = "https://b"
# End of file.
"#;
        let formatted = r#"name = "a"

[[links]]
url = "https://a"

[[links]]
url = "https://b"
"#;
        assert_eq!(
            preserve_comments(original, formatted),
            r#"name = "a"

# Links.
[[links]]
# Where it goes.
url = "https://a"

# Second.
[[links]]
url = "https://b"
# End of file.
"#
        );
    }

    #[test]
    fn keeps_comments_inside_arrays() {
        let original = "tags = [\n  # first\n  \"a\",\n  \"b\", # second\n  # done\n]\n";
        let formatted = "tags = [\n    \"a\",\n    \"b\",\n]\n";
        assert_eq!(
            preserve_comments(original, formatted),
            "tags = [\n    # first\n    \"a\",\n    \"b\", # second\n    # done\n]\n"
        );
    }

    #[test]
    fn separate_comment_blocks_stay_separate() {
        let original = "# A file note.\n\n\n# The name.\nname = \"a\"\n";
        let formatted = "name = \"a\"\n";
        assert_eq!(
            preserve_comments(original, formatted),
            "# A file note.\n\n# The name.\nname = \"a\"\n"
        );
    }

    #[test]
    fn drops_comments_for_removed_keys() {
        let original = "# Gone.\nold = 1\nname = \"a\"\n";
        let formatted = "name = \"a\"\n";
        assert_eq!(preserve_comments(original, formatted), formatted);
    }

    #[test]
    fn falls_back_to_formatted_output_on_unparseable_input() {
        let formatted = "name = \"a\"\n";
        assert_eq!(preserve_comments("name = = oops", formatted), formatted);
    }
}
