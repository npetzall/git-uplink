#!/bin/sh
# Opens the adopt UI on the demo's main-fork branch, creating the demo first
# when it is missing. Extra arguments go to `git uplink adopt`, e.g.
# ./run-demo.sh --no-preflight-deps
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
if [ ! -d "$HERE/scratch/company" ] || [ ! -e "$HERE/scratch/bin/git-uplink" ]; then
  "$HERE/make-demo.sh"
fi
cd "$HERE/scratch/company"
exec "$HERE/scratch/bin/git-uplink" adopt --branch main-fork "$@"
