//! Grammars for the colors of syntax, beside the build of codn.
//!
//! `fetch.sh` fetches their sources, in the format of Sublime Text.
//!
//! `pack <out> <folder> <list>` reads those of them that `<list>` names,
//! one a line, by where they lie in `<folder>`, and writes them as one
//! file, not packed and with no compiled patterns, as `syntect` reads it
//! back. `sources.txt` is the list, and `assets/syntax/grammars.packed`
//! in the repository of codn is what it made: see `SOURCES.md` there.
//!
//! `measure <packed> <list>` cuts the files named in `<list>`, one path a
//! line, into pieces with that file, as codn would, and says what it
//! cost: see `measure.rs`. `files.sh` makes such a list.

mod measure;

use std::collections::HashSet;
use std::path::Path;

use syntect::parsing::syntax_definition::{ContextReference, MatchOperation, Pattern};
use syntect::parsing::{Scope, SyntaxDefinition, SyntaxSetBuilder};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["pack", out, folder, list] => pack(Path::new(out), Path::new(folder), Path::new(list)),
        ["measure", packed, list] => measure::run(Path::new(packed), Path::new(list)),
        _ => Err("usage: codn-grammars pack <out> <folder of grammars> <list of grammars>\n       \
                  codn-grammars measure <packed> <list of files>"
            .into()),
    };
    if let Err(why) = done {
        eprintln!("{why}");
        std::process::exit(1);
    }
}

/// Whether a pattern leads into a grammar that is not among those
/// packed: it names one by its scope, to go on in it or to take its
/// patterns in.
fn leads_nowhere(pattern: &Pattern, known: &HashSet<Scope>) -> bool {
    let missing = |to: &ContextReference| matches!(to, ContextReference::ByScope { scope, .. } if !known.contains(scope));
    match pattern {
        Pattern::Include(to) => missing(to),
        Pattern::Match(found) => {
            let on = match &found.operation {
                MatchOperation::Push(to) | MatchOperation::Set(to) => to.iter().any(missing),
                MatchOperation::Pop | MatchOperation::None => false,
            };
            on || found.with_prototype.as_ref().is_some_and(missing)
        }
    }
}

/// Reads the grammars and writes them as one file. A line of a text is
/// handed to them with its end, which is what `true` says below.
fn pack(out: &Path, folder: &Path, list: &Path) -> Result<(), String> {
    let list = std::fs::read_to_string(list).map_err(|e| format!("{}: {e}", list.display()))?;
    let mut grammars = Vec::new();
    for line in list.lines().filter(|line| !line.is_empty() && !line.starts_with('#')) {
        let source = folder.join(line);
        let text = std::fs::read_to_string(&source).map_err(|e| format!("{}: {e}", source.display()))?;
        let name = source.file_stem().and_then(|name| name.to_str());
        let grammar = SyntaxDefinition::load_from_str(&text, true, name)
            .map_err(|e| format!("{}: {e}", source.display()))?;
        grammars.push(grammar);
    }
    // A grammar can lead into another: Markdown into the language of a
    // code block. Into one that is not packed, the crate fails at that
    // line of a file, every time. Such a pattern is taken out, and what
    // the grammar does with a language it does not know holds instead:
    // for Markdown, a code block in one color.
    let known: HashSet<Scope> = grammars.iter().map(|grammar| grammar.scope).collect();
    let mut builder = SyntaxSetBuilder::new();
    for mut grammar in grammars {
        let before: usize = grammar.contexts.values().map(|context| context.patterns.len()).sum();
        for context in grammar.contexts.values_mut() {
            context.patterns.retain(|pattern| !leads_nowhere(pattern, &known));
        }
        let after: usize = grammar.contexts.values().map(|context| context.patterns.len()).sum();
        if after < before {
            println!(
                "{}: {} patterns that lead into a grammar that is not here are out",
                grammar.name,
                before - after
            );
        }
        builder.add(grammar);
    }
    let set = builder.build();
    // Nothing may be left that names what is not there.
    let unlinked = set.find_unlinked_contexts();
    if !unlinked.is_empty() {
        return Err(unlinked.into_iter().map(|missing| format!("not there: {missing}\n")).collect());
    }
    syntect::dumps::dump_to_uncompressed_file(&set, out).map_err(|e| format!("{}: {e}", out.display()))?;
    let size = std::fs::metadata(out).map_or(0, |meta| meta.len());
    println!("{} grammars, {size} bytes", set.syntaxes().len());
    for grammar in set.syntaxes() {
        println!("{}: {}", grammar.name, grammar.file_extensions.join(" "));
    }
    Ok(())
}
