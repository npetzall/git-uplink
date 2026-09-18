#!/usr/bin/env bash
# Initialize uplink-example-internal against already-bootstrapped upstream
# and contrib clones. Runs git uplink init, imports the internal-only
# workflows patch, pushes seed branches, and publishes orphan example-reset.
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
git -C "$INTERNAL_DIR" uplink init --upstream "$UPSTREAM_URL" --contrib "$CONTRIB_URL"
git -C "$INTERNAL_DIR" push -u origin main --force
git -C "$INTERNAL_DIR" push origin uplink/state --force
git -C "$INTERNAL_DIR" push origin uplink/upstream --force

echo "Importing internal-only GitHub Actions patch"
git -C "$INTERNAL_DIR" checkout -B feat/internal-github
copy_overlay "$KIT_DIR/internal" "$INTERNAL_DIR"
git -C "$INTERNAL_DIR" add -A
git_bot -C "$INTERNAL_DIR" commit -m "Example GitHub workflows"
FROM_SHA=$(git -C "$INTERNAL_DIR" rev-parse main)
HEAD_SHA=$(git -C "$INTERNAL_DIR" rev-parse HEAD)
git -C "$INTERNAL_DIR" checkout --quiet main
git -C "$INTERNAL_DIR" merge --ff-only --quiet feat/internal-github
git -C "$INTERNAL_DIR" push origin main --force
WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT
{
  printf '%s\n\n' "Example GitHub workflows"
  printf '%s\n' "Install Uplink Actions on company main. Not for upstream."
  printf '%s\n' "----- Uplink: internal below this line -----"
  printf '%s\n' "Ticket: PROJ-0000"
} > "$WORKDIR/workflows.msg"
git -C "$INTERNAL_DIR" uplink add \
  --title "Example GitHub workflows" \
  --message-file "$WORKDIR/workflows.msg" \
  --internal-only \
  --from "$FROM_SHA" \
  --head "$HEAD_SHA" \
  --push
git -C "$INTERNAL_DIR" checkout --quiet main
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
  create_label "uplink:conflict" "B60205" "Uplink sync conflict; checkout the conflict branch, do not open a PR"

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
  gh api --method PUT "repos/${INTERNAL}/environments/oss" \
    --input - >/dev/null <<'EOF'
{"wait_timer":0,"prevent_self_review":false}
EOF
else
  echo
  echo "gh is not available; set labels, variables, Actions write permission, and Environment oss in the GitHub UI (SETUP.md)."
fi

echo
echo "Bootstrap complete."
echo "  $UPSTREAM  main + seed + example-reset"
echo "  $CONTRIB   main (upstream fork) + example-reset"
echo "  $INTERNAL  main + uplink/state + uplink/upstream + seed refs + example-reset"
echo
echo "Finish SETUP.md (oss reviewer, UPLINK_INTERNAL_TOKEN, UPLINK_UPSTREAM_TOKEN, UPLINK_CONTRIB_TOKEN), then walk examples/github/stories/."
