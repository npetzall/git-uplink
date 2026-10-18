# AGENTS.md

## Tech stack

Rust Git subcommand. The binary is `git-uplink`, so Git treats it as `git uplink`.

- **CLI:** Rust 1.98.1 (edition 2024, pinned in `rust-toolchain.toml`). Crates include clap, serde, tokio, axum, rust-embed, and ratatui. `cargo fmt`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo deny check` are required (also enforced by `.pre-commit-config.yaml`). Rust warnings are denied.
- **CLI docs:** `src/cli.rs` holds the clap definitions and may only depend on clap and std. Its help text is the source of `docs/cli.md`; never edit that file by hand. Regenerate it with `UPLINK_BLESS=1 sfw cargo test --locked --test cli_docs` (pre-commit does this, `cargo test` fails when it is stale). A new command must be added to `GROUPS` there, which orders the command list in `-h`, the man page, and `docs/cli.md`. `build.rs` generates the man pages from the same file and embeds them; `git uplink man <dir>` writes them out.
- **Operator UI:** `web/` is Vite 8, React 19, TypeScript 7, and Tailwind 4, with Vitest for its unit tests and Biome for linting and formatting. `pnpm run lint` checks (CI and pre-commit), `pnpm run format` applies fixes. `build.rs` runs `pnpm install` and `pnpm run build` for `web/` and embeds `web/dist` with rust-embed. Do not commit `web/dist`.
- **Public site:** `site/` is the same frontend stack plus Vitest, Marked, and Mermaid. GitHub Pages via `.github/workflows/publish-site.yml`. Do not commit `site/dist`.
- **Toolchain:** Node.js 22 and pnpm (version pinned by `packageManager` in the root `package.json`). `web/` and `site/` are one pnpm workspace (`pnpm-workspace.yaml`, one `pnpm-lock.yaml` at the root); a dependency both use goes in the workspace `catalog:` and is referenced as `"catalog:"`. Tests are `cargo test` (real git temp repos) and `pnpm -r test`. Lint with `pnpm -r run lint`. Supply-chain checks include `cargo deny`, pnpm audit, Socket Security, and Syft SBOMs.

## Pack and binary versions

The forge pack (`templates/`) and the binary are released and upgraded together. Assume a workflow always runs the binary of its own release: change CLI flags, output and the workflows that use them in the same commit, and do not add compatibility for an older pack or an older binary. This is stated for operators in `templates/github/README.md`.

## Socket Firewall

Prefix every `pnpm` and `cargo` command with `sfw` ([Socket Firewall Free](https://docs.socket.dev/docs/socket-firewall-free)). This includes install, build, test, fmt, and clippy.

```bash
sfw pnpm install --frozen-lockfile
sfw pnpm --filter git-uplink-site test
sfw pnpm --filter git-uplink-web run lint
sfw cargo test --locked
sfw cargo clippy --locked --all-targets -- -D warnings
```

## Branches

Never change or commit directly on `main`. Before the first edit, check the current branch; if it is `main`, create a branch from `origin/main` (no `git fetch`), not from local `main`, and work there:

```bash
git switch --no-track -c fix/short_description origin/main
```

Name it `type/short_description`, with the same types as commits.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/):

```text
type(scope): imperative subject
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `ci`, `chore`. Scopes when useful: `cli`, `web`, `site`, `ci`, `forge`. Subject is lowercase imperative, with no trailing period. Breaking changes use `!` or a `BREAKING CHANGE:` footer. Type and Scope `ci` is only for this project. If changes are done in templates it's scope is most likely `forge`.
