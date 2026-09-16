#!/usr/bin/env bash
# Bootstrap already-created clones of the three example GitHub repositories.
# The walker creates and clones the repos first; this script applies seed
# files, runs git uplink init, imports the internal-only workflows patch,
# and pushes seed branches.
#
#   export UPSTREAM_DIR=$HOME/src/uplink-example-upstream
#   export CONTRIB_DIR=$HOME/src/uplink-example-upstream-contrib
#   export INTERNAL_DIR=$HOME/src/uplink-example-internal
#   ./examples/github/scripts/bootstrap.sh
#
# See examples/github/SETUP.md.

set -euo pipefail

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=env.sh
source "$SCRIPT_DIR/env.sh"

need_cmd git
need_cmd git-uplink

git_bot() {
  git -c user.name="Uplink Example" \
    -c user.email="uplink-example@users.noreply.github.com" \
    -c commit.gpgsign=false "$@"
}

copy_overlay() {
  local src=$1 dest=$2
  if command -v rsync >/dev/null 2>&1; then
    rsync -a "$src/" "$dest/"
  else
    cp -R "$src"/. "$dest/"
  fi
}

replace_tree() {
  local src=$1 dest=$2
  if command -v rsync >/dev/null 2>&1; then
    rsync -a --delete --exclude .git "$src/" "$dest/"
  else
    find "$dest" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
    cp -R "$src"/. "$dest/"
  fi
}

ensure_remote() {
  local dir=$1 name=$2 url=$3
  if git -C "$dir" remote get-url "$name" >/dev/null 2>&1; then
    git -C "$dir" remote set-url "$name" "$url"
  else
    git -C "$dir" remote add "$name" "$url"
  fi
}

echo "Upstream clone: $UPSTREAM_DIR  ($UPSTREAM)"
echo "Contrib clone:  $CONTRIB_DIR   ($CONTRIB)"
echo "Internal clone: $INTERNAL_DIR  ($INTERNAL)"
echo "Uplink source:  $UPLINK_SRC@$UPLINK_REV"

if [[ "$UPSTREAM" == "$CONTRIB" ]]; then
  echo "Contrib must be a fork under a different owner than upstream." >&2
  exit 1
fi

echo "Seeding $UPSTREAM"
replace_tree "$KIT_DIR/upstream" "$UPSTREAM_DIR"
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
echo "Pushed $UPSTREAM main and seed"

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
echo "Pushed $CONTRIB main (fork default; not synced from upstream)"

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
  --from main \
  --push
git -C "$INTERNAL_DIR" checkout --quiet main
git -C "$INTERNAL_DIR" branch -f seed main
git -C "$INTERNAL_DIR" branch -f seed-state uplink/state
git -C "$INTERNAL_DIR" branch -f seed-upstream uplink/upstream
git -C "$INTERNAL_DIR" push origin seed seed-state seed-upstream --force
echo "Pushed $INTERNAL main, uplink/state, uplink/upstream, and seed refs"

if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  echo "Setting labels and repository variables with gh"
  create_label() {
    local name=$1 color=$2 description=$3
    gh label create "$name" --repo "$INTERNAL" --color "$color" --description "$description" 2>/dev/null \
      || gh label edit "$name" --repo "$INTERNAL" --color "$color" --description "$description"
  }
  create_label "uplink:import" "0E8A16" "Product gate: import this PR as a queued patch"
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
  set_var UPLINK_SUBMIT_AUTH "$UPLINK_SUBMIT_AUTH"
  set_var UPLINK_UPSTREAM "$UPSTREAM"
  set_var UPLINK_CONTRIB "$CONTRIB"

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
echo "  $UPSTREAM  main + seed"
echo "  $CONTRIB   main (Reset example workflow only)"
echo "  $INTERNAL  main + uplink/state + uplink/upstream + seed refs"
echo
echo "Finish SETUP.md (oss reviewer and UPLINK_GITHUB_TOKEN), then walk examples/github/stories/."
