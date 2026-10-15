#!/bin/bash
# Measures what scripts/measure.sh cannot, because it has no pointer: the
# wheel, a split with one pane printing, and the memory of many sessions
# with full scrollbacks. Same setup: a headless labwc, keys from wtype, the
# pointer from scripts/pointer.py, the numbers from CODN_TIMING. Needs a
# codn that prints "wheel to frame" lines. Takes about two minutes.
#
# usage: scripts/measure-pointer.sh <codn binary> [render node, e.g. /dev/dri/renderD129]
set -euo pipefail
binary=$(realpath -- "${1:?usage: measure-pointer.sh <codn binary> [render node]}")
[ -x "$binary" ] || { echo "not executable: $binary" >&2; exit 1; }
here=$(dirname -- "$(realpath -- "$0")")
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
B=$(mktemp -d "${TMPDIR:-/tmp}/codn-bench.XXXXXX")
P= L= K= M= P_START= L_START= K_START= M_START=
birth() { awk '{print $22}' "/proc/$1/stat" 2>/dev/null || true; }
cleanup() {
    exec 9>&- 2>/dev/null || true
    for pair in "$P:$P_START" "$K:$K_START" "$M:$M_START" "$L:$L_START"; do
        pid=${pair%%:*}; at=${pair#*:}
        if [ -n "$pid" ] && [ -n "$at" ] && [ "$(birth "$pid")" = "$at" ]; then kill "$pid" 2>/dev/null || true; fi
    done
    echo "benchmark artifacts: $B"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
mkdir -p "$B/labwc" "$B/config/codn" "$B/proj" "$B/bin"
# Hidden sessions finishing must not notify the desktop this runs on.
printf '%s\n' '#!/bin/sh' 'exit 0' > "$B/bin/notify-send"
chmod +x "$B/bin/notify-send"
export PATH="$B/bin:$PATH"
cd "$B"
printf '[[agent]]\nname = "zsh"\ncmd = "/bin/zsh"\nargs = ["-f"]\nmark = "$"\n' > config/codn/config.toml
{
    printf '%s\n' '#!/bin/bash'
    printf 'printf "%%s\n" "$WAYLAND_DISPLAY" > %q\n' "$B/display"
    printf 'printf "%%s\n" "$$" > %q\n' "$B/pid"
    printf 'exec '
    printf '%q ' env "XDG_STATE_HOME=$B/state" "XDG_CONFIG_HOME=$B/config" CODN_TIMING=1 "$binary" "$B/proj"
    printf '2> %q\n' "$B/err"
} > run.sh
chmod +x run.sh
[ -n "${2:-}" ] && export WLR_RENDER_DRM_DEVICE=$2
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 setsid nohup env -u WAYLAND_DISPLAY -u DISPLAY labwc -C "$B/labwc" -s "$B/run.sh" > labwc.log 2>&1 &
L=$!; L_START=$(birth "$L")
for _ in $(seq 1 100); do
    [ -s display ] && [ -s pid ] && break
    kill -0 "$L" 2>/dev/null || { cat labwc.log >&2; exit 1; }
    sleep 0.05
done
export WAYLAND_DISPLAY=$(cat display)
P=$(cat pid); P_START=$(birth "$P")
# Keep the virtual keyboard and the pointer registered for the entire run.
wtype -s 600000 &
K=$!; K_START=$(birth "$K")
# The output of a headless labwc is 1280x720. The window is 1200 wide and
# stands in the middle of it, from the top edge.
mkfifo pointer
python3 "$here/pointer.py" 1280 720 < pointer &
M=$!; M_START=$(birth "$M")
exec 9> pointer
sleep 0.5
Q=$(printf '%q' "$B")
grep -E -m2 'first frame|of that' err
cat > stats.py <<'EOF'
import sys,re,statistics as st
what,names={'key':('key to echo',['total','answer','until frame','frame']),
            'wheel':('wheel to frame',['total','until frame','frame','turns'])}[sys.argv[3]]
rows=[[float(x) for x in re.findall(r'\d+(?:\.\d+)?',l)] for l in open(sys.argv[1]).read().split('\n')[int(sys.argv[2]):] if what in l]
def q(v,p): v=sorted(v); return v[min(len(v)-1,int(p*len(v)))]
print(f"  n={len(rows)}")
for i,name in enumerate(names):
    v=[r[i] for r in rows]
    if v: print(f"  {name:12} median {st.median(v):6.2f}  p95 {q(v,.95):6.2f}  max {max(v):6.2f}")
EOF
python3 - <<'EOF'
import random
random.seed(2)
words=['fn','let','match','self','render','frame','quad','glyph','atlas','äöü','→','x','terminal','0x7f','::','{','}']
with open('dense.txt','w') as f:
    for n in range(10000):
        line='';out=''
        while len(line)<170:
            w=random.choice(words); c=random.randint(16,231)
            s=random.choice(['','1;','3;','4;'])
            out+=f'\x1b[{s}38;5;{c}m{w}\x1b[0m '; line+=w+' '
        f.write(out+'\n')
EOF
mark() { wc -l < $B/err; }
stats() { python3 $B/stats.py $B/err $1 $2; }
frames() { tail -n +$(($1+1)) $B/err | grep frames || true; }
newtab() { wtype -M ctrl -M shift t -m shift -m ctrl; sleep 0.6; }
run() { wtype -d 2 "$1"; wtype -k Return; }
clr() { wtype -M ctrl u -m ctrl; }
point() { printf '%s\n' "$@" >&9; }
memory() { echo "$1: $(grep -E 'VmRSS|RssAnon' /proc/$P/status | tr -s '\t \n' ' ') $(grep -E '^Pss:' /proc/$P/smaps_rollup | tr -s ' ')"; }
flood='while :; do seq 1 1000000; done'
text=$(python3 -c "import random;random.seed(1);print(''.join(random.choice('abcdefghijklmnopqrstuvwxyz ') for _ in range(200)))")
for try in 1 2 3 4 5; do newtab; wtype x; sleep 0.4; grep -q "key to echo" err && break; done; echo "session after $try tries"; clr; sleep 1
memory "M one session, empty"
run "clear; cat $Q/dense.txt"; sleep 3
memory "M one session, full scrollback"
point 'move 700 300'; sleep 0.3

m=$(mark); point 'wheel 120 40'; sleep 5.5; echo "W1 wheel up through colored text, a notch every 40 ms"; stats $m wheel; frames $m | tail -1
m=$(mark); point 'wheel -120 40'; sleep 5.5; echo "W2 the same way down"; stats $m wheel
m=$(mark); point 'wheel 300 5'; sleep 2.5; echo "W3 a notch every 5 ms"; stats $m wheel; frames $m | tail -1
m=$(mark); point 'smooth -7 600 2'; sleep 2.5; echo "W4 as from a touchpad: 7 pixels every 2 ms"; stats $m wheel; frames $m | tail -1
m=$(mark); point 'wheel 3000 0'; sleep 1.5; echo "W5 3000 notches at once"; stats $m wheel
wtype -k Return; sleep 0.3
run "timeout 9 sh -c '$flood' &"; sleep 1
m=$(mark); point 'wheel 60 40' 'wheel -60 40'; sleep 5.5; echo "W6 the session prints without pause, a notch every 40 ms"; stats $m wheel; frames $m | tail -2
sleep 4; clr; sleep 0.5

# The button that splits is the second of the icons at the right of the
# header, at 1066. The first, at 1094, opens the column with the files:
# a click there measures a window with the column in it.
point 'move 1066 19' 'sleep 0.2' 'click'; sleep 1
m=$(mark); wtype x; sleep 0.4
tail -n +$((m+1)) err | grep -q "key to echo" || echo "the split did not open a session" >&2
clr
run "timeout 16 sh -c '$flood'"; sleep 1
wtype -M ctrl -M shift -k Left -m shift -m ctrl; sleep 0.4
m=$(mark); wtype -d 40 "$text"; sleep 0.5; echo "S1 split, the other pane prints without pause, key every 40 ms"; stats $m key; frames $m | tail -3; clr
point 'move 500 300'; sleep 0.2
m=$(mark); point 'wheel 60 40' 'wheel -60 40'; sleep 5.5; echo "S2 same, the wheel in the quiet pane, a notch every 40 ms"; stats $m wheel; frames $m | tail -2
sleep 3
wtype -M ctrl -M shift w -m shift -m ctrl; sleep 0.5

# Each new session fills its scrollback of 10,000 lines with colored text.
count=2
for upto in 5 10 20; do
    while [ $count -lt $upto ]; do newtab; run "clear; cat $Q/dense.txt"; sleep 0.8; count=$((count+1)); done
    sleep 2; memory "M $upto sessions, full scrollbacks"
done
sleep 4; a=$(awk '{print $14+$15}' /proc/$P/stat); sleep 5; b=$(awk '{print $14+$15}' /proc/$P/stat); echo "ticks in 5 s idle: $((b-a))"; grep -E 'Threads' /proc/$P/status
