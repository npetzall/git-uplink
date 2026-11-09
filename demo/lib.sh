# shellcheck shell=sh
# Shared by the demo scripts; source it, do not run it.
# Needs HERE (the demo's directory) set before it is sourced.
ROOT=$(cd "$HERE/../.." && pwd)
BIN="$ROOT/target/debug"
if [ ! -x "$BIN/git-uplink" ]; then
  echo "no $BIN/git-uplink; build it first: sfw cargo build" >&2
  exit 1
fi

# Re-creates the demo directory $1 with bin/git-uplink, a symlink to the
# debug build, so a rebuild updates the demo, and puts it on PATH.
demo_reset() {
  rm -rf "$1" && mkdir -p "$1/bin"
  ln -s "$BIN/git-uplink" "$1/bin/git-uplink"
  PATH="$1/bin:$PATH"
  export PATH
}

# Prints where the demo is and how to explore it.
demo_done() {
  echo "demo ready: $1/company"
  echo "explore:    $HERE/enter.sh"
}

# Replaces this process with a shell in the demo's company clone, with the
# demo's git-uplink on PATH, creating the demo first when it is missing.
demo_enter() {
  S="$HERE/scratch"
  if [ ! -d "$S/company" ] || [ ! -e "$S/bin/git-uplink" ]; then
    "$HERE/make-demo.sh"
  fi
  cd "$S/company" || exit 1
  PATH="$S/bin:$PATH"
  export PATH
  echo "Entered the $(basename "$HERE") demo in $S/company"
  echo "git uplink runs $S/bin/git-uplink. Type exit to leave."
  exec "${SHELL:-/bin/sh}"
}
