# Contributing

Conventions (commit style, required checks) are in [AGENTS.md](AGENTS.md). This page covers building, testing, and how CI and releases work.

## Toolchain

- **Rust 1.98+** (`rust-toolchain.toml` pins 1.98.1)
- **Node.js 22** and `npm` (for the embedded operator UI and the site)

Prefix every `npm` and `cargo` command with `sfw` ([Socket Firewall Free](https://docs.socket.dev/docs/socket-firewall-free)).

## Layout

| Path | What |
| --- | --- |
| `src/` | The `git-uplink` crate |
| `web/` | Operator UI (Vite + React + Tailwind), embedded in the binary |
| `site/` | Public site on GitHub Pages (same stack) |
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

`build.rs` runs `npm ci` and `npm run build` in `web/` and embeds `web/dist` with `rust-embed`. Set `GIT_UPLINK_SKIP_WEB_BUILD=1` to embed an existing `web/dist` instead of running npm. Do not commit `web/dist` or `site/dist`.

## Test

```bash
sfw cargo test --locked
sfw npm test --prefix site
sfw cargo deny check
sfw cargo clippy --locked --all-targets -- -D warnings
```

- **Rust suite** drives real git in temp repos: stacked patches, drop-on-merge, conflicts, concurrent adds, export preflight, assess/scrub, contribution packets, plus a check that the UI was embedded.
- **Lab scenario tests** in `site/`: drop-on-merge, internal-only staying off the fork, queued work staying off the fork until to-upstream approval, and every lab step completing.
- **Typecheck**: `sfw npm run typecheck --prefix site` and `sfw npm run typecheck --prefix web`.

## CI

### Product pull requests (`pr.yml`)

Runs when a PR changes `src/`, `web/`, `templates/`, the Rust manifests, `build.rs`, `Cross.toml`, `rust-toolchain.toml`, tests, or the product workflows. It calculates the next version and calls `product.yml` with a pre-release suffix `-pr.<number>.<run>.<attempt>`.

1. `cargo deny` and the `web/` `npm audit --package-lock-only` run first.
2. Then `web/` is built once (`sfw npm ci`, typecheck, production build) and CycloneDX and SPDX SBOMs are generated.
3. Test, the musl Linux binary, and pre-commit (rustfmt, clippy, cargo deny, [zizmor](https://zizmor.sh/)) run in parallel with `GIT_UPLINK_SKIP_WEB_BUILD=1`.

CodeQL for Rust and `web/`, zizmor, and Socket start immediately and do not gate the build. Each job that compiles or scans the crate writes the version into `Cargo.toml` and `Cargo.lock`. SBOMs are always uploaded as an artifact.

### Site (`site.yml`)

Runs on site pull requests and on push to `main` when `site/` or the markdown it renders (`docs/`, `templates/*/README.md`, `examples/github/stories/`) changes.

- `sfw npm audit`, typecheck, and vitest run in parallel with site CodeQL and zizmor, then a separate build job.
- The Pages artifact is uploaded, and deploy runs, only on `main`, after that build, site CodeQL, and zizmor succeed.
- A push that only changes the site does not run the product release.
- Pages must be enabled under **Settings → Pages → Source: GitHub Actions**.

### Scheduled

A Monday schedule runs CodeQL for Rust, `web/`, and `site/`, plus zizmor and Socket.

## Release (`release.yml`)

Pushes to `main` that match the product path filter run the release.

- The version comes from the latest `vX.Y.Z` tag and the conventional commits since that tag (no tag starts at `0.0.0`).
- The workflow calls `product.yml` with that version and every release target. Each build job writes the version with `python3 .github/update_version_in_cargo.py "${PRODUCT_VERSION}"`.
- A GitHub Release is published only after that job succeeds: `gh release create --generate-notes` for a new tag, or `gh release upload --clobber` to refresh assets when the tag is already current.
- Assets: Apple Silicon, Linux musl (amd64 and arm64), and Windows binaries, `SHA256SUMS`, and `git-uplink-<version>.cdx.json` / `git-uplink-<version>.spdx.json`.

## SBOMs

Distribution SBOMs (CycloneDX JSON and SPDX JSON) cover the Rust crate and the embedded `web/` UI. `site/` is excluded because it is not part of the shipped binary. Generate both locally with [Syft](https://github.com/anchore/syft): `syft dir:.` reads `.syft.yaml`.
