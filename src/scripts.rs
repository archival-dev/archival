//! Turns a site's `scripts_dir` into the modules served from `scripts_build_dir`: each
//! TypeScript file has its types stripped, the way carriers are built, and plain
//! JavaScript is copied as written. Nothing is bundled.

use std::fmt;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptKind {
    TypeScript,
    JavaScript,
}

/// Where a file in `scripts_dir` builds to, relative to `scripts_build_dir`, or `None`
/// for a file that is not a browser script.
pub(crate) fn output_path(rel: &Path) -> Option<(PathBuf, ScriptKind)> {
    let hidden = rel.components().any(|c| match c {
        Component::Normal(name) => {
            let name = name.to_string_lossy();
            name.starts_with('.') || name == "node_modules"
        }
        _ => false,
    });
    if hidden {
        return None;
    }
    let name = rel.file_name()?.to_str()?;
    if name.ends_with(".d.ts") || name.ends_with(".d.mts") {
        return None;
    }
    let (extension, kind) = match rel.extension()?.to_str()? {
        "ts" => ("js", ScriptKind::TypeScript),
        "mts" => ("mjs", ScriptKind::TypeScript),
        "js" => ("js", ScriptKind::JavaScript),
        "mjs" => ("mjs", ScriptKind::JavaScript),
        _ => return None,
    };
    Some((rel.with_extension(extension), kind))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFailure {
    pub path: PathBuf,
    /// One `path:line:col: message` line per diagnostic.
    pub message: String,
}

#[derive(Debug, Clone, Error)]
pub struct ScriptErrors(pub Vec<ScriptFailure>);

impl ScriptErrors {
    pub(crate) fn check(failures: Vec<ScriptFailure>) -> anyhow::Result<()> {
        if failures.is_empty() {
            Ok(())
        } else {
            Err(ScriptErrors(failures).into())
        }
    }
}

impl fmt::Display for ScriptErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.0.len();
        writeln!(
            f,
            "{count} script{} failed to compile:",
            if count == 1 { "" } else { "s" }
        )?;
        for failure in &self.0 {
            writeln!(f, "{}", failure.message)?;
        }
        write!(
            f,
            "(set scripts_dir = false in archival.toml to turn off script compilation)"
        )
    }
}

#[derive(Debug, Clone, Error)]
#[error("{static_file} and {script} both build to {dest}")]
pub struct ScriptCollision {
    pub static_file: String,
    pub script: String,
    pub dest: String,
}

#[cfg(feature = "compile-scripts")]
pub(crate) use strip::compile;

