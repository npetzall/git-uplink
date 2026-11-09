#!/bin/sh
# Runs `git uplink init --upgrade` in the demo's company clone, creating the
# demo (conflict scenario) first when it is missing. Extra arguments go to
# the upgrade, so a paused upgrade is taken on with:
#   ./run-demo.sh --continue   (after resolving and `git add`)
#   ./run-demo.sh --drop
#   ./run-demo.sh --abort
# Other things to try, in the shell ./enter.sh opens:
#   touch x && git uplink init --upgrade   refuses: untracked file
#   git uplink init --upgrade              again: refuses, nothing is pushed yet
#   git uplink push && git uplink rebuild --push
#   git uplink init --upgrade              then: already up to date
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
if [ ! -d "$HERE/scratch/company" ]; then
  "$HERE/make-demo.sh"
fi
cd "$HERE/scratch/company"
exec "$HERE/scratch/bin/git-uplink" init --upgrade "$@"
