#!/usr/bin/env bash
# Seed and push uplink-example-upstream. Run this before forking contrib.
#
#   export UPSTREAM_DIR=$HOME/src/uplink-example-upstream
#   ./examples/github/scripts/bootstrap_upstream.sh
#
# See examples/github/SETUP.md.

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

need_cmd git
require_clone UPSTREAM_DIR

echo "Upstream clone: $UPSTREAM_DIR  ($UPSTREAM)"

echo "Seeding $UPSTREAM"
replace_tree "$KIT_DIR/upstream" "$UPSTREAM_DIR"
install_example_reset_stub "$UPSTREAM_DIR"
git -C "$UPSTREAM_DIR" add -A
if git -C "$UPSTREAM_DIR" diff --cached --quiet && git -C "$UPSTREAM_DIR" rev-parse --verify HEAD >/dev/null 2>&1; then
  echo "Upstream seed already committed"
else
  git_bot -C "$UPSTREAM_DIR" commit -m "initial tokens"
fi
git -C "$UPSTREAM_DIR" branch -M main
git -C "$UPSTREAM_DIR" push -u origin main --force
git -C "$UPSTREAM_DIR" branch -f seed main
git -C "$UPSTREAM_DIR" push origin seed --force
publish_example_reset "$UPSTREAM_DIR" "$KIT_DIR/reset/upstream.sh"
echo "Pushed $UPSTREAM main, seed, and example-reset"
echo
echo "Fork $UPSTREAM to your user as uplink-example-upstream-contrib, then run bootstrap_upstream-contrib.sh."
