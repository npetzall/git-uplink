#!/bin/sh
# Re-creates a scratch uplink world in ./scratch, next to this script, that is
# due an upgrade: company main carries an older forge pack than this binary's,
# and everything is published to a local origin, so nothing is pending.
#
# The older pack has a stale PR template line, an extra line in
# uplink-verify.yml, and a workflow the new pack no longer has. Queued:
#   - "Fix verify workflow by hand": what the new pack ships; dropped.
#   - "Add notes": product code; kept.
#   - "Company line in PR template" (conflict scenario only): changes the line
#     the new pack removes, so the upgrade pauses on it.
#
# Usage: ./make-demo.sh [conflict|clean]   (default: conflict)
set -e
SCENARIO=${1:-conflict}
case "$SCENARIO" in
  conflict | clean) ;;
  *)
    echo "usage: $0 [conflict|clean]" >&2
    exit 2
    ;;
esac
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=../lib.sh disable=SC1091
. "$HERE/../lib.sh"
S="$HERE/scratch"
demo_reset "$S" && cd "$S"

TEMPLATE=.github/pull_request_template.md
VERIFY=.github/workflows/uplink-verify.yml

# Commits what .uplink/ holds onto uplink/state, as git-uplink does.
commit_queue() {
  index="$(git rev-parse --git-dir)/uplink-demo-index"
  GIT_INDEX_FILE=$index git read-tree uplink/state
  GIT_INDEX_FILE=$index git add -f -- .uplink
  tree=$(GIT_INDEX_FILE=$index git write-tree)
  rm -f "$index"
  commit=$(git commit-tree "$tree" -p uplink/state -m "$1")
  git update-ref refs/heads/uplink/state "$commit"
}

# Records the commit on branch $1 as an internal-only patch with message $2,
# after landing it on main the way a merged PR does.
land_patch() {
  from=$(git rev-parse main)
  git checkout -q main
  git merge -q --ff-only "$1"
  git uplink add --internal-only --from "$from" --head "$1" --message "$2" >/dev/null
}

git init -q -b main upstream
(cd upstream && mkdir src && printf 'export const hash = "sha1";\n' > src/tokens.js && echo tokenkit > README.md && git add -A && git commit -qm "initial tokens")
git clone -q --bare upstream contrib.git
git init -q --bare -b main origin.git
git clone -q upstream company
cd company
git remote rename origin upstream
git remote add contrib ../contrib.git
git remote add origin ../origin.git
git uplink init --upstream ../upstream --contrib ../contrib.git --forge github \
  --preflight 'echo "all checks passed"' </dev/null >/dev/null 2>&1
git push -q origin main uplink/state uplink/upstream uplink/hooks

# Swap the tooling patch for an older pack: the new one, edited.
set -- .uplink/patches/*.patch
TOOLING=$1
git checkout -q --detach uplink/upstream
git am -q "$TOOLING"
cp "$VERIFY" ../verify.new.yml
printf 'old line\n' >> "$TEMPLATE"
printf '# old pack\n' >> "$VERIFY"
printf 'name: legacy\n' > .github/workflows/uplink-legacy.yml
git add -A && git commit -q --amend --no-edit
git format-patch --full-index -1 --stdout > "$TOOLING"
git checkout -q main
commit_queue "uplink: older tooling pack"
git uplink rebuild >/dev/null

git checkout -q -b fix/verify main
cp ../verify.new.yml "$VERIFY" && git add -A && git commit -qm "fix verify workflow"
land_patch fix/verify "Fix verify workflow by hand"
rm ../verify.new.yml

git checkout -q -b feat/notes main
echo "company notes" > NOTES.md && git add -A && git commit -qm "add notes"
land_patch feat/notes "Add notes"

if [ "$SCENARIO" = conflict ]; then
  git checkout -q -b feat/template main
  sed 's/^old line$/my line/' "$TEMPLATE" > ../template.tmp && mv ../template.tmp "$TEMPLATE"
  git add -A && git commit -qm "company line in PR template"
  land_patch feat/template "Company line in PR template"
fi

git checkout -q main
git push -q --force origin main uplink/state
demo_done "$S"
