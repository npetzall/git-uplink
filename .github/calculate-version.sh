#!/usr/bin/env bash
# Release version from the latest vX.Y.Z tag and conventional commits since that tag.
# No tag starts at 0.0.0. Merges are excluded. Each run bumps at most once,
# by the highest-ranking commit: any breaking commit increments major and resets
# minor and patch, else any feat increments minor and resets patch, else any
# other commit increments patch. No commits leaves the version unchanged.
# A normal run prints current_tag, current_version, next_tag, and next_version.
set -euo pipefail

# Drop a leading conventional-commit type, then an optional (scope).
# Prints the remainder of the subject, or the original subject if there is no type.
commit_rest() {
  local rest="$1"
  if [[ "$rest" != [A-Za-z]* ]]; then
    printf '%s\n' "$rest"
    return 0
  fi
  while [[ "$rest" == [A-Za-z]* ]]; do
    rest="${rest#?}"
  done
  if [[ "$rest" == \(*\)* ]]; then
    rest="${rest#*)}"
  fi
  printf '%s\n' "$rest"
}

is_breaking_message() {
  local message="$1"
  local subject rest
  subject="${message%%$'\n'*}"
  subject="${subject%$'\r'}"
  rest="$(commit_rest "$subject")"
  if [[ "$rest" == !:* ]]; then
    return 0
  fi
  [[ "$message" == *"BREAKING CHANGE:"* || "$message" == *"BREAKING-CHANGE:"* ]]
}

is_feat_subject() {
  local subject="$1"
  local rest
  subject="${subject%$'\r'}"
  if [[ "$subject" != feat* ]]; then
    return 1
  fi
  rest="${subject#feat}"
  if [[ "$rest" == :* ]]; then
    return 0
  fi
  if [[ "$rest" == \(*\)* ]]; then
    [[ "${rest#*)}" == :* ]]
    return
  fi
  return 1
}

# Prints major, minor, or patch for one commit message.
bump_kind() {
  local message="$1"
  if is_breaking_message "$message"; then
    printf 'major\n'
  elif is_feat_subject "${message%%$'\n'*}"; then
    printf 'minor\n'
  else
    printf 'patch\n'
  fi
}

# Prints the higher of two kinds (none < patch < minor < major).
max_kind() {
  local a="$1"
  local b="$2"
  local kind
  for kind in major minor patch; do
    if [[ "$a" == "$kind" || "$b" == "$kind" ]]; then
      printf '%s\n' "$kind"
      return 0
    fi
  done
  printf 'none\n'
}

bump_version() {
  local version="$1"
  local kind="$2"
  local major minor patch
  IFS=. read -r major minor patch <<<"$version"
  case "$kind" in
    major)
      major=$((major + 1))
      minor=0
      patch=0
      ;;
    minor)
      minor=$((minor + 1))
      patch=0
      ;;
    patch)
      patch=$((patch + 1))
      ;;
  esac
  printf '%d.%d.%d\n' "$major" "$minor" "$patch"
}

apply_messages() {
  local version="$1"
  shift
  local kind="none"
  local message
  for message in "$@"; do
    kind="$(max_kind "$kind" "$(bump_kind "$message")")"
    if [[ "$kind" == "major" ]]; then
      break
    fi
  done
  bump_version "$version" "$kind"
}

expect_version() {
  local got="$1"
  local want="$2"
  local label="$3"
  if [[ "$got" != "$want" ]]; then
    printf 'self-check failed: %s: got %s want %s\n' "$label" "$got" "$want" >&2
    exit 1
  fi
}

self_check() {
  expect_version "$(apply_messages 1.2.3)" "1.2.3" "no commits"
  expect_version "$(apply_messages 0.0.0 "docs: readme" "fix: bug")" "0.0.1" "no tag, docs then fix"
  expect_version "$(apply_messages 0.0.0 "fix: a" "fix: b")" "0.0.1" "patch bumps once"
  expect_version "$(apply_messages 0.1.3 "feat: add")" "0.2.0" "v0.1.3 plus feat"
  expect_version "$(apply_messages 1.2.3 "feat!: break api")" "2.0.0" "v1.2.3 plus breaking"
  expect_version "$(apply_messages 0.1.3 "feat: add" "fix: bug")" "0.2.0" "feat wins over later fix"
  expect_version "$(apply_messages 0.1.3 "fix: bug" "feat: add")" "0.2.0" "feat wins over earlier fix"
  expect_version "$(apply_messages 0.1.3 "feat: a" "feat: b")" "0.2.0" "minor bumps once"
  expect_version "$(apply_messages 1.2.3 "feat!: a" "feat: b" "fix: c")" "2.0.0" "breaking wins"
  expect_version "$(apply_messages 1.2.3 "feat: add" "$(printf 'fix: x\n\nBREAKING CHANGE: drop api')")" "2.0.0" "breaking footer wins"
  printf 'self-check ok\n'
}

semver_tag() {
  git tag --sort=-v:refname "$@" | awk '/^v[0-9]+\.[0-9]+\.[0-9]+$/ { print; exit }'
}

apply_log() {
  local version="$1"
  local remaining="$2"
  local kind="none"
  local message
  while [[ -n "$remaining" && "$kind" != "major" ]]; do
    message="${remaining%%$'\x1e'*}"
    if [[ "$remaining" == *$'\x1e'* ]]; then
      remaining="${remaining#*$'\x1e'}"
    else
      remaining=""
    fi
    while [[ "$message" == $'\n'* || "$message" == $'\r'* ]]; do
      message="${message#?}"
    done
    if [[ -z "${message//[[:space:]]/}" ]]; then
      continue
    fi
    kind="$(max_kind "$kind" "$(bump_kind "$message")")"
  done
  bump_version "$version" "$kind"
}

is_semver() {
  printf '%s\n' "$1" | awk '/^[0-9]+\.[0-9]+\.[0-9]+$/ { ok = 1 } END { exit !ok }'
}

compute_from_git() {
  local base_tag current_tag current_version next_version log
  base_tag="$(semver_tag || true)"
  if [[ -n "$base_tag" ]]; then
    current_tag="$base_tag"
    current_version="${base_tag#v}"
    log="$(git log --no-merges --pretty=format:%B%x1e "${base_tag}..HEAD")"
  else
    current_tag=""
    current_version="0.0.0"
    log="$(git log --no-merges --pretty=format:%B%x1e)"
  fi
  next_version="$(apply_log "$current_version" "$log")"
  if ! is_semver "$current_version" || ! is_semver "$next_version"; then
    printf 'refusing version: current=%s next=%s\n' "$current_version" "$next_version" >&2
    exit 1
  fi
  printf 'current_tag=%s\n' "$current_tag"
  printf 'current_version=%s\n' "$current_version"
  printf 'next_tag=v%s\n' "$next_version"
  printf 'next_version=%s\n' "$next_version"
}

run_self_check=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --self-check)
      run_self_check=true
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      exit 1
      ;;
  esac
  shift
done

if [[ "$run_self_check" == "true" ]]; then
  self_check
  exit 0
fi

compute_from_git
