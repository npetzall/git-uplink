#!/usr/bin/env bash
# Wipe leftover heads on the contrib fork. Do not move main, do not
# close PRs (contribution PRs live on upstream).
# Keeps: main, seed, example-reset.

set -euo pipefail

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
export GH_TOKEN="${GH_TOKEN:-${GITHUB_TOKEN:?set GH_TOKEN or GITHUB_TOKEN}}"

refs=$(gh api "repos/${GITHUB_REPOSITORY}/git/matching-refs/heads" --jq '.[].ref' 2>/dev/null || true)
while IFS= read -r ref; do
  [[ -z "$ref" ]] && continue
  name=${ref#refs/heads/}
  case "$name" in
    main|seed|example-reset) continue ;;
    *)
      echo "Deleting ${name}"
      gh api --method DELETE "repos/${GITHUB_REPOSITORY}/git/${ref}" >/dev/null || true
      ;;
  esac
done <<<"$refs"

echo "Contrib reset: extra branches removed; main unchanged"
