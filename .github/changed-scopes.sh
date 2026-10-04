#!/usr/bin/env bash
# Which CI scopes a pull request touches. Reads changed paths on stdin, one per
# line, and prints product=true|false and site=true|false. pr.yml uses the
# result to decide whether to call product.yml and site.yml.
set -euo pipefail

product_patterns=(
  'src/'
  'tests/'
  'web/'
  'templates/'
  'docs/cli\.md$'
  'Cargo\.(toml|lock)$'
  'build\.rs$'
  'Cross\.toml$'
  'rust-toolchain\.toml$'
  'ruff\.toml$'
  '\.pre-commit-config\.yaml$'
  '\.github/calculate-version\.sh$'
  '\.github/changed-scopes\.sh$'
  '\.github/[^/]+\.py$'
  '\.github/workflows/(pr|product|version|codeql|zizmor|socket)\.yml$'
  '\.github/codeql/(web|templates|github-scripts)\.yml$'
)

site_patterns=(
  'site/'
  'docs/'
  'examples/github/stories/'
  'templates/[^/]+/README\.md$'
  '\.github/changed-scopes\.sh$'
  '\.github/workflows/(pr|site|publish-site|codeql|zizmor|socket)\.yml$'
  '\.github/codeql/site\.yml$'
)

# Join patterns into one regex anchored at the start of the path.
anchored() {
  local IFS='|'
  printf '^(%s)' "$*"
}

scopes() {
  local product_re site_re path
  local product=false site=false
  product_re="$(anchored "${product_patterns[@]}")"
  site_re="$(anchored "${site_patterns[@]}")"
  while IFS= read -r path; do
    if [[ "$path" =~ $product_re ]]; then
      product=true
    fi
    if [[ "$path" =~ $site_re ]]; then
      site=true
    fi
  done
  printf 'product=%s\nsite=%s\n' "$product" "$site"
}

expect_scopes() {
  local want_product="$1"
  local want_site="$2"
  shift 2
  local got want
  got="$(printf '%s\n' "$@" | scopes)"
  want="$(printf 'product=%s\nsite=%s' "$want_product" "$want_site")"
  if [[ "$got" != "$want" ]]; then
    printf 'self-check failed: %s: got %s want %s\n' "$*" "${got//$'\n'/ }" "${want//$'\n'/ }" >&2
    exit 1
  fi
}

self_check() {
  expect_scopes false false "README.md"
  expect_scopes false false "srcs/main.rs"
  expect_scopes true false "src/cli.rs"
  expect_scopes true false "Cargo.lock"
  expect_scopes true false ".github/update_version_in_cargo.py"
  expect_scopes true false "templates/github/.github/workflows/uplink-pr.yml"
  expect_scopes false true "site/src/main.tsx"
  expect_scopes false true "docs/design.md"
  expect_scopes false true ".github/workflows/publish-site.yml"
  expect_scopes true true "docs/cli.md"
  expect_scopes true true "templates/github/README.md"
  expect_scopes true true ".github/workflows/pr.yml"
  expect_scopes true true "README.md" "src/cli.rs" "site/package.json"
  printf 'self-check ok\n'
}

case "${1:-}" in
  --self-check)
    self_check
    ;;
  "")
    scopes
    ;;
  *)
    printf 'unknown argument: %s\n' "$1" >&2
    exit 1
    ;;
esac
