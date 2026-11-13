#!/bin/sh
# Runs `git uplink init --upgrade` in the demo's company clone, creating the
# demo first when it is missing. It drops "Fix verify workflow by hand",
# keeps "Add notes", finishes without pausing, and asks whether to update the
# remote. Extra arguments go to the upgrade. Then, in the shell ./enter.sh opens:
#   git uplink init --upgrade              after No: offers a reset, then asks again
#   git uplink init --upgrade              after Yes: already up to date
#   ./run-demo.sh --allow-reset --push-if-unchanged   the same without questions
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=../lib.sh disable=SC1091
. "$HERE/../lib.sh"
demo_run init --upgrade "$@"
