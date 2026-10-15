//! The colors of syntax: grammars that cut a line of a file into pieces
//! with names. The only place that knows `syntect`.
//!
//! `cutter` is the process that does the cutting, codn itself started
//! with `ARGUMENT`, `client` the thread of the app that asks it, and
//! `wire` what the two say to each other. Here are the grammars and which
//! of them shows which file.

pub mod client;
mod cutter;
mod outline;
mod wire;

use std::ffi::OsStr;
use std::path::Path;

use syntect::parsing::SyntaxSet;

pub use cutter::child;

/// What codn is started with to be the process for the colors.
pub const ARGUMENT: &str = "--syntax";

/// The grammars as `tools/grammars` packed them, put in as they are.
/// Where each comes from is in `assets/syntax/SOURCES.md`.
static PACKED: &[u8] = include_bytes!("../../assets/syntax/grammars.packed");

/// The grammar that shows a file, by the ending of its name without
/// regard to case. The grammars name endings themselves, and two of them
/// the same ones, so which is taken is said here and nowhere else. Astro
/// is shown as HTML and MDX as Markdown: no grammar for either can be
/// read. A header is taken for C++.
const BY_ENDING: [(&str, &str); 44] = [
    ("json", "JSON"),
    ("jsonl", "JSON"),
    ("jsonc", "JSON"),
    ("ipynb", "JSON"),
    ("webmanifest", "JSON"),
    ("py", "Python"),
    ("pyi", "Python"),
    ("ts", "TypeScript"),
    ("mts", "TypeScript"),
    ("cts", "TypeScript"),
    ("tsx", "TypeScriptReact"),
    ("md", "Markdown"),
    ("markdown", "Markdown"),
    ("mdx", "Markdown"),
    ("js", "JavaScript"),
    ("jsx", "JavaScript"),
    ("mjs", "JavaScript"),
    ("cjs", "JavaScript"),
    ("html", "HTML"),
    ("htm", "HTML"),
    ("astro", "HTML"),
    ("css", "CSS"),
    ("cpp", "C++"),
    ("cc", "C++"),
    ("cxx", "C++"),
    ("hpp", "C++"),
    ("hh", "C++"),
    ("hxx", "C++"),
    ("h", "C++"),
    ("c", "C"),
    ("rs", "Rust"),
    ("yaml", "YAML"),
    ("yml", "YAML"),
    ("sh", SHELL),
    ("bash", SHELL),
    ("zsh", SHELL),
    ("xml", "XML"),
    ("svg", "XML"),
    ("toml", "TOML"),
    ("qml", "QML"),
    ("sql", "SQL"),
    ("diff", "Diff"),
    ("patch", "Diff"),
    ("mk", "Makefile"),
];

/// The same by the whole name, for files that are told by it.
const BY_NAME: [(&str, &str); 12] = [
    ("Makefile", "Makefile"),
    ("makefile", "Makefile"),
    ("GNUmakefile", "Makefile"),
    ("Cargo.lock", "TOML"),
    ("uv.lock", "TOML"),
    ("poetry.lock", "TOML"),
    ("Pipfile", "TOML"),
    ("PKGBUILD", SHELL),
    (".bashrc", SHELL),
    (".zshrc", SHELL),
    (".zshenv", SHELL),
    (".profile", SHELL),
];

const SHELL: &str = "Bourne Again Shell (bash)";

/// Reads the grammars. Nothing of them is unpacked or compiled before a
/// language is used.
fn grammars() -> Option<SyntaxSet> {
    syntect::dumps::from_uncompressed_data(PACKED).ok()
}

/// The name of the grammar that shows a file of this name, if there is one.
pub fn language(name: &OsStr) -> Option<&'static str> {
    let name = name.to_str()?;
    if let Some((_, grammar)) = BY_NAME.iter().find(|(whole, _)| *whole == name) {
        return Some(grammar);
    }
    let ending = Path::new(name).extension()?.to_str()?.to_lowercase();
    BY_ENDING.iter().find(|(known, _)| *known == ending).map(|(_, grammar)| *grammar)
}

#[cfg(test)]
mod tests {
    use syntect::parsing::syntax_definition::Pattern;
    use syntect::parsing::{ParseState, Regex, Region, ScopeStack, ScopeStackOp};

    use super::*;