#[cfg(feature = "compile-scripts")]
mod strip {
    use super::ScriptFailure;
    use std::path::Path;
    use std::sync::{Arc, Mutex};
    use swc_common::{
        errors::{DiagnosticBuilder, Emitter, Handler, HANDLER},
        sync::Lrc,
        BytePos, FileName, Globals, SourceMap, Span, GLOBALS,
    };
    use swc_ecma_ast::{CallExpr, Callee, ExportAll, Expr, ImportDecl, Lit, NamedExport};
    use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax};
    use swc_ecma_visit::{Visit, VisitWith};
    use swc_ts_fast_strip::{operate, Mode, Options};

    type Reported = Arc<Mutex<Vec<(String, Option<BytePos>)>>>;

    /// Positions are resolved after the fact: the source map is not `Send`, and an
    /// `Emitter` must be.
    struct Collect(Reported);

    impl Emitter for Collect {
        fn emit(&mut self, db: &mut DiagnosticBuilder<'_>) {
            let pos = db.span.primary_span().map(|span| span.lo);
            self.0.lock().unwrap().push((db.message(), pos));
        }
    }

    fn syntax() -> TsSyntax {
        TsSyntax {
            decorators: true,
            ..Default::default()
        }
    }

    pub(crate) fn compile(source: &str, path: &Path) -> Result<String, ScriptFailure> {
        let display = path.to_string_lossy().into_owned();
        let source = rewrite_specifiers(source);
        let cm: Lrc<SourceMap> = Default::default();
        let reported = Reported::default();
        let handler = Handler::with_emitter(true, false, Box::new(Collect(reported.clone())));
        // `operate` reports unsupported syntax through the scoped HANDLER, not the
        // handler it is given.
        let result = GLOBALS.set(&Globals::new(), || {
            HANDLER.set(&handler, || {
                operate(
                    &cm,
                    &handler,
                    source,
                    Options {
                        module: Some(true),
                        filename: Some(display.clone()),
                        parser: syntax(),
                        mode: Mode::StripOnly,
                        ..Default::default()
                    },
                )
            })
        });
        result.map(|output| output.code).map_err(|err| {
            let mut lines: Vec<String> = reported
                .lock()
                .unwrap()
                .iter()
                .map(|(message, pos)| match pos {
                    Some(pos) => {
                        let loc = cm.lookup_char_pos(*pos);
                        format!("{display}:{}:{}: {message}", loc.line, loc.col_display + 1)
                    }
                    None => format!("{display}: {message}"),
                })
                .collect();
            if lines.is_empty() {
                lines.push(format!("{display}: {err}"));
            }
            ScriptFailure {
                path: path.to_path_buf(),
                message: lines.join("\n"),
            }
        })
    }

    /// Points relative imports of `.ts`/`.mts` files at the `.js`/`.mjs` they build to.
    /// The swap is one byte, so every position in the file is unchanged. Source that
    /// does not parse is returned as is, for `operate` to report.
    fn rewrite_specifiers(source: &str) -> String {
        let cm: Lrc<SourceMap> = Default::default();
        let fm = cm.new_source_file(Lrc::new(FileName::Anon), source.to_string());
        let mut parser = Parser::new(Syntax::Typescript(syntax()), StringInput::from(&*fm), None);
        let Ok(module) = parser.parse_module() else {
            return source.to_string();
        };
        let mut specifiers = Specifiers::default();
        module.visit_with(&mut specifiers);
        let mut bytes = source.as_bytes().to_vec();
        for span in specifiers.0 {
            let lo = (span.lo.0 - fm.start_pos.0) as usize;
            let hi = (span.hi.0 - fm.start_pos.0) as usize;
            let Some(specifier) = source.get(lo + 1..hi.saturating_sub(1)) else {
                continue;
            };
            let local = specifier.starts_with("./")
                || specifier.starts_with("../")
                || (specifier.starts_with('/') && !specifier.starts_with("//"));
            if local && (specifier.ends_with(".ts") || specifier.ends_with(".mts")) {
                // The `t` of `ts`, just before the closing quote.
                bytes[hi - 3] = b'j';
            }
        }
        String::from_utf8(bytes).expect("an ASCII byte swapped for another stays UTF-8")
    }

    #[derive(Default)]
    struct Specifiers(Vec<Span>);

    impl Visit for Specifiers {
        fn visit_import_decl(&mut self, n: &ImportDecl) {
            self.0.push(n.src.span);
        }
        fn visit_export_all(&mut self, n: &ExportAll) {
            self.0.push(n.src.span);
        }
        fn visit_named_export(&mut self, n: &NamedExport) {
            if let Some(src) = &n.src {
                self.0.push(src.span);
            }
        }
        fn visit_call_expr(&mut self, n: &CallExpr) {
            if let Callee::Import(_) = n.callee {
                if let Some(Expr::Lit(Lit::Str(s))) = n.args.first().map(|arg| &*arg.expr) {
                    self.0.push(s.span);
                }
            }
            n.visit_children_with(self);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_scripts_to_their_outputs() {
        let cases = [
            ("main.ts", Some(("main.js", ScriptKind::TypeScript))),
            ("a/b/c.mts", Some(("a/b/c.mjs", ScriptKind::TypeScript))),
            ("plain.js", Some(("plain.js", ScriptKind::JavaScript))),
            ("plain.mjs", Some(("plain.mjs", ScriptKind::JavaScript))),
            ("types.d.ts", None),
            ("types.d.mts", None),
            ("view.tsx", None),
            ("style.css", None),
            (".hidden.ts", None),
            ("node_modules/pkg/index.js", None),
        ];
        for (input, expected) in cases {
            assert_eq!(
                output_path(Path::new(input)),
                expected.map(|(path, kind)| (PathBuf::from(path), kind)),
                "{input}"
            );
        }
    }

    #[cfg(feature = "compile-scripts")]
    mod compile {
        use super::super::compile;
        use pretty_assertions::assert_eq;
        use std::path::Path;

        fn strip(source: &str) -> String {
            compile(source, Path::new("scripts/main.ts")).unwrap()
        }

        #[test]
        fn strips_types_in_place() {
            let source = "import type { A } from \"./a.js\";\nimport { b, type C } from \"./b.js\";\nconst x: number = b as number;\nexport function f<T>(v: T): T { return v; }\n";
            let output = strip(source);
            assert_eq!(output.len(), source.len());
            assert_eq!(output.lines().count(), source.lines().count());
            assert!(output.contains("import { b"), "{output}");
            assert!(!output.contains("number"), "{output}");
            assert!(!output.contains("type"), "{output}");
            assert!(output.contains("export function f"), "{output}");
        }

        #[test]
        fn rewrites_local_typescript_specifiers() {
            let output = strip(
                "import { a } from \"./a.ts\";\nexport * from \"../b.mts\";\nexport { c } from '/js/c.ts';\nconst d = import(\"./d.ts\");\n",
            );
            assert_eq!(
                output,
                "import { a } from \"./a.js\";\nexport * from \"../b.mjs\";\nexport { c } from '/js/c.js';\nconst d = import(\"./d.js\");\n"
            );
        }

        #[test]
        fn leaves_other_specifiers_alone() {
            let source = "import { a } from \"./a.js\";\nimport \"pkg.ts\";\nimport \"//cdn.example/x.ts\";\nconst s = \"./not-an-import.ts\";\n";
            assert_eq!(strip(source), source);
        }

        #[test]
        fn reports_unerasable_syntax_with_its_position() {
            let failure = compile(
                "const ok = 1;\nenum Color { Red }\n",
                Path::new("scripts/bad.ts"),
            )
            .unwrap_err();
            assert!(
                failure.message.starts_with("scripts/bad.ts:2:1: "),
                "{}",
                failure.message
            );
            assert!(failure.message.contains("enum"), "{}", failure.message);
        }

        #[test]
        fn reports_syntax_errors_with_their_position() {
            let failure = compile("const = ;\n", Path::new("scripts/bad.ts")).unwrap_err();
            assert!(
                failure.message.starts_with("scripts/bad.ts:1:"),
                "{}",
                failure.message
            );
        }
    }
}
