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

# Creates the demo when it is missing, then moves into its company clone with
# the demo's git-uplink first on PATH.
demo_cd() {
  S="$HERE/scratch"
  if [ ! -d "$S/company" ] || [ ! -e "$S/bin/git-uplink" ]; then
    "$HERE/make-demo.sh"
  fi
  cd "$S/company" || exit 1
  PATH="$S/bin:$PATH"
  export PATH
}

# Replaces this process with `git-uplink "$@"` in the demo's company clone.
demo_run() {
  demo_cd
  exec git-uplink "$@"
}

# Replaces this process with a shell in the demo's company clone. The shell
# reads no startup files: they may put another git-uplink ahead of the demo's
# on PATH.
demo_enter() {
  demo_cd
  NAME=$(basename "$HERE")
  echo "Entered the $NAME demo in $S/company"
  echo "git uplink runs $(command -v git-uplink)."
  echo "Your shell startup files are skipped. Type exit to leave."
  unset ENV
  case "$(basename "${SHELL:-/bin/sh}")" in
    zsh) PS1="uplink-demo:$NAME %~ %# " && export PS1 && exec zsh -f ;;
    bash) PS1="uplink-demo:$NAME \\w \\$ " && export PS1 && exec bash --norc --noprofile ;;
    *) PS1="uplink-demo:$NAME \$ " && export PS1 && exec /bin/sh ;;
  esac
}