    /// Cuts a text into pieces and returns the names that were given,
    /// by line. Panics where the crate could not cut a line or the names
    /// do not nest.
    fn names(set: &SyntaxSet, grammar: &str, text: &str) -> Vec<Vec<String>> {
        let syntax = set.find_syntax_by_name(grammar).unwrap_or_else(|| panic!("no grammar {grammar}"));
        let (mut state, mut stack) = (ParseState::new(syntax), ScopeStack::new());
        let mut lines = Vec::new();
        for line in text.split_inclusive('\n') {
            let pieces = state.parse_line(line, set).unwrap_or_else(|e| panic!("{grammar}: {line:?}: {e}"));
            let mut given = Vec::new();
            for (_, change) in &pieces {
                if let ScopeStackOp::Push(scope) = change {
                    given.push(scope.build_string());
                }
                stack.apply(change).unwrap_or_else(|e| panic!("{grammar}: {line:?}: {e}"));
            }
            lines.push(given);
        }
        lines
    }

    #[test]
    fn the_grammars_are_read_and_every_file_that_is_named_has_one() {
        let set = grammars().expect("the file of grammars fits the crate");
        assert!(set.find_unlinked_contexts().is_empty(), "a grammar names one that is not there");
        for (known, grammar) in BY_ENDING.iter().chain(&BY_NAME) {
            assert!(set.find_syntax_by_name(grammar).is_some(), "{known}: no grammar {grammar}");
        }
        let named = |name: &str| language(OsStr::new(name));
        assert_eq!(named("a.rs"), Some("Rust"));
        assert_eq!(named("App.TSX"), Some("TypeScriptReact"), "without regard to case");
        assert_eq!(named("page.astro"), Some("HTML"));
        assert_eq!(named("post.mdx"), Some("Markdown"));
        assert_eq!((named("x.mjs"), named("x.jsx")), (Some("JavaScript"), Some("JavaScript")));
        assert_eq!((named("log.jsonl"), named("icon.svg")), (Some("JSON"), Some("XML")));
        assert_eq!((named("store.h"), named("store.c")), (Some("C++"), Some("C")));
        assert_eq!(
            (named("Makefile"), named("Cargo.lock"), named(".zshrc")),
            (Some("Makefile"), Some("TOML"), Some(SHELL))
        );
        // A name without an ending, an ending without a grammar, a name
        // that is no text.
        assert_eq!((named("LICENSE"), named("a.png"), named("rs"), named(".rs")), (None, None, None, None));
        assert_eq!(language(std::os::unix::ffi::OsStrExt::from_bytes(b"a\xff.rs")), None);
    }

    /// A pattern is compiled when a file first needs it, and one that
    /// does not compile ends the process there. So each is compiled here,
    /// with the engine the build uses, and every context is unpacked. A
    /// pattern that takes a piece of the file in is compiled here without
    /// one: with pieces, in the tests of the cutter. And a grammar must
    /// not lead where there is nothing, by any of the four ways it can:
    /// the crate's own check looks at two of them.
    #[test]
    fn every_pattern_of_every_grammar_compiles_with_the_engine_that_is_used() {
        use syntect::parsing::syntax_definition::{ContextReference, MatchOperation};
        let set = grammars().unwrap();
        let (mut grammars, mut patterns) = (0, 0);
        let resolved = |to: &ContextReference| matches!(to, ContextReference::Direct { .. });
        let broken = |pattern: &str| Regex::try_compile(pattern).map(|why| format!("{why}: {pattern}"));
        assert!(broken("(a").is_some(), "the check itself tells a pattern that does not compile");
        for grammar in set.into_builder().syntaxes() {
            grammars += 1;
            if let Some(first) = &grammar.first_line_match {
                assert_eq!(broken(first), None, "{}", grammar.name);
            }
            for (context, held) in &grammar.contexts {
                for pattern in &held.patterns {
                    let pattern = match pattern {
                        Pattern::Match(pattern) => pattern,
                        Pattern::Include(to) => {
                            assert!(resolved(to), "{} in {context} takes in {to:?}", grammar.name);
                            continue;
                        }
                    };
                    patterns += 1;
                    let on = match &pattern.operation {
                        MatchOperation::Push(to) | MatchOperation::Set(to) => to.iter().all(resolved),
                        MatchOperation::Pop | MatchOperation::None => true,
                    };
                    let over = pattern.with_prototype.as_ref().is_none_or(resolved);
                    assert!(on && over, "{} in {context} leads nowhere: {pattern:?}", grammar.name);
                    // One that takes in a piece of the file is compiled
                    // at every use, with the piece escaped. Here with none.
                    let text = if pattern.has_captures {
                        pattern.regex_with_refs(&Region::new(), "").regex_str().to_owned()
                    } else {
                        pattern.regex.regex_str().to_owned()
                    };
                    assert_eq!(broken(&text), None, "{} in {context}", grammar.name);
                }
            }
        }
        // Whoever packs other grammars sees here how many there are.
        assert_eq!((grammars, patterns), (24, 4648));
    }

