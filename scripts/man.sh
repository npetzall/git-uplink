#!/bin/sh
# Show a man page built from the working tree: scripts/man.sh [command]
set -eu
cd "$(dirname "$0")/.."
sfw cargo run --locked --quiet -- man target/man >/dev/null
man "target/man/git-uplink${1:+-$1}.1"
