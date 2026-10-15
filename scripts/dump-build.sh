#!/bin/bash
# Builds a codn that writes down what every frame was made of, for
# scripts/compare-looks.sh: a line with the hits as they were recorded,
# and a line with the number of quads and a hash of them, appended to the
# file that CODN_DUMP names. The lines are added to a copy of the sources,
# never to the repository: nothing of it is in a build anyone uses.
#
# usage: scripts/dump-build.sh <revision, or . for the working tree> <build dir>
# Prints the path of the binary. Both builds of a comparison are made
# this way, each into a build dir of its own.
set -euo pipefail
revision=${1:?usage: dump-build.sh <revision, or . for the working tree> <build dir>}
build=${2:?usage: dump-build.sh <revision, or . for the working tree> <build dir>}
repo=$(dirname "$(dirname "$(realpath -- "$0")")")
mkdir -p "$build"; build=$(realpath -- "$build")
rm -rf "$build/src"; mkdir "$build/src"
cd "$repo"
if [ "$revision" = . ]; then
    cp -r src assets Cargo.toml Cargo.lock rustfmt.toml "$build/src/"
else
    git archive "$revision" src assets Cargo.toml Cargo.lock rustfmt.toml | tar -x -C "$build/src"
fi
# The patched libraries are taken as they are in the working tree.
ln -s "$repo/vendor" "$build/src/vendor"
python3 - "$build/src" <<'EOF'
import sys
root = sys.argv[1]

def insert(path, anchor, lines, before=False):
    text = open(root + path).read()
    assert text.count(anchor) == 1, f'{path}: the place for the dump is not there any more'
    text = text.replace(anchor, lines + anchor if before else anchor + lines)
    open(root + path, 'w').write(text)

insert('/src/ui/mod.rs', '    ui.hits = hits;\n', '''    if let Some(path) = std::env::var_os("CODN_DUMP") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
            let _ = writeln!(file, "H {:?}", ui.hits);
        }
    }
''')
insert('/src/render/gl.rs', '        let c = clear.map(|v| v as f32 / 255.0);\n', '''        if let Some(path) = std::env::var_os("CODN_DUMP") {
            use std::hash::{Hash, Hasher};
            use std::io::Write;
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hasher);
            if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
                let _ = writeln!(file, "Q {} {:016x}", instances.len(), hasher.finish());
            }
        }
''', before=True)
EOF
cargo build --release --manifest-path "$build/src/Cargo.toml" --target-dir "$build/target" >&2
echo "$build/target/release/codn"
