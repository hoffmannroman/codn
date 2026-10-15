#!/bin/bash
# Fetches the sources of the grammars: the collection of `bat` at the
# commit named here, with the two collections it takes them from at the
# commits it names, and with its own corrections to them applied.
# Nothing of it is kept in the repository of codn: `codn-grammars pack`
# makes one file of those that sources.txt lists, and that file is.
# To take newer grammars, change the commit, run this and pack, and say
# so in assets/syntax/SOURCES.md.
#
# usage: tools/grammars/fetch.sh <folder that is not there yet>
set -euo pipefail
to=${1:?usage: fetch.sh <folder that is not there yet>}
commit=4608fc959aa8abf80d32198836511a570b7ae9ea
[ -e "$to" ] && { echo "$to is there already" >&2; exit 1; }
git init -q "$to"
git -C "$to" fetch -q --depth 1 https://github.com/sharkdp/bat "$commit"
git -C "$to" checkout -q FETCH_HEAD
git -C "$to" submodule update -q --init --depth 1 assets/syntaxes/01_Packages assets/syntaxes/02_Extra/TOML
# Of bat's corrections, those to grammars that are taken.
for name in JavaScript Makefile Markdown Python Rust ShellScript XML; do
    (cd "$to/assets" && patch --strip=0 -s < "patches/$name.sublime-syntax.patch")
done
git -C "$to" submodule status assets/syntaxes/01_Packages assets/syntaxes/02_Extra/TOML
echo "the grammars are under $to/assets/syntaxes"
