#!/bin/bash
# Pictures of a codn that nobody sees, the same steps for every build, so
# that an old and a new build can be compared pixel by pixel: a headless
# labwc at scale 1, keys from wtype, the pointer from pointer.py, pictures
# from grim. Needs labwc, wtype, grim, zsh and python3. Takes about a
# minute. scripts/compare-looks.sh runs it for two builds.
#
# usage: scripts/shots.sh <codn binary> <fixture> <out dir> [render node]
# The fixture is what compare-looks.sh makes: the builds compared must see
# the same folders under the same names.
set -euo pipefail
usage="usage: shots.sh <codn binary> <fixture> <out dir> [render node]"
binary=$(realpath -- "${1:?$usage}"); F=$(realpath -- "${2:?$usage}"); out=${3:?$usage}; node=${4:-}
here=$(dirname "$(realpath -- "$0")")
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
mkdir -p "$out"; out=$(realpath -- "$out")
B=$(mktemp -d "${TMPDIR:-/tmp}/codn-shots.XXXXXX")
P= L= K= M= P_START= L_START= K_START= M_START=
birth() { awk '{print $22}' "/proc/$1/stat" 2>/dev/null || true; }
cleanup() {
    exec 9>&- 2>/dev/null || true
    for pair in "$P:$P_START" "$K:$K_START" "$M:$M_START" "$L:$L_START"; do
        pid=${pair%%:*}; at=${pair#*:}
        if [ -n "$pid" ] && [ -n "$at" ] && [ "$(birth "$pid")" = "$at" ]; then kill "$pid" 2>/dev/null || true; fi
    done
    rm -rf "$B"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$B/labwc" "$B/config/codn" "$B/bin"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$B/bin/notify-send"
chmod +x "$B/bin/notify-send"
export PATH="$B/bin:$PATH"
cd "$B"
cat > config/codn/config.toml <<EOF
roots = ["$F"]

[[agent]]
name = "zsh"
cmd = "/bin/zsh"
args = ["-f"]
mark = "\$"

[[agent]]
name = "other"
cmd = "/bin/sh"
icon = "sparkle"
resume = ["-c", ": {id}"]

[[agent.option]]
name = "Mode"
choices = [{ name = "default" }, { name = "verbose", args = ["-v"] }, { name = "trace", args = ["-x"] }]

[[agent.option]]
name = "Errors"
choices = [{ name = "default" }, { name = "stop", args = ["-e"] }]

[agent.history]
files = "$B/history/*.jsonl"
id = "id"
folder = "cwd"
title = ["title"]
EOF
# Twelve conversations of the second project, a year apart, so that their
# order and what the list says of their age are the same in every run.
mkdir -p "$B/history"
walk=0
for title in "Ford after rain" "Signs at the fork" "Cairns on the plateau" "Map the north ridge" \
    "Shelter roof" "Spring by the pass" "Scree below the col" "Winter route" "Count the waymarks" \
    "Bridge over the brook" "Hut logbook" "Descent to the lake"; do
    walk=$((walk+1))
    printf '{"id":"walk-%02d","cwd":"%s","title":"%s"}\n' "$walk" "$F/beta" "$title" > "$B/history/$walk.jsonl"
    touch -d "$((2000+walk))-01-01 12:00 UTC" "$B/history/$walk.jsonl"
done
{
    printf '%s\n' '#!/bin/bash'
    printf 'printf "%%s\n" "$WAYLAND_DISPLAY" > %q\n' "$B/display"
    printf 'printf "%%s\n" "$$" > %q\n' "$B/pid"
    printf 'exec '
    printf '%q ' env "XDG_STATE_HOME=$B/state" "XDG_CONFIG_HOME=$B/config" "PS1=%% " TZ=UTC "$binary" "$F/alpha"
    printf '2> %q\n' "$B/err"
} > run.sh
chmod +x run.sh
[ -n "$node" ] && export WLR_RENDER_DRM_DEVICE=$node
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 setsid nohup env -u WAYLAND_DISPLAY -u DISPLAY labwc -C "$B/labwc" -s "$B/run.sh" > labwc.log 2>&1 &
L=$!; L_START=$(birth "$L")
for _ in $(seq 1 100); do
    [ -s display ] && [ -s pid ] && break
    kill -0 "$L" 2>/dev/null || { cat labwc.log >&2; exit 1; }
    sleep 0.05
done
export WAYLAND_DISPLAY=$(cat display)
P=$(cat pid); P_START=$(birth "$P")
wtype -s 600000 &
K=$!; K_START=$(birth "$K")
mkfifo pointer
python3 "$here/pointer.py" 1280 720 < pointer &
M=$!; M_START=$(birth "$M")
exec 9> pointer
sleep 1
n=0
shot() { sleep "${2:-0.8}"; n=$((n+1)); grim "$out/$(printf %02d $n)-$1.png"; }
chord() { local k=$1; wtype -M ctrl -M shift "$k" -m shift -m ctrl; }
run() { wtype -d 2 "$1"; wtype -k Return; }
ptr() { printf '%s\n' "$@" >&9; }
# The second project comes through the socket of the running instance.
env "XDG_STATE_HOME=$B/state" "XDG_CONFIG_HOME=$B/config" "$binary" "$F/beta" 2>> "$B/err2" || true
shot empty 2.5
ptr "move 259 52" "click"; shot launch-by-click
wtype -k Return; sleep 1.2
run "PS1='%% '; clear"
run "cat $(printf %q "$F/screen.txt")"
shot session 2.5
# What a program reports about itself, in the bar along the bottom. One
# limit resets on the first day of 2100, a Friday, which the codn of these
# pictures reads in Greenwich time: the same words whenever they are taken.
run "printf 'model = \"demo-1\"\noptions = [\"high\", \"think\"]\ntokens = 124000\nwindow = 200000\nspeed = 78\n[limits]\n5h = 34\n7d = { used = 12.5, resets = 4102444800 }\n' > \$CODN_STATUS"
shot status 2.5
chord e; shot launch
wtype -k Down; shot launch-down
wtype -k Right; shot launch-options
wtype -k Escape; sleep 0.3; wtype -k Escape
# The list of the second project, whose agent has twelve conversations on
# disk: the five latest and a row for the rest, that row chosen, and a
# search through all of them.
ptr "move 259 116" "click"; shot earlier
for _ in 1 2 3 4 5 6 7; do wtype -k Down; done; wtype -k Return; shot earlier-all
wtype ford; shot earlier-search
wtype -k Escape; sleep 0.3; ptr "move 259 52"
chord o; shot add
wtype al; shot add-filter
wtype -k Escape; sleep 0.3; wtype -k Escape; sleep 0.3
chord d; shot diff 1.5
chord d; sleep 0.5
chord r; wtype name; shot rename
wtype -k Return; shot renamed
chord t; sleep 1; run "PS1='%% '; clear"; run "printf 'second https://example.org/a\n'"
shot second 2.5
# Control held with the pointer on the address that was printed, in the
# second row of the grid: it is underlined.
ptr "move 413 68"; wtype -M ctrl -s 1500 -m ctrl & held=$!
shot address 0.6; wait "$held"; sleep 0.2
wtype -M ctrl -k equal -m ctrl; shot zoom-in
wtype -M ctrl 0 -m ctrl; sleep 0.4
wtype -M ctrl -k minus -m ctrl; shot zoom-out
wtype -M ctrl 0 -m ctrl; sleep 0.4
# The pointer: over the rows of the sidebar, over the buttons of the window,
# over the head of the pane. The window is 1200 wide in the middle of 1280.
for at in "100 52" "100 80" "100 108" "100 144" "210 52" "210 80" "259 52" "259 144" "1037 19" "1066 19" "1094 19" "1130 18" "1174 18" "1218 18" "600 300"; do
    ptr "move $at"; shot "hover-${at// /-}" 0.4
done
ptr "move 1066 19" "click"; shot split 1.2
ptr "move 500 19"; shot split-hover-left 0.4
ptr "move 900 300" "click"; shot split-focus-right 0.5
chord d; shot split-diff 1.5
chord d; sleep 0.5
chord w; shot split-closed 0.6
# Drag a selection in the terminal.
wtype -M ctrl -k Tab -m ctrl; sleep 0.5
ptr "move 400 120" "click" "click"; shot select-word 0.5
ptr "move 400 160" "click" "click" "click"; shot select-line 0.5
ptr "wheel 3"; shot wheel-up 0.5
ptr "wheel -3"; shot wheel-down 0.5
ptr "move 80 706"; shot hover-add 0.4
ptr "click"; shot add-by-click 0.6
# What opened the list closes it, a click later.
sleep 0.6; ptr "click"; shot add-closed 0.6
wtype -k Escape; sleep 0.3
# The column with the files of the project, and a file in a pane beside
# the session: a folder opened, a word and more selected, the end of a
# line that is cut, another file, and the changes in the place of a file.
chord f; shot tree 1.2
ptr "move 1080 80"; shot tree-hover 0.4
ptr "click"; shot tree-folder 1
ptr "move 1080 108" "click"; shot file 1.2
ptr "move 800 110" "click" "click"; shot file-word 0.5
wtype -M shift -k Down -m shift; shot file-selection 0.5
wtype -k Up -k End; shot file-line-end 0.5
# Shift with the wheel moves the view sideways, here back from that end.
wtype -M shift -s 600 -m shift & sleep 0.2; ptr "wheel 1"; wait $!; shot file-sideways 0.5
# A file that is changed says so by its name, and another file does not
# take its place unasked: the lines at the bottom name the keys. Escape
# stays, and the change is taken back.
wtype x; shot file-changed 0.5
ptr "move 1080 136" "click"; shot file-asks 0.6
wtype -k Escape; wtype -M ctrl z -m ctrl; shot file-unchanged 0.5
ptr "move 1080 136" "click"; shot file-other 1
# What changed in a file since the last commit: the number of a marked
# line leads to the changes at that file, the row of a file there back.
ptr "move 1080 108" "click"; sleep 1
ptr "move 668 88"; shot file-number-hover 0.5
ptr "click"; shot file-to-changes 1.5
ptr "move 800 48"; shot changes-file-hover 0.4
ptr "click"; shot changes-to-file 1.2
chord d; shot file-gives-way 1.5
chord d; sleep 0.5
# A file of Markdown as it is meant, moved down, and its text through
# the switch and back. The changes take its place and go, so that what
# follows has the panes it had.
ptr "move 1080 164" "click"; shot markdown 1.5
ptr "move 800 300" "wheel -3"; shot markdown-down 0.5
chord m; shot markdown-text 0.8
chord m; shot markdown-meant-again 0.8
chord d; sleep 0.8; chord d; sleep 0.5
chord f; shot tree-folded 0.8
# The sidebar folded to a strip, a list that lies over the pane, and back.
chord b; shot sidebar-folded 0.8
chord e; shot sidebar-folded-list 0.8
wtype -k Escape; sleep 0.3
chord b; shot sidebar-whole 0.8
# What a program says it runs beside itself, as rows below its session:
# the records of OSC 7501 that have an id. None of them at work, whose
# mark would go round between two pictures. One lies below another, one
# has no title, and there are more than are shown; then some go, and all.
said() { run "printf '\\033]7501;$1\\033\\\\'"; }
titled() { printf %s "$1" | base64 -w0; }
said "state=blocked:kind=question:id=a:title=$(titled 'Explore the sidebar')"
said "state=idle:id=a/b:title=$(titled 'Read the docs')"
said "state=done:id=c"
shot subs 2.5
for i in 1 2 3 4 5 6 7 8; do said "state=idle:id=n$i:title=$(titled "Agent $i")"; done
shot subs-more 2.5
said "state=clear:id=a"; shot subs-fewer 2.5
said "state=clear"; shot subs-gone 2.5
cp "$B/err" "$out/err.txt" 2>/dev/null || true
echo "pictures: $out ($n)"
