# Forge packs

`git uplink init --forge <forge>` installs one of these packs as the tooling patch on company `main`. Each pack folder is embedded in the binary and written into the product repository, except its `README.md`.

| `--forge` | Pack | Docs |
| --- | --- | --- |
| `github` | [`github/`](github/) | [GitHub workflows](github/README.md) |
| `try-it-on-github` | [`github/`](github/) + [`try-it-on-github/`](try-it-on-github/) | [Try it on GitHub](try-it-on-github/README.md) |

[`github/`](github/) is the base pack for github.com and GitHub Enterprise Cloud: workflows, the pull request template, the `install-git-uplink` action, the actions that call the company hooks, and sample rulesets. Paths in it are relative to the product repository.

Other forges are overlays on the base. A file with the same path replaces the base file, a new path is added, and `Forge::omitted_paths` (`src/types.rs`) lists base files the forge leaves out. Keep an overlay to what really differs. `ghec` and `example-github` are accepted as former names of `github` and `try-it-on-github`.

[`github-hooks/`](github-hooks/) is not installed on `main`. `git uplink init` commits it as the local orphan branch `uplink/hooks`, which `git uplink push` publishes: the [assessment hook guide](github-hooks/assessment-hook.md) and starter, and the [toolchain hook](github-hooks/toolchain-hook.md) stub that preflight jobs call to set up the runner.
