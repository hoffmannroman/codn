#!/bin/bash
# Pictures of a codn on an X server that nobody sees, at any scale: the
# same steps for every build, so that an old and a new build can be
# compared pixel by pixel. Xvfb, keys and pointer from xdotool, pictures
# from ImageMagick's import, drawn by Mesa in software. Needs Xvfb,
# xdotool, ImageMagick, zsh and python3. Takes about two minutes.
# scripts/compare-looks.sh runs it for two builds.
#
# usage: scripts/shots-x11.sh <codn binary> <fixture> <out dir> <scale>
set -euo pipefail
usage="usage: shots-x11.sh <codn binary> <fixture> <out dir> <scale>"
binary=$(realpath -- "${1:?$usage}"); F=$(realpath -- "${2:?$usage}"); out=${3:?$usage}; scale=${4:?$usage}
mkdir -p "$out"; out=$(realpath -- "$out")
B=$(mktemp -d "${TMPDIR:-/tmp}/codn-xshots.XXXXXX")
X= P= X_START= P_START=
birth() { awk '{print $22}' "/proc/$1/stat" 2>/dev/null || true; }
cleanup() {
    for pair in "$P:$P_START" "$X:$X_START"; do
        pid=${pair%%:*}; at=${pair#*:}
        if [ -n "$pid" ] && [ -n "$at" ] && [ "$(birth "$pid")" = "$at" ]; then kill "$pid" 2>/dev/null || true; fi
    done
    rm -rf "$B"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$B/config/codn" "$B/bin"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$B/bin/notify-send"
chmod +x "$B/bin/notify-send"
export PATH="$B/bin:$PATH"
cat > "$B/config/codn/config.toml" <<EOF
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
exec 8> "$B/displayfd"
Xvfb -displayfd 8 -screen 0 2500x1600x24 -nolisten tcp > "$B/xvfb.log" 2>&1 &
X=$!; X_START=$(birth "$X")
for _ in $(seq 1 100); do [ -s "$B/displayfd" ] && break; sleep 0.05; done
export DISPLAY=:$(cat "$B/displayfd")
env -u WAYLAND_DISPLAY "XDG_STATE_HOME=$B/state" "XDG_CONFIG_HOME=$B/config" "WINIT_X11_SCALE_FACTOR=$scale" \
    LIBGL_ALWAYS_SOFTWARE=1 TZ=UTC "$binary" "$F/alpha" 2> "$B/err" &
P=$!; P_START=$(birth "$P")
for _ in $(seq 1 100); do xdotool search --name '^codn$' > "$B/win" 2>/dev/null && [ -s "$B/win" ] && break; sleep 0.1; done
win=$(head -1 "$B/win")
xdotool windowmove "$win" 0 0 2>/dev/null || true
xdotool windowfocus "$win"
sleep 1
n=0
shot() { sleep "${2:-0.8}"; n=$((n+1)); import -window root -crop "$(xdotool getwindowgeometry "$win" | awk '/Geometry/{print $2}')+0+0" +repage "$out/$(printf %02d $n)-$1.png"; }
key() { xdotool key --clearmodifiers "$@"; }
run() { xdotool type --delay 2 "$1"; xdotool key Return; }
# A point given in logical pixels of the window.
at() { xdotool mousemove "$(python3 -c "print(round($1*$scale))")" "$(python3 -c "print(round($2*$scale))")"; }
click() { xdotool click 1; }
env -u WAYLAND_DISPLAY "XDG_STATE_HOME=$B/state" "XDG_CONFIG_HOME=$B/config" "$binary" "$F/beta" 2>> "$B/err2" || true
shot empty 2.5
at 219 52; click; shot launch-by-click
key Return; sleep 1.5
run "PS1='%% '; clear"
run "cat $(printf %q "$F/screen.txt")"
shot session 2.5
# What a program reports about itself, in the bar along the bottom. One
# limit resets on the first day of 2100, a Friday, which the codn of these
# pictures reads in Greenwich time: the same words whenever they are taken.
run "printf 'model = \"demo-1\"\noptions = [\"high\", \"think\"]\ntokens = 124000\nwindow = 200000\nspeed = 78\n[limits]\n5h = 34\n7d = { used = 12.5, resets = 4102444800 }\n' > \$CODN_STATUS"
shot status 2.5
key ctrl+shift+e; shot launch
key Down; shot launch-down
key Right; shot launch-options
key Right; shot launch-option-next
key Down; shot launch-option-second
key Escape; sleep 0.3; key Escape
# The list of the second project, whose agent has twelve conversations on
# disk: the five latest and a row for the rest, that row chosen, and a
# search through all of them.
at 219 116; click; shot earlier
key Down Down Down Down Down Down Down Return; shot earlier-all
xdotool type ford; shot earlier-search
key Escape; sleep 0.3; at 219 52
key ctrl+shift+o; shot add
xdotool type al; shot add-filter
key Escape; sleep 0.3; key Escape; sleep 0.3
key ctrl+shift+o; xdotool type "$F/"; shot add-browse 1.2
key Escape; sleep 0.3; key Escape; sleep 0.3
key ctrl+shift+d; shot diff 1.5
key ctrl+shift+d; sleep 0.5
key ctrl+shift+r; xdotool type name; shot rename
key Return; shot renamed
key ctrl+shift+t; sleep 1; run "PS1='%% '; clear"; run "printf 'second https://example.org/a\n'"
shot second 2.5
# Control held with the pointer on the address that was printed, in the
# second row of the grid: it is underlined.
at 373 68; xdotool keydown ctrl; shot address 0.5; xdotool keyup ctrl; sleep 0.2
key ctrl+equal; shot zoom-in
key ctrl+0; sleep 0.4
for p in "60 52" "60 80" "60 108" "60 144" "170 52" "170 80" "219 52" "219 144" "997 19" "1026 19" "1054 19" "1090 18" "1134 18" "1178 18" "560 300"; do
    at $p; shot "hover-${p// /-}" 0.4
done
at 1026 19; click; shot split 1.2
at 460 19; shot split-hover-left 0.4
at 860 300; click; shot split-focus-right 0.5
key ctrl+shift+d; shot split-diff 1.5
key ctrl+shift+d; sleep 0.5
key ctrl+shift+w; shot split-closed 0.6
at 360 120; click; click; shot select-word 0.5
xdotool click 4; xdotool click 4; shot wheel-up 0.5
at 100 300; xdotool click 5; xdotool click 5; shot sidebar-wheel 0.5
at 40 746; shot hover-add 0.4
click; shot add-by-click 0.6
# What opened the list closes it, a click later.
sleep 0.6; click; shot add-closed 0.6
key Escape; sleep 0.3
# The column with the files of the project, and a file in a pane beside
# the session: a folder opened, a word and more selected, the end of a
# line that is cut, another file, and the changes in the place of a file.
key ctrl+shift+f; shot tree 1.2
at 1040 80; shot tree-hover 0.4
click; shot tree-folder 1
at 1040 108; click; shot file 1.2
at 760 110; click; click; shot file-word 0.5
key shift+Down; shot file-selection 0.5
key Up End; shot file-line-end 0.5
# Shift with the wheel moves the view sideways, here back from that end.
xdotool keydown shift click 4 keyup shift; shot file-sideways 0.5
# A file that is changed says so by its name, and another file does not
# take its place unasked: the lines at the bottom name the keys. Escape
# stays, and the change is taken back.
xdotool type x; shot file-changed 0.5
at 1040 136; click; shot file-asks 0.6
key Escape; key ctrl+z; shot file-unchanged 0.5
at 1040 136; click; shot file-other 1
# What changed in a file since the last commit: the number of a marked
# line leads to the changes at that file, the row of a file there back.
at 1040 108; click; sleep 1
at 628 88; shot file-number-hover 0.5
click; shot file-to-changes 1.5
at 760 48; shot changes-file-hover 0.4
click; shot changes-to-file 1.2
key ctrl+shift+d; shot file-gives-way 1.5
key ctrl+shift+d; sleep 0.5
# A file of Markdown as it is meant, moved down, and its text through
# the switch and back. The changes take its place and go, so that what
# follows has the panes it had.
at 1040 164; click; shot markdown 1.5
at 760 300; xdotool click 5; xdotool click 5; xdotool click 5; shot markdown-down 0.5
key ctrl+shift+m; shot markdown-text 0.8
key ctrl+shift+m; shot markdown-meant-again 0.8
key ctrl+shift+d; sleep 0.8; key ctrl+shift+d; sleep 0.5
key ctrl+shift+f; shot tree-folded 0.8
# What is typed first after the keys with Control above comes with
# Control still held: a `p` brought the last command back, seen three
# times in three runs. The `e` that `exit 3` lost here now and then may
# have gone the same way. A space takes that, and does nothing.
at 600 300; click; xdotool type ' '; sleep 0.2
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
run "exit 3"; shot ended 1.5
# The sidebar folded to a strip by its key, a list that lies over the
# pane, and back by its icon, the first of those at the top right.
key ctrl+shift+b; shot sidebar-folded 0.8
key ctrl+shift+e; shot sidebar-folded-list 0.8
key Escape; sleep 0.3
at 977 19; click; shot sidebar-whole 0.8
cp "$B/err" "$out/err.txt" 2>/dev/null || true
echo "pictures: $out ($n)"
