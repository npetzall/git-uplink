#!/usr/bin/env bash
# Reset this repository's main from the seed branch.
# Run from GitHub Actions (workflow_dispatch) with GH_TOKEN.
# Keeps: main, seed, example-reset. Closes open PRs. Force-push is expected.

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
      main|seed|example-reset) continue ;;
      *)
        echo "Deleting ${name}"
        gh api --method DELETE "repos/${GITHUB_REPOSITORY}/git/${ref}" >/dev/null || true
        ;;
    esac
  done <<<"$refs"
}

echo "Reset $GITHUB_REPOSITORY: seed -> main"
close_open_prs
delete_extra_heads

git fetch origin seed
git push --force origin "refs/remotes/origin/seed:refs/heads/main"
echo "main now matches seed"
