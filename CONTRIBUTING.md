# Contributing

Conventions (commit style, required checks) are in [AGENTS.md](AGENTS.md). This page covers building, testing, and how CI and releases work.

## Toolchain

- **Rust 1.98+** (`rust-toolchain.toml` pins 1.98.1)
- **Node.js 22** and **pnpm** (for the embedded operator UI and the site). Install the standalone pnpm from [pnpm.io](https://pnpm.io/installation); it switches to the version in the root `package.json` `packageManager` field on first use.

- **Docker**, for the shellcheck and actionlint pre-commit hooks.
- **[pre-commit](https://pre-commit.com/)**: `sfw uv tool install pre-commit --with pre-commit-uv`, then `pre-commit install` in the clone.

Prefix every `pnpm` and `cargo` command with `sfw` ([Socket Firewall Free](https://docs.socket.dev/docs/socket-firewall-free)).

## Layout

| Path | What |
| --- | --- |
| `src/` | The `git-uplink` crate |
| `web/` | Operator UI (Vite + React + Tailwind), embedded in the binary |
| `site/` | Public site on GitHub Pages (same stack) |
| `pnpm-workspace.yaml` | pnpm workspace of `web/` and `site/`, with the catalog of the dependency versions they share |
| `templates/` | Forge packs installed by `git uplink init` |
| `examples/github/` | Worked example on three GitHub repositories |
| `docs/` | Reference docs, also rendered on the site |

## Build and install a development version

End users should install a [release binary](README.md#install). To run what is on your branch:

```bash
sfw cargo install --path .
git uplink version
```

Or, without installing into Cargo’s bin directory:

```bash
sfw cargo build --release
export PATH="$PWD/target/release:$PATH"
```

`git uplink version` shows the commit the binary was built from, with `-dirty` when the tree had local changes.

`build.rs` runs `pnpm install --frozen-lockfile --filter git-uplink-web...` and `pnpm --filter git-uplink-web run build` and embeds `web/dist` with `rust-embed`. Set `GIT_UPLINK_SKIP_WEB_BUILD=1` to embed an existing `web/dist` instead of running pnpm. Do not commit `web/dist` or `site/dist`.

To read a man page as your branch builds it, run `scripts/man.sh` for `git-uplink(1)` or `scripts/man.sh <command>` for one command. It writes the pages to `target/man`.

## Test

```bash
sfw cargo test --locked
sfw pnpm -r test
sfw pnpm audit
sfw cargo deny check
sfw cargo clippy --locked --all-targets -- -D warnings
```

- **Rust suite** drives real git in temp repos: stacked patches, drop-on-merge, conflicts, concurrent adds, export preflight, assess/scrub, contribution packets, plus a check that the UI was embedded.
- **Lab scenario tests** in `site/`: drop-on-merge, internal-only staying off the fork, queued work staying off the fork until to-upstream approval, and every lab step completing.
- **Typecheck**: `sfw pnpm -r run typecheck`.
- **Lint**: `sfw pnpm -r run lint` checks, `sfw pnpm -r run format` applies the fixes.

## Pre-commit

`.pre-commit-config.yaml` runs the checks CI runs: rustfmt, clippy, `docs/cli.md`, cargo deny, pnpm audit, Biome, ruff, [zizmor](https://zizmor.sh/), [actionlint](https://github.com/rhysd/actionlint) and [ShellCheck](https://www.shellcheck.net/). Run every hook on every file with:

```bash
sfw uv tool run --with pre-commit-uv pre-commit run --all-files
```

Running it under `sfw` installs the Python hook environments (ruff, zizmor, PyYAML) through the firewall.

- **actionlint** checks this repository's workflows and the forge pack's (`templates/*/.github/workflows/`), with the settings in `.github/actionlint.yaml`. It runs ShellCheck on their `run:` steps.
- **ShellCheck** checks the shell scripts, and `.github/shellcheck_actions.py` checks the `run:` steps of composite actions (`action.yml`), which actionlint does not.
- Both run in Docker images pinned by digest, because `sfw` does not cover image pulls. To upgrade one, bump `rev`, then take the digest of the new tag (`docker buildx imagetools inspect docker.io/koalaman/shellcheck:<tag>`) and replace every reference to the image: `.pre-commit-config.yaml` (both ShellCheck hooks) and, for actionlint, `.github/workflows/actionlint.yml`.

## CI

### Pull requests (`pr.yml`)

Runs on every PR. The `Changed scopes` job passes the changed paths to `.github/changed-scopes.sh`, which decides which of two scopes apply:

- **Product**, when the PR changes `src/`, `web/`, `templates/`, the Rust manifests, `build.rs`, `Cross.toml`, `rust-toolchain.toml`, tests, anything under `.github/`, or the shell scripts in `scripts/`, `examples/github/reset/` and `demo/`. The last two are there so that pre-commit (actionlint, ShellCheck, zizmor) runs on every workflow and script.
- **Site**, when it changes `site/`, the markdown the site renders (`docs/`, `templates/*/README.md`, `examples/github/stories/`), or the site workflows.

`Merge gate` is the only required check. It always runs, and fails when any job failed or was cancelled. A scope that did not apply is skipped, and that passes.

### Product (`product.yml`)

For the product scope, `pr.yml` calculates the next version and calls `product.yml` with a pre-release suffix `-pr.<number>.<run>.<attempt>`.

1. `cargo deny` and `pnpm audit` of the workspace lockfile run first.
2. Then `web/` is built once (`sfw pnpm install`, lint, typecheck, vitest, production build) and CycloneDX and SPDX SBOMs are generated.
3. Test, the musl Linux binary, and pre-commit run in parallel with `GIT_UPLINK_SKIP_WEB_BUILD=1`. CI's pre-commit skips the hooks other jobs already run: pnpm audit and cargo deny (step 1), Biome (the `web/` and site jobs), and `docs/cli.md` (the `cli_docs` test).

CodeQL for Rust and `web/`, zizmor, actionlint, and Socket start immediately and do not gate the build. Each job that compiles or scans the crate writes the version into `Cargo.toml` and `Cargo.lock`. SBOMs are always uploaded as an artifact.

### Site (`site.yml`)

Called by `pr.yml` for the site scope, and by `publish-site.yml`.

- `sfw pnpm audit`, lint, typecheck, and vitest run in parallel with site CodeQL, zizmor, actionlint, and Socket, then a separate build job.
- pnpm audit, zizmor, actionlint, and Socket cover the whole repository. When a PR changes both scopes, `pr.yml` passes `scans: false`, so they run once, in `product.yml`.
- The build uses the Pages base path and uploads the Pages artifact only when the caller sets `pages: true`.

### GitHub Pages (`publish-site.yml`)

Runs on push to `main` when `site/`, the markdown it renders, the pnpm workspace files, or the site workflows change.

- It calls `site.yml` with `pages: true`, and deploys only after every job in it succeeds.
- A push that only changes the site does not run the product release.
- Pages must be enabled under **Settings → Pages → Source: GitHub Actions**.

### Scheduled

A Monday schedule runs CodeQL for Rust, `web/`, and `site/`, plus zizmor, actionlint, and Socket. zizmor and actionlint upload their findings to code scanning and do not fail the job; their pre-commit hooks are the gate.

## Release (`release.yml`)

Pushes to `main` that match the product path filter run the release.

- The version comes from the latest `vX.Y.Z` tag and the conventional commits since that tag (no tag starts at `0.0.0`).
- The workflow calls `product.yml` with that version and every release target. Each build job writes the version with `python3 .github/update_version_in_cargo.py "${PRODUCT_VERSION}"`.
- A GitHub Release is published only after that job succeeds and only for a new version: `gh release create --draft --generate-notes` with every asset, then `gh release edit --draft=false`. An unchanged version skips the publish job.
- Releases are immutable (repository setting): once published, the tag cannot move and assets cannot be added, replaced, or deleted. GitHub creates a release attestation for each one. A broken release is fixed by the next version, not in place.
- Assets: Apple Silicon, Linux musl (amd64 and arm64), and Windows binaries, `SHA256SUMS`, and `git-uplink-<version>.cdx.json` / `git-uplink-<version>.spdx.json`.
- Every asset is signed with cosign (keyless) in the publish job and gets a `<asset>.sigstore.json` bundle. The signer is `https://github.com/npetzall/git-uplink/.github/workflows/release.yml@refs/heads/main`, issuer `https://token.actions.githubusercontent.com`. The pack's install action pins that identity, so renaming `release.yml` or releasing from another branch breaks installs.
- The publish job also attests the binaries with `actions/attest`: SLSA build provenance, and the SPDX SBOM bound to them. Both list the binaries by `SHA256SUMS`. Verify with `gh attestation verify git-uplink-<target> --repo npetzall/git-uplink`.

## SBOMs

Distribution SBOMs (CycloneDX JSON and SPDX JSON) cover the Rust crate and the embedded `web/` UI. `site/` is excluded because it is not part of the shipped binary. Generate both locally with [Syft](https://github.com/anchore/syft): `syft dir:.` reads `.syft.yaml`.
