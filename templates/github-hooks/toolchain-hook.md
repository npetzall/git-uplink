# Uplink toolchain hook

Preflight applies a change onto public upstream and runs `preflight.sh` from this branch on the runner. The toolchain hook installs what that script needs: language runtimes, package managers, system packages, caches.

Hooks are company-only. They live on the orphan branch `uplink/hooks`, never on `main`, so they are never queued, replayed, or contributed.

## When it runs

Every Uplink job that runs `preflight.sh` calls the hook right before the step that runs it:

- **Uplink PR checks:** job *Uplink upstream preflight*;
- **Uplink gate:** conflict, amend and transfer PRs;
- **Uplink import**, **Uplink submit**, **Uplink amend** (complete), and **Uplink transfer** (start and complete): their preflight job.

Those jobs hold a read-only Actions token and nothing else: no App token, no Environment, no credentials in the checkout. The job that writes (import, submit, amend, transfer) takes the preflight job's result and runs neither the hook nor `preflight.sh`.

The forge pack's `.github/actions/uplink-toolchain-hook` on `main` checks out `uplink/hooks` into `.uplink-hooks/` and runs `.github/actions/uplink-toolchain-hook/action.yml` from there. If that file is missing on `uplink/hooks`, the job shows a notice and carries on without it. **A failed hook fails the job.**

## How it is wired

- `git uplink init` creates `uplink/hooks` locally with this file, a stub hook that only prints it, `preflight.sh`, and `uplink.toml` (the CLI's settings). `git uplink push` publishes it with `uplink/state`. `git uplink init --upgrade` adds files a newer pack brings, without changing the ones you have. `git uplink doctor` reports when it is missing, lacks the hook, or is not pushed.
- Your hook is `.github/actions/uplink-toolchain-hook/action.yml` on `uplink/hooks`. Edit it there.

## Contract

- A [composite action](https://docs.github.com/actions/sharing-automations/creating-actions/creating-a-composite-action) with no inputs.
- It runs in the caller's job, after the product is checked out in the workspace. Paths in the action are relative to the workspace; use `${{ github.action_path }}` for files that sit next to it on `uplink/hooks`.
- Install tools, set `PATH` through `$GITHUB_PATH`, and set environment through `$GITHUB_ENV`. Later steps, including `preflight.sh`, see them.
- Do not build, test, or run product code here. Preflight does that, on the change it applies.
- Do not change the workspace checkout or push anything.

The hook never runs in a job that holds write tokens or Environment secrets. Still keep it to installing tools from sources you trust, pinned to a version or commit: it runs before every preflight.

## Examples

Replace the stub's steps with what you need. Pin third-party actions to a full commit SHA.

Node.js with an npm cache:

```yaml
name: Uplink toolchain hook
runs:
  using: composite
  steps:
    - uses: actions/setup-node@<sha> # v5
      with:
        node-version: 22
        cache: npm
```

Java with Gradle:

```yaml
name: Uplink toolchain hook
runs:
  using: composite
  steps:
    - uses: actions/setup-java@<sha> # v5
      with:
        distribution: temurin
        java-version: 21
    - uses: gradle/actions/setup-gradle@<sha> # v5
```

Rust and system packages:

```yaml
name: Uplink toolchain hook
runs:
  using: composite
  steps:
    - name: System packages
      shell: bash
      run: sudo apt-get update && sudo apt-get install -y --no-install-recommends pkg-config libssl-dev
    - name: Rust toolchain
      shell: bash
      run: rustup toolchain install stable --profile minimal
```

## `preflight.sh`

Put the build and test commands in `preflight.sh` on this branch, for example `npm ci && npm test`, `./gradlew check`, or `cargo test --locked`. `git uplink init` asked for the command when it created the file.

- Preflight runs it as `sh preflight.sh` from the root of the tree under test. A non-zero exit fails preflight.
- What it prints is shown in the job log as it runs, stdout and stderr together. On a failure it is also in the PR comment.
- This branch is checked out beside the script for the run, so it can call other files here through `"$(dirname "$0")"`.
- `GITHUB_TOKEN`, `GH_TOKEN`, and `UPLINK_*_TOKEN` / `UPLINK_*_KEY` are removed from its environment. Do not export other secrets through `$GITHUB_ENV` unless the build needs them: the script sees them.
- It builds and runs product code, public upstream's included, so it never runs next to a credential. In CI, `git uplink` refuses to run it in a step that holds a token. The pack runs it in a job of its own and hands the result to the job that writes (`--preflight-result`).

Known limitations:

- **The script can read company source.** Its job holds a read-only Actions token for the company repository, and the tree under test sits in a clone of it. Code the script builds and runs, public upstream's included, can read what that token and that clone can. It cannot write, and it cannot reach the App tokens or Environment secrets.
- **On a developer's machine the script runs as the developer.** The token names are removed from its environment, but it can still read what the user can: other processes, credential stores, any file. The refusal to run next to a credential applies in CI only. Review the script and the tree it builds as you would any build you run locally.
- **The verdict is the script's own exit code.** You write the script, so that is as intended. Code it runs could make it exit 0. Preflight checks that a change builds and passes its tests; it is not a defence against hostile code in the tree.

To try a change before it lands, commit it on a branch made from `uplink/hooks` and run:

```bash
git uplink preflight --command-only --hooks <branch>
```

## Change the hook

A push to `uplink/hooks` that changes only `.github/actions/` does not need `workflows` write. With the recommended ruleset (`.github/uplink-hooks-ruleset.json` on `main`), open a pull request against `uplink/hooks`.
