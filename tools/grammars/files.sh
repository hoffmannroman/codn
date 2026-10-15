#!/bin/bash
# Lists the files to measure with: what git tracks in the repositories
# under a folder, up to four levels down, each no larger than codn shows.
# Folders of builds and of dependencies are not tracked, so they are out.
#
# usage: tools/grammars/files.sh <folder> > list
set -euo pipefail
root=$(realpath -- "${1:?usage: files.sh <folder>}")
find "$root" -maxdepth 4 -name .git -not -path '*/node_modules/*' -not -path '*/vendor/*' -prune 2>/dev/null | while IFS= read -r git; do
    repo=${git%/.git}
    git -C "$repo" ls-files -z 2>/dev/null | while IFS= read -r -d '' file; do
        path=$repo/$file
        case $path in */node_modules/*|*/vendor/*) continue ;; esac
        [ -f "$path" ] && [ ! -L "$path" ] && [ -s "$path" ] && [ "$(stat -c %s -- "$path")" -le 8388608 ] && printf '%s\n' "$path"
    done
done
