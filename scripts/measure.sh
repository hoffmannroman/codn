#!/bin/bash
# Measures typing, paging and throughput in a codn that nobody sees: a
# headless labwc, keys from wtype, the numbers from CODN_TIMING. Needs labwc,
# wtype, zsh and python3. Takes about three minutes.
#
# usage: scripts/measure.sh <codn binary> [render node, e.g. /dev/dri/renderD129]
set -euo pipefail
binary=$(realpath -- "${1:?usage: measure.sh <codn binary> [render node]}")
[ -x "$binary" ] || { echo "not executable: $binary" >&2; exit 1; }
export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
B=$(mktemp -d "${TMPDIR:-/tmp}/codn-bench.XXXXXX")
P= L= K= P_START= L_START= K_START=
birth() { awk '{print $22}' "/proc/$1/stat" 2>/dev/null || true; }
cleanup() {
    for pair in "$P:$P_START" "$K:$K_START" "$L:$L_START"; do
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
# Keep the virtual keyboard registered for the entire measurement.
wtype -s 600000 &
K=$!; K_START=$(birth "$K")
sleep 0.5
Q=$(printf '%q' "$B")
echo "render nodes of codn: $(ls -l /proc/$P/fd | grep -oE 'renderD[0-9]+|nvidia[a-z0-9-]*' | sort | uniq -c | tr '\n' ' ' || true)"
grep -E -m2 'first frame|of that' err
cat > stats.py <<'EOF'
import sys,re,statistics as st
rows=[[float(x) for x in re.findall(r'\d+\.\d+',l)] for l in open(sys.argv[1]).read().split('\n')[int(sys.argv[2]):] if 'key to echo' in l]
def q(v,p): v=sorted(v); return v[min(len(v)-1,int(p*len(v)))]
print(f"  n={len(rows)}")
for i,name in enumerate(['total','answer','until frame','frame']):
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
stats() { python3 $B/stats.py $B/err $1; }
frames() { tail -n +$(($1+1)) $B/err | grep frames; }
newtab() { wtype -M ctrl -M shift t -m shift -m ctrl; sleep 0.6; }
run() { wtype -d 2 "$1"; wtype -k Return; }
clr() { wtype -M ctrl u -m ctrl; }
text=$(python3 -c "import random;random.seed(1);print(''.join(random.choice('abcdefghijklmnopqrstuvwxyz ') for _ in range(200)))")
for try in 1 2 3 4 5; do newtab; wtype x; sleep 0.4; grep -q "key to echo" err && break; done; echo "session after $try tries"; clr; sleep 1
m=$(mark); wtype -d 40 "$text"; sleep 0.5; echo "A1 zsh idle, key every 40 ms"; stats $m; clr
run cat; sleep 0.5
m=$(mark); wtype -d 40 "$text"; sleep 0.5; echo "A2 cat idle"; stats $m; wtype -k Return; wtype -M ctrl d -m ctrl; sleep 0.3
m=$(mark); wtype -d 5 "$text"; sleep 0.5; echo "A3 zsh, key every 5 ms"; stats $m; clr
wtype -k Return; run "clear; cat $Q/dense.txt"; sleep 3
m=$(mark); wtype -d 40 "$text"; sleep 0.5; echo "A4 zsh idle, screen full of colored text"; stats $m; frames $m | tail -1; clr; sleep 1
m=$(mark); for i in $(seq 1 120); do wtype -M shift -k Prior -m shift; done; sleep 1.2
echo "D paging up through colored text"; frames $m
wtype -k Return; sleep 0.3
m=$(mark); run "{ time cat $Q/dense.txt } 2> $Q/t_dense; { time seq 1 3000000 } 2> $Q/t_seq"; sleep 8; echo "E one session"; cat t_dense t_seq; frames $m
for i in 1 2 3; do newtab; run "timeout 22 sh -c 'while :; do seq 1 1000000; done'"; done
newtab; sleep 1
m=$(mark); wtype -d 40 "$text"; sleep 0.5; echo "B zsh, 3 hidden sessions flooding"; stats $m; clr
top -b -n1 -p $P | tail -1
sleep 14
run "timeout 14 sh -c 'while :; do seq 1 1000000; done' &"; sleep 1
m=$(mark); wtype -d 40 "$text"; sleep 0.5; echo "C the session itself floods (answer = any output)"; stats $m; frames $m
sleep 6; clr; sleep 1
rm -f go t4_*
for i in 1 2 3 4; do wtype -M ctrl -k Tab -m ctrl; sleep 0.4; clr; run "while [ ! -e $Q/go ]; do sleep 0.02; done; { time seq 1 3000000 } 2> $Q/t4_\$\$"; sleep 0.3; done
sleep 0.5; m=$(mark); touch go; sleep 10; echo "E four sessions at once"; cat t4_*; frames $m
sleep 4; a=$(awk '{print $14+$15}' /proc/$P/stat); sleep 5; b=$(awk '{print $14+$15}' /proc/$P/stat); echo "ticks in 5 s idle: $((b-a))"; grep -E 'VmRSS|RssAnon|Threads' /proc/$P/status
