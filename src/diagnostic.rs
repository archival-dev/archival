//! What archival reports about a site that is wrong, in a form other programs
//! can read.
//!
//! An error printed for a person is a sentence; the same error read by an
//! editor, a CI job or a model needs the parts separated - which file, which
//! key, which rule - and needs them to survive archival's prose being reworded.
//! [`Code`] is the part that is promised to stay stable; `message` and `help`
//! are not.

use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// The format of a serialized report. Bumped when a consumer would have to
/// change to keep reading one.
pub const DIAGNOSTIC_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// The site will not build, or will build into something archival cannot
    /// read back.
    Error,
    /// The site builds, and something in it does not do what it looks like it
    /// does - a value archival drops, a field no template can reach.
    Warning,
}

/// Which rule was broken. Stable across releases: a variant is added or
/// deprecated, never repurposed, so a stored result stays comparable with a
/// later archival's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// The file is not TOML.
    TomlSyntax,
    /// A field is named one of the names archival populates itself.
    ReservedFieldName,
    /// An object type is named one archival reserves.
    ReservedObjectName,
    /// A field's type is not one archival has, and no editor type aliases it.
    UnrecognizedType,
    /// An array of tables that is not a well-formed oneof.
    InvalidOneof,
    /// An array that is not a string enum.
    InvalidEnum,
    /// A value that does not fit the type its field declares.
    TypeMismatch,
    /// A child entry archival cannot read.
    InvalidChild,
    /// The definitions file is wrong in a way this archival cannot classify.
    /// A reader should treat it as an error and show `message`.
    InvalidDefinitions,
    /// `archival.toml` is wrong.
    InvalidManifest,
    /// An object file is wrong in a way this archival cannot classify.
    InvalidObject,
    /// A key on an object that its definition does not declare. Archival drops
    /// the value, so a template reading it renders nothing.
    UnknownField,
    /// `order` is present and is not a number, so it does not sort.
    NonNumericOrder,
    /// A template does not parse.
    TemplateParse,
    /// A template a page or an object type names is not there.
    TemplateMissing,
}

impl Code {
    /// The severity a code carries when nothing more specific is known.
    pub fn severity(self) -> Severity {
        match self {
            Self::UnknownField | Self::NonNumericOrder => Severity::Warning,
            _ => Severity::Error,
        }
    }
}

impl Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The serde spelling, so a printed code and a serialized one match.
        let name = serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        write!(f, "{name}")
    }
}

/// Where in a file a diagnostic is about. Byte offsets for a program, line and
/// column for a person; both are counted from the start of the file, and
/// `line`/`column` are 1-based in characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: u32,
    pub column: u32,
}

impl Span {
    /// The span `range` covers in `source`.
    pub fn new(source: &str, range: Range<usize>) -> Self {
        let start = range.start.min(source.len());
        let end = range.end.min(source.len()).max(start);
        let before = &source[..start];
        let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let line_start = before.rfind('\n').map_or(0, |at| at + 1);
        let column = source[line_start..start].chars().count() as u32 + 1;
        Self {
            start,
            end,
            line,
            column,
        }
    }

    /// The whole of `source`, for an error that names no key.
    pub fn whole(source: &str) -> Self {
        Self::new(source, 0..source.len())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Code,
    /// What went wrong, for a person. Not stable; match on `code`.
    pub message: String,
    /// What to do about it, where archival knows. Not stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Relative to the site root, so a report does not carry a local path.
    pub file: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    /// The object type this is about, where there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object: Option<String>,
    /// The field this is about, where there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl Diagnostic {
    pub fn new(code: Code, file: impl AsRef<Path>, message: impl Into<String>) -> Self {
        Self {
            severity: code.severity(),
            code,
            message: message.into(),
            help: None,
            file: file.as_ref().to_path_buf(),
            span: None,
            object: None,
            field: None,
        }
    }

    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn about(mut self, object: Option<String>, field: Option<String>) -> Self {
        self.object = object;
        self.field = field;
        self
    }

    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
}

impl Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let where_ = match &self.span {
            Some(span) => format!("{}:{}:{}", self.file.display(), span.line, span.column),
            None => self.file.display().to_string(),
        };
        write!(f, "{where_}: {} [{}]", self.message, self.code)?;
        if let Some(help) = &self.help {
            write!(f, "\n  help: {help}")?;
        }
        Ok(())
    }
}

/// Everything one pass found, plus what produced it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    /// The archival that produced this.
    pub archival: String,
    /// [`DIAGNOSTIC_SCHEMA`] at the time it was written.
    pub schema: u32,
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    pub fn new(diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            archival: env!("CARGO_PKG_VERSION").to_string(),
            schema: DIAGNOSTIC_SCHEMA,
            diagnostics,
        }
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|one| one.is_error())
    }

    pub fn has_errors(&self) -> bool {
        self.errors().next().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_span_counts_lines_from_one() {
        let source = "a = 1\nbb = 2\nccc = 3\n";
        let at = source.find("bb").unwrap();
        let span = Span::new(source, at..at + 2);
        assert_eq!((span.line, span.column), (2, 1));
        assert_eq!((span.start, span.end), (at, at + 2));
    }

    /// Columns are characters, not bytes, so an accented name does not shift
    /// everything after it.
    #[test]
    fn a_column_counts_characters() {
        let source = "café = \"string\"\n";
        let at = source.find('=').unwrap();
        assert_eq!(Span::new(source, at..at + 1).column, 6);
    }

    #[test]
    fn a_span_past_the_end_is_clamped() {
        let source = "a = 1\n";
        let span = Span::new(source, 100..200);
        assert_eq!((span.start, span.end), (source.len(), source.len()));
    }

    /// A stored result has to stay readable, so the wire names are pinned here
    /// rather than left to the enum's spelling.
    #[test]
    fn codes_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&Code::ReservedFieldName).unwrap(),
            "\"reserved_field_name\""
        );
        assert_eq!(Code::UnrecognizedType.to_string(), "unrecognized_type");
    }

    #[test]
    fn warnings_are_not_errors() {
        assert_eq!(Code::UnknownField.severity(), Severity::Warning);
        let report = Report::new(vec![Diagnostic::new(
            Code::UnknownField,
            "objects/a.toml",
            "x",
        )]);
        assert!(!report.has_errors());
    }
}
