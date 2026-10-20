#!/bin/sh
# Opens the adopt UI on the demo's main-fork branch, creating the demo first
# when it is missing. Extra arguments go to `git uplink adopt`, e.g.
# ./run-demo.sh --no-preflight-deps
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
if [ ! -d "$HERE/demo/company" ]; then
  "$HERE/make-demo.sh"
fi
cd "$HERE/demo/company"
exec "$ROOT/target/debug/git-uplink" adopt --branch main-fork "$@"
