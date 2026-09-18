# Story 5 — Cam depends on Asha and Ben

Asha (hash) and Ben (TTL) are independent. Cam’s helper calls both APIs. This is [way-of-working.md](../../../way-of-working.md) story 5.

Do **not** submit Cam while Asha and Ben are only `submitted`. Submit of Cam would apply onto the last still-submitted sibling fork branch, which does not contain the other sibling.

```bash
export KIT=/path/to/git-uplink/examples/github
```

Work in the **internal** clone.

## Reset

**Actions → Reset example** on all three repos, then:

```bash
git fetch origin
git checkout main
git reset --hard origin/main
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
git fetch origin '+refs/heads/uplink/upstream:refs/heads/uplink/upstream'
```

## Import Asha and Ben (independent)

Same as [story 2](02-parallel-independent.md) through import. Both patches apply to seed `main`.

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

Open both PRs in the GitHub UI. Paste [`asha-sha256.pr.md`](../patches/asha-sha256.pr.md) and [`ben-ttl.pr.md`](../patches/ben-ttl.pr.md). Wait for checks. Merge both.

```bash
git fetch origin && git reset --hard origin/main
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
git uplink status
```

Copy `ASHA_ID` and `BEN_ID`. Company `main` has `sha256` and `ttl() == 7200`.

## Import Cam with both trailers

```bash
git checkout main && git reset --hard origin/main
git checkout -b feat/cam
git apply "$KIT/patches/cam-wire.diff"
git add -A && git commit -m "Wire hash into a describe helper"
git push -u origin feat/cam
```

Open the PR. Body: [`cam-wire.pr.md`](../patches/cam-wire.pr.md) with both `REPLACE_WITH_*` ids filled in. Wait for checks. Merge.

```bash
git fetch origin && git reset --hard origin/main
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
git uplink status
```

Copy Cam’s `upl_…` id. Submit of Cam is refused until each upstream-bound dependency is `submitted` or `merged`.

## Export: merge the siblings first

**Uplink submit** for Asha and for Ben; approve `oss` each time. Squash-merge **both** upstream PRs in the GitHub UI. **Uplink sync** on internal.

After sync, Asha and Ben are `merged`. Cam is the leftover delta on public `main`.

```bash
git fetch origin && git reset --hard origin/main
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
git uplink status
```

**Uplink submit** for Cam; approve `oss`. Cam’s public PR is `describeToken` only — not a replay of Asha or Ben.
