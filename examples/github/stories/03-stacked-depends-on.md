# Story 3 — Ben builds on Asha (depends-on)

Asha is queued but not submitted. Ben’s log line needs `sha256`. This is [way-of-working.md](../../../way-of-working.md) story 3.

```bash
export KIT=/path/to/git-uplink/examples/github
```

Work in the **internal** clone.

## Reset

**Actions → Reset example** on all three repos, then:

```bash
git uplink reset
```

## Import Asha

Same as [story 1](01-solo-fix.md) through import (do not submit yet):

```bash
git checkout -b feat/sha256
git apply "$KIT/patches/asha-sha256.diff"
git add -A && git commit -m "Use SHA-256 for tokens"
git push -u origin feat/sha256
```

Open the PR in the GitHub UI; paste [`asha-sha256.pr.md`](../patches/asha-sha256.pr.md). Wait for checks. Merge.

```bash
git uplink reset
git uplink status
```

Copy Asha’s `upl_…` id (the SHA-256 row, not the workflows patch).

## Preflight without depends-on (should fail)

```bash
git uplink reset
git checkout -b feat/ben-log-nodep
git apply "$KIT/patches/ben-log.diff"
git add -A && git commit -m "Log token hashes"
git push -u origin feat/ben-log-nodep
```

Open a PR. Body: [`ben-log.pr.md`](../patches/ben-log.pr.md) **without** the `Uplink-Depends-On` line.

**Uplink upstream preflight** should fail and comment suggested `Uplink-Depends-On` lines. Do not merge. Close the PR in the GitHub UI.

## Import Ben with the trailer

```bash
git uplink reset
git checkout -b feat/ben-log
git apply "$KIT/patches/ben-log.diff"
git add -A && git commit -m "Log token hashes"
git push -u origin feat/ben-log
```

Open a PR. Body: [`ben-log.pr.md`](../patches/ben-log.pr.md) with `REPLACE_WITH_ASHA_ID` changed to Asha’s id. Wait for checks. Merge.

```bash
git uplink reset
git uplink status
```

Queue: Asha then Ben; Ben lists `dependsOn`. `src/tokens.js` logs then returns `sha256`.

## Submit order

**Uplink submit** for Ben **first**. The job should fail: `Submit upl_asha before upl_ben`.

Then submit Asha (approve `to-upstream`), then Ben (approve `to-upstream`). Ben’s public PR is stacked on Asha’s contrib branch until Asha merges, or you wait until Asha is `merged` and submit Ben onto public `main`.
