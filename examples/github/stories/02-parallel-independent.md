# Story 2 — Asha and Ben in parallel; Ben merges first

Independent changes (hash vs TTL). Queue order is not upstream order. This is [way-of-working.md](../../../way-of-working.md) story 2.

```bash
export KIT=/path/to/git-uplink/examples/github
```

Work in the **internal** clone.

## Reset

**Actions → Reset example** on all three repos, then:

```bash
git uplink reset
```

## Two branches from the same baseline

Both patches apply to seed `main`. Do **not** record `Uplink-Depends-On`.

```bash
git checkout -b feat/sha256
git apply "$KIT/patches/asha-sha256.diff"
git add -A && git commit -m "Use SHA-256 for tokens"
git push -u origin feat/sha256

git checkout main
git checkout -b feat/ttl
git apply "$KIT/patches/ben-ttl.diff"
git add -A && git commit -m "Extend TTL"
git push -u origin feat/ttl
```

Open two PRs in the GitHub UI (Compare & pull request). Titles `Use SHA-256 for tokens` and `Extend TTL`. Bodies: [`asha-sha256.pr.md`](../patches/asha-sha256.pr.md) and [`ben-ttl.pr.md`](../patches/ben-ttl.pr.md).

Wait for assess + preflight on both. Merge Asha first, then Ben (one after the other; import is serialized by `uplink-mutate`).

```bash
git uplink reset
git uplink status
```

Both patches are `queued`. `src/tokens.js` has `sha256` and `ttl() == 7200`. Note both ids.

## Submit independently, merge Ben first

**Actions → Uplink submit** for each id (either order). Approve `to-upstream` each time.

Each public PR is the patch on public `main`, not stacked on the other.

On **upstream**, squash-merge **Ben’s** PR first. Then **Actions → Uplink sync** on internal. Ben’s trailer is the only new commit, so inspect applies immediately (no `from-upstream` wait).

```bash
git uplink reset
git uplink status
```

Ben is `merged`. Asha is still `submitted` (or `queued` if you had not submitted her) and is still applied on company `main`. Public `main` has TTL 7200; Asha’s SHA-256 is still the internal patch.

Merge Asha’s upstream PR and sync again to drop her too.
