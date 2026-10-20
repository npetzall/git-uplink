#!/bin/sh
# Re-creates a scratch uplink world in ./demo, next to this script: a public
# upstream, a contrib bare repo, and a company clone with uplink initialized
# and a main-fork branch to adopt from. One fork commit adds a TODO, which
# the demo preflight.sh rejects.
set -e
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
BIN="$ROOT/target/debug"
if [ ! -x "$BIN/git-uplink" ]; then
  echo "no $BIN/git-uplink; build it first: sfw cargo build" >&2
  exit 1
fi
PATH="$BIN:$PATH"
S="$HERE/demo"
rm -rf "$S" && mkdir -p "$S" && cd "$S"

git init -q -b main upstream
(cd upstream && mkdir src && printf 'export const hash = "sha1";\n' > src/tokens.js && echo tokenkit > README.md && git add -A && git commit -qm "initial tokens")
git clone -q --bare upstream contrib.git
git clone -q upstream company
cd company
git remote rename origin upstream
git remote add contrib ../contrib.git
git uplink init --upstream ../upstream --contrib ../contrib.git --forge github >/dev/null 2>&1

git worktree add -q ../hooks uplink/hooks
cat > ../hooks/preflight.sh <<'EOF'
echo "running checks..."
sleep 1
if grep -rn TODO src; then
  echo "error: TODO markers are not allowed upstream"
  exit 1
fi
echo "all checks passed"
EOF
(cd ../hooks && git add preflight.sh && git commit -qm "preflight: reject TODOs")
git worktree remove ../hooks

git checkout -q -b main-fork
printf 'export const max = 10;\n' > src/limits.js && git add -A && git commit -qm "Add limits" -m "Callers need an upper bound."
printf 'export const max = 10;\nexport const min = 1;\n' > src/limits.js && git add -A && git commit -qm "Add min limit"
printf 'export function helper() {\n  // TODO: handle errors\n}\n' > src/helper.js && git add -A && git commit -qm "Add helper"
echo "fork notes" > NOTES.md && git add -A && git commit -qm "Fork notes"
git checkout -q main
echo "demo ready: $S/company"
