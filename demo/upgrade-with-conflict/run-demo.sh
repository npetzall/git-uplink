#!/bin/sh
# Runs `git uplink init --upgrade` in the demo's company clone, creating the
# demo first when it is missing. The upgrade pauses on "Company line in PR
# template". Extra arguments go to the upgrade, so it is taken on with:
#   ./run-demo.sh --continue   (after resolving and `git add`)
#   ./run-demo.sh --drop
#   ./run-demo.sh --abort
# Once it is taken on, it asks whether to update the remote.
# Other things to try, in the shell ./enter.sh opens:
#   touch x && git uplink init --upgrade   offers a reset that deletes x
#   git uplink init --upgrade              after Yes: already up to date
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=../lib.sh disable=SC1091
. "$HERE/../lib.sh"
demo_run init --upgrade "$@"