    /// That a grammar can be read does not say that it works: keys the
    /// crate does not know are passed over. So each language is tried,
    /// and has to name what every color scheme tells apart.
    #[test]
    fn a_text_of_every_language_is_cut_into_pieces_with_the_names_expected() {
        let set = grammars().unwrap();
        let tried: [(&str, &str, &[&str]); 19] = [
            ("JSON", "{\"a\": [1, true]}\n", &["string", "constant.numeric", "constant.language"]),
            (
                "Python",
                "def f(x):  # note\n    return \"s\" + 1\n",
                &["comment", "string", "storage.type", "keyword"],
            ),
            (
                "TypeScript",
                "const x: number = 1; // note\nlet s = \"a\";\n",
                &["comment", "string", "storage.type"],
            ),
            (
                "TypeScriptReact",
                "const a = <div className=\"x\">hi</div>; // note\n",
                &["comment", "string", "entity.name.tag"],
            ),
            (
                "Markdown",
                "# Head\n\nsome *text* and `code`\n",
                &["markup.heading", "markup.italic", "markup.raw"],
            ),
            (
                "JavaScript",
                "function f() { return \"s\"; } // note\n",
                &["comment", "string", "storage.type"],
            ),
            (
                "HTML",
                "<!-- note -->\n<p class=\"x\">hi</p>\n<style>a { color: red; }</style>\n",
                &["comment", "string", "entity.name.tag", "source.css"],
            ),
            (
                "CSS",
                "/* note */\na { color: red; }\n",
                &["comment", "entity.name.tag", "support.type.property-name"],
            ),
            (
                "C++",
                "// note\nint main() { return 0; }\nconst char* s = \"x\";\n",
                &["comment", "string", "storage.type", "keyword"],
            ),
            (
                "C",
                "// note\nint main() { return 0; }\nconst char* s = \"x\";\n",
                &["comment", "string", "storage.type", "keyword"],
            ),
            ("Rust", "// note\nfn main() { let s = \"x\"; }\n", &["comment", "string", "storage.type"]),
            ("YAML", "# note\nkey: \"value\"\n", &["comment", "string", "entity.name.tag"]),
            (SHELL, "# note\necho \"hi\"\n", &["comment", "string"]),
            ("XML", "<!-- note -->\n<a b=\"c\"/>\n", &["comment", "string", "entity.name.tag"]),
            ("TOML", "# note\nkey = \"value\"\n", &["comment", "string"]),
            ("QML", "// note\nItem { property string s: \"x\" }\n", &["comment", "string"]),
            ("SQL", "-- note\nSELECT 'x' FROM t;\n", &["comment", "string", "keyword"]),
            ("Diff", "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n", &["markup.inserted", "markup.deleted"]),
            ("Makefile", "# note\nall:\n\techo \"x\"\n", &["comment", "entity.name.function"]),
        ];
        for (grammar, text, expected) in tried {
            let given = names(&set, grammar, text).concat();
            for name in expected {
                assert!(
                    given.iter().any(|given| given.starts_with(name)),
                    "{grammar}: no {name} in {given:?}"
                );
            }
        }
        // Every grammar that shows a file was tried.
        for (known, grammar) in BY_ENDING.iter().chain(&BY_NAME) {
            assert!(tried.iter().any(|(tried, ..)| tried == grammar), "{known}: {grammar} was not tried");
        }
    }

    /// Markdown goes into the grammar of the language a code block names.
    /// Into one that is not packed the crate fails at that line, so those
    /// ways are taken out when the grammars are packed: see `pack` in
    /// `tools/grammars`.
    #[test]
    fn a_code_block_in_a_language_without_a_grammar_is_text_and_what_follows_is_cut() {
        let set = grammars().unwrap();
        let any = |line: &[String], name: &str| line.iter().any(|given| given.starts_with(name));
        for language in
            ["java", "go", "ruby", "php", "cs", "objc", "r", "dot", "puppet", "nothing-at-all", ""]
        {
            let text = format!("```{language}\nint x = \"s\";\n```\n# Head\n");
            let lines = names(&set, "Markdown", &text);
            assert!(!any(&lines[1], "string"), "{language}: {:?}", lines[1]);
            assert!(any(&lines[3], "markup.heading"), "{language}: {:?}", lines[3]);
        }
        // One that is packed is cut by its own grammar.
        let lines = names(&set, "Markdown", "```rust\nlet s = \"x\"; // note\n```\n# Head\n");
        assert!(any(&lines[1], "string") && any(&lines[1], "comment"), "{:?}", lines[1]);
        assert!(any(&lines[3], "markup.heading"));
    }
}
