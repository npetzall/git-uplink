#!/usr/bin/env bash
# Initialize uplink-example-internal against already-bootstrapped upstream
# and contrib clones. Runs git uplink init --forge example-github (installs
# the internal-only workflows patch), pushes seed branches, and publishes
# orphan example-reset.
#
#   export UPSTREAM_DIR=$HOME/src/uplink-example-upstream
#   export CONTRIB_DIR=$HOME/src/uplink-example-upstream-contrib
#   export INTERNAL_DIR=$HOME/src/uplink-example-internal
#   ./examples/github/scripts/bootstrap_internal.sh
#
# See examples/github/SETUP.md.

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

need_cmd git
need_cmd git-uplink
require_clone UPSTREAM_DIR
require_clone CONTRIB_DIR
require_clone INTERNAL_DIR
ensure_contrib_owner_differs

echo "Upstream clone: $UPSTREAM_DIR  ($UPSTREAM)"
echo "Contrib clone:  $CONTRIB_DIR   ($CONTRIB)"
echo "Internal clone: $INTERNAL_DIR  ($INTERNAL)"
echo "Uplink source:  $UPLINK_SRC@$UPLINK_REV"

echo "Initializing $INTERNAL"
ensure_remote "$INTERNAL_DIR" origin "$INTERNAL_URL"
ensure_remote "$INTERNAL_DIR" upstream "$UPSTREAM_URL"
ensure_remote "$INTERNAL_DIR" contrib "$CONTRIB_URL"
git -C "$INTERNAL_DIR" fetch upstream
git -C "$INTERNAL_DIR" checkout -B main upstream/main
git -C "$INTERNAL_DIR" uplink init --upstream "$UPSTREAM_URL" --contrib "$CONTRIB_URL" --forge example-github
git -C "$INTERNAL_DIR" push -u origin main --force
git -C "$INTERNAL_DIR" push origin uplink/state --force
git -C "$INTERNAL_DIR" push origin uplink/upstream --force

echo "Seeding reset refs"
git -C "$INTERNAL_DIR" branch -f seed main
git -C "$INTERNAL_DIR" branch -f seed-state uplink/state
git -C "$INTERNAL_DIR" branch -f seed-upstream uplink/upstream
git -C "$INTERNAL_DIR" push origin seed seed-state seed-upstream --force
publish_example_reset "$INTERNAL_DIR" "$KIT_DIR/reset/internal.sh"
echo "Pushed $INTERNAL main, uplink/state, uplink/upstream, seed refs, and example-reset"

if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  echo "Setting labels and repository variables with gh"
  create_label() {
    local name=$1 color=$2 description=$3
    gh label create "$name" --repo "$INTERNAL" --color "$color" --description "$description" 2>/dev/null \
      || gh label edit "$name" --repo "$INTERNAL" --color "$color" --description "$description"
  }
  create_label "uplink:internal-only" "5319E7" "Never approve or submit this change upstream"
  create_label "uplink:conflict" "B60205" "Uplink sync conflict; resolve via the gated work PR"
  create_label "uplink:transfer-to-upstream" "1D76DB" "Uplink gated transfer to the upstream queue"
  create_label "uplink:transfer-to-internal" "1D76DB" "Uplink gated transfer to the internal queue"

  set_var() {
    gh variable set "$1" --repo "$INTERNAL" --body "$2"
  }
  set_var UPLINK_PREFLIGHT "npm test"
  set_var UPLINK_REDACT_KEYWORDS "companyTelemetry,AcmeCorp"
  set_var UPLINK_INTERNAL_DOMAINS "acme.example"
  set_var UPLINK_EXPORT_AUTHOR "Uplink Example <uplink@users.noreply.github.com>"
  set_var UPLINK_SRC "$UPLINK_SRC"
  set_var UPLINK_REV "$UPLINK_REV"
  set_var UPLINK_INTERNAL_AUTH "$UPLINK_INTERNAL_AUTH"
  set_var UPLINK_CONTRIB_AUTH "$UPLINK_CONTRIB_AUTH"
  set_var UPLINK_UPSTREAM_AUTH "$UPLINK_UPSTREAM_AUTH"

  gh api --method PUT "repos/${INTERNAL}/actions/permissions" \
    -F enabled=true -f allowed_actions=all >/dev/null || true
  gh api --method PUT "repos/${INTERNAL}/actions/permissions/workflow" \
    -f default_workflow_permissions=write -F can_approve_pull_request_reviews=false >/dev/null || true
  gh api --method PUT "repos/${INTERNAL}/environments/to-upstream" \
    --input - >/dev/null <<'EOF'
{"wait_timer":0,"prevent_self_review":false}
EOF
  gh api --method PUT "repos/${INTERNAL}/environments/from-upstream" \
    --input - >/dev/null <<'EOF'
{"wait_timer":0,"prevent_self_review":false}
EOF
  gh api --method PUT "repos/${INTERNAL}/environments/abandon-contrib" \
    --input - >/dev/null <<'EOF'
{"wait_timer":0,"prevent_self_review":false}
EOF
else
  echo
  echo "gh is not available; set labels, variables, Actions write permission, and Environments to-upstream, from-upstream, and abandon-contrib in the GitHub UI (SETUP.md)."
fi

echo
echo "Bootstrap complete."
echo "  $UPSTREAM  main + seed + example-reset"
echo "  $CONTRIB   main (upstream fork) + example-reset"
echo "  $INTERNAL  main + uplink/state + uplink/upstream + seed refs + example-reset"
echo
echo "Finish SETUP.md (to-upstream, from-upstream, and abandon-contrib reviewers; UPLINK_INTERNAL_TOKEN, UPLINK_UPSTREAM_TOKEN, UPLINK_CONTRIB_TOKEN on to-upstream and abandon-contrib), then walk examples/github/stories/."
