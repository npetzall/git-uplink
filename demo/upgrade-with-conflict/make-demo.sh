#!/bin/sh
# Re-creates a scratch uplink world in ./scratch, next to this script, that is
# due an upgrade (see ../upgrade-lib.sh). Besides the queue of the upgrade
# demo, "Company line in PR template" changes the line the new pack removes,
# so the upgrade pauses on it.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=../lib.sh disable=SC1091
. "$HERE/../lib.sh"
# shellcheck source=../upgrade-lib.sh disable=SC1091
. "$HERE/../upgrade-lib.sh"
upgrade_world conflict
demo_done "$S"
