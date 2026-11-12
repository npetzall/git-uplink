#!/bin/sh
# Re-creates a scratch uplink world in ./scratch, next to this script, that is
# due an upgrade (see ../upgrade-lib.sh). Nothing in the queue conflicts with
# the new pack, so the upgrade runs through.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=../lib.sh disable=SC1091
. "$HERE/../lib.sh"
# shellcheck source=../upgrade-lib.sh disable=SC1091
. "$HERE/../upgrade-lib.sh"
upgrade_world clean
demo_done "$S"
