#!/bin/sh
# Runs `git uplink init --upgrade` in the demo's company clone, creating the
# demo first when it is missing. It drops "Fix verify workflow by hand",
# keeps "Add notes", and finishes without pausing. Extra arguments go to the
# upgrade. Then, in the shell ./enter.sh opens:
#   git uplink init --upgrade              again: refuses, nothing is pushed yet
#   git uplink push && git uplink rebuild --push
#   git uplink init --upgrade              then: already up to date
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=../lib.sh disable=SC1091
. "$HERE/../lib.sh"
demo_run init --upgrade "$@"
