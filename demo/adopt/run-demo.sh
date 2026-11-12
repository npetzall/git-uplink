#!/bin/sh
# Opens the adopt UI on the demo's main-fork branch, creating the demo first
# when it is missing. Extra arguments go to `git uplink adopt`, e.g.
# ./run-demo.sh --no-preflight-deps
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=../lib.sh disable=SC1091
. "$HERE/../lib.sh"
demo_run adopt --branch main-fork "$@"
