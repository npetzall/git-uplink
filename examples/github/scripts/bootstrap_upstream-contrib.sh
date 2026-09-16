#!/usr/bin/env bash
# Add the Reset example workflow to the contrib fork of upstream.
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

echo "Installing contrib Reset example workflow on $CONTRIB"
copy_overlay "$KIT_DIR/upstream-contrib" "$CONTRIB_DIR"
git -C "$CONTRIB_DIR" add -A
if git -C "$CONTRIB_DIR" diff --cached --quiet && git -C "$CONTRIB_DIR" rev-parse --verify HEAD >/dev/null 2>&1; then
  echo "Contrib reset workflow already committed"
else
  git_bot -C "$CONTRIB_DIR" commit -m "Reset example workflow"
fi
git -C "$CONTRIB_DIR" branch -M main
git -C "$CONTRIB_DIR" push -u origin main
echo "Pushed $CONTRIB main (Reset example workflow on the fork)"
echo
echo "Create uplink-example-internal, then run bootstrap_internal.sh."
