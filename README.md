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

## Install

Download the binary for your platform from [Releases](https://github.com/npetzall/git-uplink/releases) and put it on `PATH` as `git-uplink`. With the GitHub CLI on macOS or Linux:

```bash
target=aarch64-apple-darwin   # or x86_64-unknown-linux-musl, aarch64-unknown-linux-musl
gh release download --repo npetzall/git-uplink --pattern "git-uplink-$target" --pattern SHA256SUMS
grep " git-uplink-$target\$" SHA256SUMS | shasum -a 256 -c
chmod +x "git-uplink-$target"
mkdir -p ~/.local/bin && mv "git-uplink-$target" ~/.local/bin/git-uplink
git uplink version
```

Make sure `~/.local/bin` is on your `PATH`. On Windows, download `git-uplink-x86_64-pc-windows-msvc.exe`, rename it to `git-uplink.exe`, and place it in a directory on `PATH`.

Installing only adds the command. Using it needs a set of repositories wired together — pick one of the paths below. To build from source, see [CONTRIBUTING.md](CONTRIBUTING.md).

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
| What developers do | [Day to day](https://npetzall.github.io/git-uplink/day-to-day) | `site/` |
| Every command and flag, credentials | [CLI](https://npetzall.github.io/git-uplink/cli) | [docs/cli.md](docs/cli.md) |
| Try it on three GitHub repos | [Try it yourself](https://npetzall.github.io/git-uplink/examples) | [examples/github/](examples/github/) |
| Step-by-step setup; what each workflow does and needs | [Production setup](https://npetzall.github.io/git-uplink/setup) | [templates/ghec/README.md](templates/ghec/README.md) (workflows) |
| From init to sync, gates, diagrams | [Internals](https://npetzall.github.io/git-uplink/internals) | `site/` |

## Contributing

Building from source, tests, CI, and releases: [CONTRIBUTING.md](CONTRIBUTING.md).
