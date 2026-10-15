# Where the grammars come from

`grammars.packed` holds the grammars the colors of syntax are made
with, 24 of them, for nineteen languages. It is not written by hand:
`tools/grammars/fetch.sh` fetches the sources named here at the commits
named here, and `codn-grammars pack` makes the file of those that
`tools/grammars/sources.txt` lists. Packing the same sources twice gives
the same bytes. The test `every_pattern_of_every_grammar_compiles_with_the_engine_that_is_used`
in `src/syntax/mod.rs` counts the grammars and their patterns, 4,648.

To take a newer state or another language: change the commit in
`fetch.sh` or the list, fetch into an empty folder, pack, and change
this page, the endings in `src/syntax/mod.rs` and the count in the test.
A grammar is taken only with a license that says so.

```sh
tools/grammars/fetch.sh /tmp/grammar-sources
cd tools/grammars && cargo run --release -- pack ../../assets/syntax/grammars.packed \
    /tmp/grammar-sources/assets/syntaxes sources.txt
```

## The sources

All of them are taken through the collection of
[bat](https://github.com/sharkdp/bat) at
`4608fc959aa8abf80d32198836511a570b7ae9ea` (2026-10-01), which names
the two collections below at the commits given and holds the rest
itself. bat is under MIT or Apache-2.0, at the choice of who takes it,
copyright 2018-2023 bat-developers: `licenses/bat-MIT.txt`,
`licenses/bat-NOTICE.txt`.

| Grammar | For | From | License |
| --- | --- | --- | --- |
| JSON | `json`, `jsonl`, `jsonc`, `ipynb`, `webmanifest` | Packages | Packages |
| Python, Regular Expressions (Python) | `py`, `pyi` | Packages, corrected by bat | Packages |
| Markdown | `md`, `markdown`, `mdx` | Packages, corrected by bat, changed here | Packages |
| JavaScript, Regular Expressions (Javascript) | `js`, `jsx`, `mjs`, `cjs` | Packages, corrected by bat | Packages |
| HTML | `html`, `htm`, `astro` | Packages | Packages |
| CSS | `css` | Packages | Packages |
| C++ | `cpp`, `cc`, `cxx`, `hpp`, `hh`, `hxx`, `h` | Packages | Packages |
| C | `c` | Packages | Packages |
| Rust | `rs` | Packages, corrected by bat | MIT, `licenses/sublimehq-Packages-Rust.txt` |
| YAML | `yaml`, `yml` | Packages | MIT, copyright 2015 FichteFoll, `licenses/sublimehq-Packages-YAML.txt` |
| Bourne Again Shell (bash), Shell-Unix-Generic, commands-builtin-shell-bash | `sh`, `bash`, `zsh`, and files of a shell by their name | Packages, corrected by bat | Packages |
| XML | `xml`, `svg` | Packages, corrected by bat | Packages |
| SQL | `sql` | Packages | Packages |
| Diff | `diff`, `patch` | Packages | Packages |
| Makefile | `mk`, `Makefile` | Packages, corrected by bat | Packages |
| Regular Expression | inside patterns of the two above | Packages | Packages |
| TypeScript, TypeScriptReact | `ts`, `mts`, `cts`, `tsx` | bat, which made them from the grammars of TypeScript-Sublime-Plugin | Apache-2.0, `licenses/TypeScript-Sublime-Plugin.txt` |
| TOML | `toml`, and lock files by their name | sublime_toml_highlighting | MIT, copyright 2021 Jason Williams, `licenses/sublime_toml_highlighting.txt` |
| QML | `qml` | bat, which made it from the grammar of Sublime-QML | MIT, copyright 2013 Sergey Kozlov, `licenses/Sublime-QML.txt` |

- **Packages**: [sublimehq/Packages](https://github.com/sublimehq/Packages)
  at `759d6eed9b4beed87e602a23303a121c3a6c2fb3`, the grammars of
  Sublime Text in the format `syntect` 5.3.0 reads. Its license,
  `licenses/sublimehq-Packages.txt`, grants to copy, use, modify, sell
  and distribute, and makes an exception for files that carry a license
  of their own: of those taken, Rust and YAML do.
- **TypeScript-Sublime-Plugin**:
  [Microsoft/TypeScript-Sublime-Plugin](https://github.com/Microsoft/TypeScript-Sublime-Plugin)
  at `ba45efd058df5111837e30fb9598cfc8cbd51095`. bat holds the two
  grammars in the format that is read, as
  `assets/syntaxes/02_Extra/TypeScript.sublime-syntax` and
  `TypsecriptReact.sublime-syntax`.
- **sublime_toml_highlighting**:
  [jasonwilliams/sublime_toml_highlighting](https://github.com/jasonwilliams/sublime_toml_highlighting)
  at `f5a57e8bff694a4e6c52a491dae579aabc7427cf`.
- **Sublime-QML**: [skozlovf/Sublime-QML](https://github.com/skozlovf/Sublime-QML)
  at `e3e058a21f3c44d64ef5eda93925ee84169b62ec`. bat holds the grammar
  in the format that is read, as `assets/syntaxes/02_Extra/QML.sublime-syntax`.

## What is changed

- **By bat**, before the grammars are packed: its corrections to
  JavaScript, Makefile, Markdown, Python, Rust, the shell and XML,
  `assets/patches/` there. `fetch.sh` applies them.
- **Here**, while they are packed: Markdown goes into the grammar of
  the language a code block names. Eleven of its patterns lead into
  grammars that are not among these (C#, Go, Graphviz, Java,
  Objective-C and Objective-C++, PHP in two ways, Puppet, R, Ruby), and
  the crate fails at the line where such a block begins. Those eleven
  patterns are taken out, so that such a block is text in one color, as
  a block in a language Markdown does not know is. Nothing else is
  changed.

## Looked at and not taken

- **INI**, for the units of systemd and files of settings: its origin,
  `clintberry/sublime-text-2-ini`, names no license.
- **Astro and MDX**: there is a grammar for each only in a newer
  format that `syntect` 5.3.0 does not read. A file of Astro is shown
  as HTML, its head between the dashes wrongly, and MDX as Markdown.
- The other languages bat holds. These nineteen are what was counted
  in a set of projects on 2026-10-07, with Diff and Makefile.
