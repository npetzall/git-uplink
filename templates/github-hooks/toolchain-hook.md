# Uplink toolchain hook

Preflight applies a change onto public upstream and runs the `preflight` command from `uplink.toml` on the runner. The toolchain hook installs what that command needs: language runtimes, package managers, system packages, caches.

Hooks are company-only. They live on the orphan branch `uplink/hooks`, never on `main`, so they are never queued, replayed, or contributed.

## When it runs

Every Uplink job that runs preflight calls the hook right before its Uplink step:

- **Uplink PR checks:** job *Uplink upstream preflight*;
- **Uplink gate:** conflict and transfer PRs;
- **Uplink import**, **Uplink submit**, **Uplink amend**, and **Uplink transfer** (start and complete).

The forge pack's `.github/actions/uplink-toolchain-hook` on `main` checks out `uplink/hooks` into `.uplink-hooks/` and runs `.github/actions/uplink-toolchain-hook/action.yml` from there. If that file is missing on `uplink/hooks`, the job shows a notice and carries on without it. **A failed hook fails the job.**

## How it is wired

- `git uplink init` creates `uplink/hooks` locally with this file, a stub hook that only prints it, and `uplink.toml`, which holds the `preflight` command. `git uplink push` publishes it with `uplink/state`. `git uplink init --upgrade` adds files a newer pack brings, without changing the ones you have. `git uplink doctor` reports when it is missing, lacks the hook, or is not pushed.
- Your hook is `.github/actions/uplink-toolchain-hook/action.yml` on `uplink/hooks`. Edit it there.

## Contract

- A [composite action](https://docs.github.com/actions/sharing-automations/creating-actions/creating-a-composite-action) with no inputs.
- It runs in the caller's job, after the product is checked out in the workspace. Paths in the action are relative to the workspace; use `${{ github.action_path }}` for files that sit next to it on `uplink/hooks`.
- Install tools, set `PATH` through `$GITHUB_PATH`, and set environment through `$GITHUB_ENV`. Later steps, including the preflight command, see them.
- Do not build, test, or run product code here. Preflight does that, on the change it applies.
- Do not change the workspace checkout or push anything.

**Some callers hold write tokens and Environment secrets** (import, submit, amend, transfer). The hook runs in those jobs. Keep it to installing tools from sources you trust, pinned to a version or commit.

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

Then set `preflight` in `uplink.toml` on this branch, for example `npm ci && npm test`, `./gradlew check`, or `cargo test --locked`. `git uplink init` asked for it when it created the file.

## Change the hook

A push to `uplink/hooks` that changes only `.github/actions/` does not need `workflows` write. With the recommended ruleset (`.github/uplink-hooks-ruleset.json` on `main`), open a pull request against `uplink/hooks`.
