# git-uplink

**Build on a public project from a private forge — and still contribute back cleanly.**

`git uplink` is a Git subcommand for companies that must build on a public project, keep unreleased work private, pass IP review, and contribute through an **upstream-owned public contribution fork**. GitHub Enterprise Cloud with Enterprise Managed Users is one such forge.

📖 **Docs, playbook, and a live lab:** [npetzall.github.io/git-uplink](https://npetzall.github.io/git-uplink/)

## The model

Developers keep **one internal branch per change**. Merge is the product gate. Company `main` is always:

```text
public upstream/main  +  tooling  +  active upstream[]  +  active internal[]
```

1. **Write** — branch from company `main`; write the PR as if it were the upstream submission.
2. **Product gate** — review and merge. The change is recorded as `queued`.
3. **IP gate** — the `to-upstream` GitHub Environment; reviewers read a committed assessment packet.
4. **Contribute** — the approved patch is pushed to the contribution fork and a public PR is opened.
5. **Sync** — upstream moves are pulled in; merged contributions drop out of the queue on their own.

Company-only changes take the `uplink:internal-only` path and never reach the second gate.

## Who does what

| Role | Does | Never | Guide |
| --- | --- | --- | --- |
| **Developer** | Branches from company `main`, opens and merges PRs, works in the gated PRs the forge opens, dispatches amend, transfer and submit. | Runs `git uplink`, pushes `main`, rewrites a patch by hand. | [Developer](https://npetzall.github.io/git-uplink/developer) |
| **Forge** | The workflows of the pack: import, sync, submit, rebase, resolve, transfer, amend. | Touches a developer's branch unless asked. | [Forge](https://npetzall.github.io/git-uplink/forge) |
| **Operator** | Sets up the repositories, runs `init` and `init --upgrade`, adopts history, troubleshoots, repairs by hand. | Stands in for the forge or rewrites a developer's patch. | [Operator](https://npetzall.github.io/git-uplink/operator) |

## Install

On macOS or Linux, install into `~/.uplink` with the signed `install.sh` from the latest release. It needs `curl` and [cosign](https://docs.sigstore.dev/cosign/system_config/installation/):

```bash
curl -fsSLO https://github.com/npetzall/git-uplink/releases/latest/download/install.sh
curl -fsSLO https://github.com/npetzall/git-uplink/releases/latest/download/install.sh.sigstore.json
cosign verify-blob install.sh --bundle install.sh.sigstore.json \
  --certificate-identity https://github.com/npetzall/git-uplink/.github/workflows/release.yml@refs/heads/main \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
sh install.sh
```

It keeps each version as `~/.uplink/bin/git-uplink-X.Y.Z` with `~/.uplink/bin/git-uplink` linked to the active one, and checks every download three ways: the hash against the release's `SHA256SUMS`, the cosign signature against the release workflow, and the version the binary reports against the tag. Put `~/.uplink/bin` first on `PATH`:

```bash
echo 'export PATH="$HOME/.uplink/bin:$PATH"' >> ~/.zshrc        # zsh
echo 'export PATH="$HOME/.uplink/bin:$PATH"' >> ~/.bashrc       # bash on Linux
echo 'export PATH="$HOME/.uplink/bin:$PATH"' >> ~/.bash_profile # bash on macOS
```

Open a new shell and run `git uplink version`. After that, the binary manages itself, with the same checks:

```bash
git uplink self-available        # the ten latest releases
git uplink self-check            # is a newer version installed or released?
git uplink self-update           # install the latest and switch to it
git uplink self-update 1.2.3     # install a given version and switch to it
git uplink self-list             # installed versions, * marks the active one
git uplink self-use 1.2.3        # switch back to an installed version
```

Releases come from the repository in `~/.uplink/config.toml`, which `install.sh` writes. A fork points its users at its own releases and its own release workflow:

```toml
repo = "acme/git-uplink"
signer = "https://github.com/acme/git-uplink/.github/workflows/release.yml@refs/heads/main"
```

`sh install.sh --repo acme/git-uplink` writes that file on a first install.

### By hand, and on Windows

Every release asset is signed with cosign by the release workflow. Releases are [immutable](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases): the tag cannot move and assets cannot be replaced. With the GitHub CLI you can also check that and the build provenance:

```bash
target=aarch64-apple-darwin   # or x86_64-unknown-linux-musl, aarch64-unknown-linux-musl
gh release download --repo npetzall/git-uplink --pattern "git-uplink-$target" --pattern "git-uplink-$target.sigstore.json" --pattern SHA256SUMS
grep " git-uplink-$target\$" SHA256SUMS | shasum -a 256 -c
cosign verify-blob "git-uplink-$target" --bundle "git-uplink-$target.sigstore.json" \
  --certificate-identity https://github.com/npetzall/git-uplink/.github/workflows/release.yml@refs/heads/main \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
gh release verify-asset --repo npetzall/git-uplink "git-uplink-$target"
gh attestation verify "git-uplink-$target" --repo npetzall/git-uplink
chmod +x "git-uplink-$target"
mkdir -p ~/.local/bin && mv "git-uplink-$target" ~/.local/bin/git-uplink
git uplink version
```

The `gh release verify-asset` line checks that the file matches GitHub's attestation for the latest release. The `gh attestation verify` line checks that the binary was built by this repository's release workflow, from the commit the release names. Releases published before build attestations were added have none, and that line fails for them. On Windows, download `git-uplink-x86_64-pc-windows-msvc.exe`, rename it to `git-uplink.exe`, and place it in a directory on `PATH`.

The man pages are in the binary. To make `git uplink --help` and `man git-uplink` work, write them to a `man1` directory on your man path:

```bash
git uplink man ~/.local/share/man/man1
```

Only the operator installs it; the forge installs its own copy in every job, and developers need none. Installing only adds the command. Using it needs a set of repositories wired together — pick one of the paths below. To build from source, see [CONTRIBUTING.md](CONTRIBUTING.md).

## Try it for yourself

The [GitHub example](https://npetzall.github.io/git-uplink/examples) runs the full model in a new GitHub organization: an upstream repo, its contribution fork, and a private internal repo, wired with fine-grained tokens. Bootstrap scripts do the setup; seven short stories walk through submit, conflicts, dependencies, and internal-only changes.

No GitHub account handy? The [live lab](https://npetzall.github.io/git-uplink/lab) plays the same stories in the browser.

## Production setup

[Production setup](https://npetzall.github.io/git-uplink/setup) walks through setting up a GitHub Enterprise Cloud repository step by step, in the web UI or with the gh CLI: the contribution fork, GitHub Apps, variables and secrets, the `to-upstream` / `from-upstream` / `abandon-contrib` Environments, labels, and rulesets, ending with `git uplink init` and the first push.

## Documentation

| Topic | Site | Source |
| --- | --- | --- |
| Why it exists | [Why](https://npetzall.github.io/git-uplink/why) | `site/` |
| How it works | [How](https://npetzall.github.io/git-uplink/how) | `site/` |
| What developers do | [Developer](https://npetzall.github.io/git-uplink/developer) | `site/` |
| Setup, upgrade, adopt, repairs | [Operator](https://npetzall.github.io/git-uplink/operator) | `site/` |
| What each workflow does and what starts it | [Forge](https://npetzall.github.io/git-uplink/forge) | `site/` |
| Every command and flag, credentials | [CLI](https://npetzall.github.io/git-uplink/cli) | [docs/cli.md](docs/cli.md) |
| Try it on three GitHub repos | [Try it yourself](https://npetzall.github.io/git-uplink/examples) | [examples/github/](examples/github/) |
| Step-by-step setup; what each workflow does and needs | [Production setup](https://npetzall.github.io/git-uplink/setup) | [templates/github/README.md](templates/github/README.md) (workflows) |
| Every flow, gates, diagrams | [Internals](https://npetzall.github.io/git-uplink/internals) | `site/` |
| What Uplink guards, and what the operator must set up | [Security](https://npetzall.github.io/git-uplink/security) | `site/` |
| Hooks on `uplink/hooks` | [Hooks](https://npetzall.github.io/git-uplink/setup?forge=github&view=hooks) | [templates/github-hooks/](templates/github-hooks/) |

## Contributing

Building from source, tests, CI, and releases: [CONTRIBUTING.md](CONTRIBUTING.md).
