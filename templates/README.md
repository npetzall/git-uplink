# Forge packs

`git uplink init --forge <forge>` installs one of these packs as the tooling patch on company `main`. Each pack folder is embedded in the binary and written into the product repository, except its `README.md`.

| `--forge` | Pack | Docs |
| --- | --- | --- |
| `ghec` | [`ghec/`](ghec/) | [GitHub Enterprise Cloud setup](ghec/README.md) |
| `example-github` | [`example-github/`](example-github/) | [GitHub example workflows](example-github/README.md) |

[`github/`](github/) holds files shared by every GitHub-family pack: the pull request template, the `install-git-uplink` action, the actions that call the company hooks, and sample rulesets.

[`github-hooks/`](github-hooks/) is not installed on `main`. `git uplink init` commits it as the local orphan branch `uplink/hooks`, which `git uplink push` publishes: the [assessment hook guide](github-hooks/assessment-hook.md) and starter, and the [toolchain hook](github-hooks/toolchain-hook.md) stub that preflight jobs call to set up the runner.
