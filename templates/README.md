# Forge packs

`git uplink init --forge <forge>` installs one of these packs as the tooling patch on company `main`. Each pack folder is embedded in the binary and written into the product repository, except its `README.md`.

| `--forge` | Pack | Docs |
| --- | --- | --- |
| `ghec` | [`ghec/`](ghec/) | [GitHub Enterprise Cloud setup](ghec/README.md) |
| `example-github` | [`example-github/`](example-github/) | [GitHub example setup](../examples/github/SETUP.md) |

[`github/`](github/) holds files shared by every GitHub-family pack: the pull request template, the `install-git-uplink` action, and a sample pack-files ruleset.
