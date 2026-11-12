#!/bin/sh
# Opens a shell in the demo's company clone with the demo's git-uplink on
# PATH, so `git uplink ...` works there; exit leaves it. Creates the demo
# first when it is missing.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=../lib.sh disable=SC1091
. "$HERE/../lib.sh"
demo_enter
