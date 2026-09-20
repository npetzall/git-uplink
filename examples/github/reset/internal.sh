#!/usr/bin/env bash
# Reset the company product repo from seed / seed-state / seed-upstream.
# Run from GitHub Actions with GH_TOKEN. Force-push is expected.
# Keeps: main, seed, seed-state, seed-upstream, uplink/state, uplink/upstream,
# example-reset.

set -euo pipefail

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
export GH_TOKEN="${GH_TOKEN:-${GITHUB_TOKEN:?set GH_TOKEN or GITHUB_TOKEN}}"

comment="Resetting the git-uplink example to seed."

close_open_prs() {
  local numbers
  numbers=$(gh pr list --repo "$GITHUB_REPOSITORY" --state open --json number --jq '.[].number')
  if [[ -z "$numbers" ]]; then
    return
  fi
  while IFS= read -r n; do
    [[ -z "$n" ]] && continue
    echo "Closing PR #$n"
    gh pr close "$n" --repo "$GITHUB_REPOSITORY" --comment "$comment" || true
  done <<<"$numbers"
}

delete_extra_heads() {
  local refs
  refs=$(gh api "repos/${GITHUB_REPOSITORY}/git/matching-refs/heads" --jq '.[].ref' 2>/dev/null || true)
  while IFS= read -r ref; do
    [[ -z "$ref" ]] && continue
    local name=${ref#refs/heads/}
    case "$name" in
      main|seed|seed-state|seed-upstream|uplink/state|uplink/upstream|example-reset) continue ;;
      *)
        echo "Deleting ${name}"
        gh api --method DELETE "repos/${GITHUB_REPOSITORY}/git/${ref}" >/dev/null || true
        ;;
    esac
  done <<<"$refs"
}

echo "Reset $GITHUB_REPOSITORY from seed refs"
close_open_prs
delete_extra_heads

git fetch origin seed seed-state seed-upstream
git push --force origin "refs/remotes/origin/seed:refs/heads/main"
git push --force origin "refs/remotes/origin/seed-state:refs/heads/uplink/state"
git push --force origin "refs/remotes/origin/seed-upstream:refs/heads/uplink/upstream"
echo "main, uplink/state, and uplink/upstream match seed refs"
