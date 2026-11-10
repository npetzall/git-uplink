#!/bin/sh
# First install of git-uplink under ~/.uplink (macOS and Linux).
#
#   sh install.sh [VERSION] [--repo owner/name] [--signer URL]
#
# VERSION is X.Y.Z, vX.Y.Z or latest (default). Every download must match the
# release's SHA256SUMS, carry a cosign signature by the signer, and report the
# version it was downloaded as. Needs curl and cosign. Later upgrades:
# `git uplink self-update`.
set -eu

repo=""
signer=""
version="latest"
issuer="https://token.actions.githubusercontent.com"
uplink_home="${UPLINK_HOME:-$HOME/.uplink}"
bin="$uplink_home/bin"

die() {
  echo "install.sh: $*" >&2
  exit 1
}

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) [ $# -ge 2 ] || die "--repo needs a value"; repo="$2"; shift 2 ;;
    --signer) [ $# -ge 2 ] || die "--signer needs a value"; signer="$2"; shift 2 ;;
    -h|--help) sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) die "unknown option: $1" ;;
    *) version="$1"; shift ;;
  esac
done

# An existing config.toml names the repo and signer unless flags override it.
config="$uplink_home/config.toml"
config_value() {
  [ -f "$config" ] || return 0
  sed -n "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p" "$config" | head -n 1
}
[ -n "$repo" ] || repo="$(config_value repo)"
[ -n "$repo" ] || repo="npetzall/git-uplink"
[ -n "$signer" ] || signer="$(config_value signer)"
[ -n "$signer" ] || signer="https://github.com/$repo/.github/workflows/release.yml@refs/heads/main"

echo "$repo" | grep -Eq '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$' || die "repo must be owner/name: $repo"
case "$signer" in https://*) ;; *) die "signer must be an https:// identity: $signer" ;; esac

command -v curl >/dev/null 2>&1 || die "curl is required"
command -v cosign >/dev/null 2>&1 ||
  die "cosign is required: https://docs.sigstore.dev/cosign/system_config/installation/"
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  die "sha256sum or shasum is required"
fi

fetch() {
  curl -fsSL --proto '=https' --proto-redir '=https' "$@"
}

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-musl ;;
  *) die "no release build for $(uname -s)/$(uname -m); install git-uplink manually" ;;
esac

if [ "$version" = latest ]; then
  url="$(fetch -I -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")"
  version="${url##*/tag/}"
  [ "$version" != "$url" ] || die "$repo has no latest release"
fi
version="${version#v}"
echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "not a version: $version"

asset="git-uplink-$target"
base="https://github.com/$repo/releases/download/v$version"
mkdir -p "$uplink_home/tmp" "$bin"
tmp="$(mktemp -d "$uplink_home/tmp/install.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Downloading v$version ($target) from $repo"
fetch -o "$tmp/$asset" "$base/$asset"
fetch -o "$tmp/$asset.sigstore.json" "$base/$asset.sigstore.json"
fetch -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"

expected="$(awk -v a="$asset" '{ n = $2; sub(/^\*/, "", n) } n == a { print tolower($1); exit }' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || die "SHA256SUMS of v$version does not list $asset"
actual="$(sha256 "$tmp/$asset")"
[ "$actual" = "$expected" ] || die "sha256 mismatch for $asset: SHA256SUMS lists $expected, got $actual"
echo "sha256 matches SHA256SUMS"

cosign verify-blob "$tmp/$asset" --bundle "$tmp/$asset.sigstore.json" \
  --certificate-identity "$signer" \
  --certificate-oidc-issuer "$issuer" ||
  die "cosign could not verify $asset as signed by $signer"
echo "cosign signature by $signer"

chmod 755 "$tmp/$asset"
reported="$("$tmp/$asset" version)" || die "the v$version download does not run"
case "$reported" in
  "git-uplink $version "*|"git-uplink $version") ;;
  *) die "the v$version download reports \"$reported\", not version $version" ;;
esac

mv "$tmp/$asset" "$bin/git-uplink-$version"
if [ -e "$bin/git-uplink" ] && [ ! -L "$bin/git-uplink" ]; then
  die "$bin/git-uplink is not a symlink; move it away and retry"
fi
ln -s "git-uplink-$version" "$bin/.git-uplink.$$"
mv -f "$bin/.git-uplink.$$" "$bin/git-uplink"

if [ ! -f "$config" ]; then
  printf 'repo = "%s"\nsigner = "%s"\n' "$repo" "$signer" >"$config"
fi
echo "Installed git-uplink $version in $bin"

case ":$PATH:" in
  *":$bin:"*)
    found="$(command -v git-uplink || true)"
    if [ "$found" != "$bin/git-uplink" ]; then
      echo "Warning: $found comes before $bin on PATH; remove it or move $bin first." >&2
    fi
    ;;
  *)
    # The single quotes are deliberate: $HOME and $PATH expand when the rc file runs.
    # shellcheck disable=SC2016
    line='export PATH="$HOME/.uplink/bin:$PATH"'
    [ "$uplink_home" = "$HOME/.uplink" ] || line="export PATH=\"$bin:\$PATH\""
    cat <<EOF

Add $bin to PATH, then open a new shell:
  zsh:            echo '$line' >> ~/.zshrc
  bash (Linux):   echo '$line' >> ~/.bashrc
  bash (macOS):   echo '$line' >> ~/.bash_profile

Then check it with: git uplink version
EOF
    ;;
esac
