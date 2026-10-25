#!/bin/sh
# Regenerate docs/media/demo.gif.
#
#   scripts/demo/make-demo.sh
#
# Copies the sample project in scripts/demo/project into a fresh temporary git
# repository, runs Sakshi there with `demo-agent` (a scripted stand-in for a
# coding agent that really edits the files and reports through the IPC
# protocol), presses keys on the schedule in schedule.json, and renders the
# recording with agg (https://github.com/asciinema/agg).
#
# Needs: cargo, git, python3, agg. macOS/Linux only (the stand-in agent talks to
# Sakshi's Unix socket).

set -eu

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
out="${1:-$root/docs/media/demo.gif}"

command -v agg >/dev/null 2>&1 || { echo "need agg: cargo install --git https://github.com/asciinema/agg" >&2; exit 1; }

cargo build --release --locked -p sakshi --manifest-path "$root/Cargo.toml"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT INT TERM
project="$work/checkout-service"
cp -R "$here/project" "$project"
git -C "$project" init -q -b main
git -C "$project" add -A
git -C "$project" -c user.name=demo -c user.email=demo@example.com commit -qm "checkout service"

(
    cd "$project"
    PATH="$here:$PATH" python3 "$here/record.py" "$work/demo.cast" 110 30 "$here/schedule.json" -- \
        "$root/target/release/sakshi" --no-notify --no-log demo-agent
)

agg --font-size 15 --theme monokai --idle-time-limit 2 --last-frame-duration 4 "$work/demo.cast" "$out"
echo "wrote $out"
