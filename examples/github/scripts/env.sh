#!/usr/bin/env bash
# Shared paths, GitHub remotes, and helpers for the example bootstrap scripts.
# Each script calls require_clone for the dirs it needs (UPSTREAM_DIR,
# CONTRIB_DIR, INTERNAL_DIR). origin on each clone must be set.

set -euo pipefail

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

origin_url() {
  git -C "$1" remote get-url origin
}

github_owner_repo() {
  local url=$1
  if [[ "$url" =~ github.com[:/]([^/]+)/([^/.]+)(\.git)?$ ]]; then
    printf '%s/%s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}"
    return 0
  fi
  echo "Could not parse GitHub owner/repo from $url" >&2
  return 1
}

# require_clone UPSTREAM_DIR | CONTRIB_DIR | INTERNAL_DIR
# Sets the matching *_URL and owner/repo (UPSTREAM, CONTRIB, INTERNAL).
require_clone() {
  local var=$1
  local prefix dir url owner_repo
  case "$var" in
    UPSTREAM_DIR) prefix=UPSTREAM ;;
    CONTRIB_DIR) prefix=CONTRIB ;;
    INTERNAL_DIR) prefix=INTERNAL ;;
    *)
      echo "Unknown clone var: $var" >&2
      exit 1
      ;;
  esac
  dir="${!var:-}"
  if [[ -z "$dir" ]]; then
    echo "Set $var to a clone of the GitHub repo." >&2
    exit 1
  fi
  if [[ ! -d "$dir/.git" ]]; then
    echo "Not a git clone: $dir" >&2
    exit 1
  fi
  url=$(origin_url "$dir")
  owner_repo=$(github_owner_repo "$url")
  printf -v "${prefix}_URL" '%s' "$url"
  printf -v "${prefix}" '%s' "$owner_repo"
}

ensure_contrib_owner_differs() {
  if [[ "$UPSTREAM" == "$CONTRIB" ]]; then
    echo "Contrib must be a fork under a different owner than upstream." >&2
    exit 1
  fi
}

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

install_example_reset_stub() {
  local dest=$1
  mkdir -p "$dest/.github/workflows"
  cp "$KIT_DIR/example-reset.yml" "$dest/.github/workflows/example-reset.yml"
}

# publish_example_reset CLONE_DIR SCRIPT_SRC
# Orphan branch example-reset: stub workflow + that repo's reset script.
publish_example_reset() {
  local dir=$1
  local script_src=$2
  local return_branch
  return_branch=$(git -C "$dir" branch --show-current 2>/dev/null || true)
  if [[ -z "$return_branch" ]]; then
    return_branch=main
  fi

  git -C "$dir" switch --orphan example-reset_tmp
  git -C "$dir" branch -D example-reset 2>/dev/null || true
  git -C "$dir" switch --orphan example-reset
  install_example_reset_stub "$dir"
  mkdir -p "$dir/scripts"
  cp "$script_src" "$dir/scripts/reset-example.sh"
  git -C "$dir" add -- \
    .github/workflows/example-reset.yml \
    scripts/reset-example.sh
  git_bot -C "$dir" commit -m "Reset example"
  git -C "$dir" push origin example-reset --force
  git -C "$dir" switch -f "$return_branch"
}
