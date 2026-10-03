# AGENTS.md

## Tech stack

Rust Git subcommand. The binary is `git-uplink`, so Git treats it as `git uplink`.

- **CLI:** Rust 1.98.1 (edition 2024, pinned in `rust-toolchain.toml`). Crates include clap, serde, tokio, axum, rust-embed, and ratatui. `cargo fmt`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo deny check` are required (also enforced by `.pre-commit-config.yaml`). Rust warnings are denied.
- **CLI docs:** `src/cli.rs` holds the clap definitions and may only depend on clap and std. Its help text is the source of `docs/cli.md`; never edit that file by hand. Regenerate it with `UPLINK_BLESS=1 sfw cargo test --locked --test cli_docs` (pre-commit does this, `cargo test` fails when it is stale). A new command must be added to `GROUPS` there, which orders the command list in `-h`, the man page, and `docs/cli.md`. `build.rs` generates the man pages from the same file and embeds them; `git uplink man <dir>` writes them out.
- **Operator UI:** `web/` is Vite 8, React 19, TypeScript 7, and Tailwind 4. `build.rs` runs `npm ci` and `npm run build` and embeds `web/dist` with rust-embed. Do not commit `web/dist`.
- **Public site:** `site/` is the same frontend stack plus Vitest, Marked, and Mermaid. GitHub Pages via `.github/workflows/site.yml`. Do not commit `site/dist`.
- **Toolchain:** Node.js 22. Tests are `cargo test` (real git temp repos) and `npm test --prefix site`. Supply-chain checks include `cargo deny`, npm audit, Socket Security, and Syft SBOMs.

## Socket Firewall

Prefix every `npm` and `cargo` command with `sfw` ([Socket Firewall Free](https://docs.socket.dev/docs/socket-firewall-free)). This includes install, build, test, fmt, and clippy.

```bash
sfw npm ci --prefix web
sfw npm test --prefix site
sfw cargo test --locked
sfw cargo clippy --locked --all-targets -- -D warnings
```

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/):

```text
type(scope): imperative subject
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `build`, `ci`, `chore`. Scopes when useful: `cli`, `web`, `site`, `ci`, `forge`. Subject is lowercase imperative, with no trailing period. Breaking changes use `!` or a `BREAKING CHANGE:` footer. Type and Scope `ci` is only for this project. If changes are done in templates it's scope is most likely `forge`.
