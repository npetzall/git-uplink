# TODO

Steps that need someone with admin rights on the repository, because the
code alone cannot do them. Delete a one-time item once it is done.

## One-time

- [ ] **Create the `release` environment.** Settings → Environments → New
  environment → `release`. Under deployment branches, choose "Selected
  branches" and allow only `main`. Add required reviewers if a release should
  wait for approval.

  Why: the `publish` job in `.github/workflows/release.yml` holds
  `contents: write`, `id-token: write` and `attestations: write`, and now runs
  in `environment: release`. If the environment is missing, GitHub creates it
  on the first run with no protection, and the job gets nothing from it. The
  cosign signing identity (workflow path and ref) does not change.

- [ ] **Enable private vulnerability reporting.** Settings → Security →
  Private vulnerability reporting → Enable.

  Why: `SECURITY.md` sends reporters to
  `https://github.com/npetzall/git-uplink/security/advisories/new`, and that
  page only works while the feature is on.

## Recurring

- [ ] **Bump the Socket Firewall version.** Watch
  [SocketDev/sfw-free releases](https://github.com/SocketDev/sfw-free/releases).
  When a release is at least 7 days old, set `firewall-version` to its tag on
  every `SocketDev/action` step (`.github/workflows/product.yml`, `site.yml`,
  `socket.yml`) in one commit. The current pin is `v1.16.0`.

  Why: the action downloads sfw without a checksum, so the version is pinned
  instead of `latest`. Dependabot does not track that input. See the CI rules
  in `AGENTS.md`.
