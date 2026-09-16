#!/usr/bin/env bash
# Shared paths and GitHub remotes for the example bootstrap script.
# Required: UPSTREAM_DIR, CONTRIB_DIR, INTERNAL_DIR — local clones of the
# three already-created GitHub repositories (origin must be set).

set -euo pipefail

if [[ -z "${UPSTREAM_DIR:-}" || -z "${CONTRIB_DIR:-}" || -z "${INTERNAL_DIR:-}" ]]; then
  echo "Set UPSTREAM_DIR, CONTRIB_DIR, and INTERNAL_DIR to clones of the three GitHub repos." >&2
  exit 1
fi

for d in "$UPSTREAM_DIR" "$CONTRIB_DIR" "$INTERNAL_DIR"; do
  if [[ ! -d "$d/.git" ]]; then
    echo "Not a git clone: $d" >&2
    exit 1
  fi
done

origin_url() {
  git -C "$1" remote get-url origin
}

UPSTREAM_URL=$(origin_url "$UPSTREAM_DIR")
CONTRIB_URL=$(origin_url "$CONTRIB_DIR")
INTERNAL_URL=$(origin_url "$INTERNAL_DIR")

github_owner_repo() {
  local url=$1
  if [[ "$url" =~ github.com[:/]([^/]+)/([^/.]+)(\.git)?$ ]]; then
    printf '%s/%s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}"
    return 0
  fi
  echo "Could not parse GitHub owner/repo from $url" >&2
  return 1
}

UPSTREAM=$(github_owner_repo "$UPSTREAM_URL")
CONTRIB=$(github_owner_repo "$CONTRIB_URL")
INTERNAL=$(github_owner_repo "$INTERNAL_URL")

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
KIT_DIR=$(cd "$SCRIPT_DIR/.." && pwd)
REPO_ROOT=$(cd "$KIT_DIR/../.." && pwd)

UPLINK_SRC="${UPLINK_SRC:-}"
if [[ -z "$UPLINK_SRC" ]]; then
  origin=$(git -C "$REPO_ROOT" remote get-url origin 2>/dev/null || true)
  if [[ "$origin" =~ github.com[:/]([^/]+)/([^/.]+)(\.git)?$ ]]; then
    UPLINK_SRC="${BASH_REMATCH[1]}/${BASH_REMATCH[2]}"
  else
    UPLINK_SRC="npetzall/git-uplink"
  fi
fi
UPLINK_REV="${UPLINK_REV:-main}"
UPLINK_SUBMIT_AUTH="${UPLINK_SUBMIT_AUTH:-pat}"

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}
