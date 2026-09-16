#!/usr/bin/env bash
# Publish the contrib Reset example script on orphan example-reset.
# Do not change contrib main (it must stay a fork of upstream).
# Upstream must already be bootstrapped and forked.
#
#   export CONTRIB_DIR=$HOME/src/uplink-example-upstream-contrib
#   # optional, to check the fork owner differs from upstream:
#   export UPSTREAM_DIR=$HOME/src/uplink-example-upstream
#   ./examples/github/scripts/bootstrap_upstream-contrib.sh
#
# See examples/github/SETUP.md.

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

need_cmd git
require_clone CONTRIB_DIR

if [[ -n "${UPSTREAM_DIR:-}" ]]; then
  require_clone UPSTREAM_DIR
  ensure_contrib_owner_differs
fi

echo "Contrib clone:  $CONTRIB_DIR   ($CONTRIB)"

echo "Publishing contrib Reset example on orphan example-reset"
git -C "$CONTRIB_DIR" fetch origin
git -C "$CONTRIB_DIR" checkout -B main origin/main 2>/dev/null || git -C "$CONTRIB_DIR" checkout main
publish_example_reset "$CONTRIB_DIR" "$KIT_DIR/reset/contrib.sh"
echo "Pushed $CONTRIB example-reset (main unchanged, still the upstream fork)"
echo
echo "Create uplink-example-internal, then run bootstrap_internal.sh."
