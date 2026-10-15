#!/bin/bash
# Says whether two builds of codn look the same: the pictures of
# scripts/shots.sh at scale 1 and of scripts/shots-x11.sh at 1.25, 1.5
# and 2, taken of both and compared pixel by pixel. No test covers what is
# drawn, this stands in for one. Needs what the two scripts need, and
# git. Takes about a quarter of an hour.
#
# usage: scripts/compare-looks.sh <old codn binary> <new codn binary> [render node]
# SCALES="1 1.5" limits the scales, 1 being the one on Wayland.
#
# Builds made by scripts/dump-build.sh also write down the hits and a hash
# of the quads of every frame, and these are compared too. Pictures do not
# show the order of the hits, which decides what a click means. Read the
# counts with this in mind: a frame drawn in passing, while a program still
# prints or a list of folders arrives, is in one run and not in the other,
# also between two runs of one build. At scale 1 the hits came out equal
# run after run, and the quads equal or one frame apart. On X11 a third of
# the quads differ every time, and now and then a list of hits, that of
# the folders shown while a path is typed.
set -uo pipefail
old=$(realpath -- "${1:?usage: compare-looks.sh <old codn binary> <new codn binary> [render node]}")
new=$(realpath -- "${2:?usage: compare-looks.sh <old codn binary> <new codn binary> [render node]}")
node=${3:-}
here=$(dirname "$(realpath -- "$0")")
A=$(mktemp -d "${TMPDIR:-/tmp}/codn-looks.XXXXXX")
F=$A/fixture

# What both builds are shown: two projects, one with changes of every kind,
# a repository to be found, and text that uses what a terminal can draw.
commit() { git -C "$1" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false "${@:2}"; }
for project in alpha beta; do
    mkdir -p "$F/$project/src"
    git init -q -b main "$F/$project"
    printf 'fn main() {\n    println!("hello");\n}\n' > "$F/$project/src/main.rs"
    printf '# %s\n\nA fixture.\n' "$project" > "$F/$project/README.md"
    # A file of Markdown with what a preview draws, long enough to be
    # moved. Its name stands behind the others in the column.
    {
        printf '# A plan\n\nFlowing text with **bold**, *italic*, `code` and a [link](README.md),\n'
        printf 'long enough to be wrapped in a pane beside a session.\n\n'
        printf '| What | Count |\n| --- | ---: |\n| Rows | 12 |\n| Cells that are wrapped where the pane is narrow | 3 |\n\n'
        printf '```rust\nfn main() {\n    let name = "world";\n}\n```\n\n> A quote.\n\n'
        for step in $(seq 1 30); do printf -- '- [ ] step %s\n' "$step"; done
    } > "$F/$project/plan.md"
    commit "$F/$project" add -A
    GIT_AUTHOR_DATE=2026-01-01T00:00:00Z GIT_COMMITTER_DATE=2026-01-01T00:00:00Z commit "$F/$project" commit -q -m first
done
printf 'fn main() {\n    let name = "world";\n    println!("hello {name}");\n    // a long line that runs past the edge of a narrow diff view, to see how it is cut there, äöü → λ\n}\n' > "$F/alpha/src/main.rs"
printf 'new\n' > "$F/alpha/notes.txt"
rm "$F/alpha/README.md"
mkdir -p "$F/gamma/deep" && git init -q -b main "$F/gamma/deep"
python3 - "$F/screen.txt" <<'EOF'
import sys
o = ['plain \x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[1;3mboth\x1b[0m \x1b[4munder\x1b[0m \x1b[7minverse\x1b[0m \x1b[9mstrike\x1b[0m \x1b[2mdim\x1b[0m']
o.append(''.join(f'\x1b[48;5;{c}m  ' for c in range(16)) + '\x1b[0m')
o.append(''.join(f'\x1b[38;5;{c}m#' for c in range(16, 232, 3)) + '\x1b[0m')
o.append('┌──────┬──────┐  ╔══════╗  ╭────╮  ▁▂▃▄▅▆▇█  ░▒▓  ▏▎▍▌▋▊▉')
o.append('│ box  │ draw │  ║ dbl  ║  │ rd │  ▖▗▘▙▚▛▜▝▞▟')
o.append('└──────┴──────┘  ╚══════╝  ╰────╯  ┼ ┤ ├ ┬ ┴ ╳ ╱ ╲')
o.append('äöü ß → ← ↑ ↓ λ ∑ 日本語 한국어 😀 🚀 é ä')
o.append('\x1b[38;2;255;120;0mtruecolor\x1b[0m \x1b[4:3mcurly\x1b[0m \x1b]8;;https://example.invalid\x1b\\link\x1b]8;;\x1b\\')
o += [f'line {i:03d} ' + 'x' * (i * 13) for i in range(14)]
open(sys.argv[1], 'w').write('\n'.join(o) + '\n')
EOF

differs=0
for scale in ${SCALES:-1 1.25 1.5 2}; do
    for which in old new; do
        binary=$old; [ $which = new ] && binary=$new
        out=$A/$which-$scale
        mkdir -p "$out"
        if [ "$scale" = 1 ]; then
            CODN_DUMP=$out/frames.txt "$here/shots.sh" "$binary" "$F" "$out" $node > "$out/log.txt" 2>&1
        else
            CODN_DUMP=$out/frames.txt "$here/shots-x11.sh" "$binary" "$F" "$out" "$scale" > "$out/log.txt" 2>&1
        fi || { echo "scale $scale: taking the pictures of the $which build failed, see $out/log.txt"; differs=1; }
    done
    count=0; unequal=0
    for picture in "$A/old-$scale"/*.png; do
        [ -e "$picture" ] || continue
        name=$(basename "$picture"); count=$((count + 1))
        pixels=$(compare -metric AE "$picture" "$A/new-$scale/$name" null: 2>&1 | awk '{print $1}')
        if [ "$pixels" != 0 ]; then
            echo "scale $scale: $name differs ($pixels)"
            unequal=$((unequal + 1))
        fi
    done
    [ "$(ls "$A/new-$scale"/*.png 2>/dev/null | wc -l)" = "$count" ] || { echo "scale $scale: not the same number of pictures"; unequal=$((unequal + 1)); }
    [ $count = 0 ] && unequal=1
    echo "scale $scale: $((count - unequal)) of $count pictures equal"
    [ $unequal = 0 ] || differs=1
    # What the frames were made of, where the builds wrote it down.
    if [ -s "$A/old-$scale/frames.txt" ] && [ -s "$A/new-$scale/frames.txt" ]; then
        for kind in H:hits Q:quads; do
            a=$(grep "^${kind%%:*}" "$A/old-$scale/frames.txt" | sort -u)
            b=$(grep "^${kind%%:*}" "$A/new-$scale/frames.txt" | sort -u)
            echo "scale $scale: ${kind##*:} of $(comm -12 <(echo "$a") <(echo "$b") | wc -l) kinds of frame in both," \
                "$(comm -23 <(echo "$a") <(echo "$b") | grep -c .) only old, $(comm -13 <(echo "$a") <(echo "$b") | grep -c .) only new"
        done
    fi
done
echo "pictures and logs: $A"
exit $differs
