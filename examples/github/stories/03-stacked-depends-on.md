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
git fetch origin --prune
git uplink status
```

## Import Asha

Same as [story 1](01-solo-fix.md) through import (do not submit yet):

```bash
git switch -C feat/sha256 main
git apply "$KIT/patches/asha-sha256.diff"
git add -A && git commit -m "Use SHA-256 for tokens"
git push -u origin feat/sha256
```

Open the PR in the GitHub UI. Title:

```text
Use SHA-256 for tokens
```

Body:

```text
Replace SHA-1 in the default hasher with SHA-256.

----- Uplink: internal below this line -----

Ticket: PROJ-1234
Uplink-Export-Author: Asha <asha@users.noreply.github.com>
```

Wait for checks. Merge.

```bash
git uplink reset
git uplink status
```

Copy Asha’s `upl_…` id (the SHA-256 row, not the workflows patch).

## Preflight without depends-on (should fail)

```bash
git uplink reset
git switch -C feat/ben-log-nodep main
git apply "$KIT/patches/ben-log.diff"
git add -A && git commit -m "Log token hashes"
git push -u origin feat/ben-log-nodep
```

Open a PR. Title:

```text
Log token hashes
```

Body (**without** the `Uplink-Depends-On` line):

```text
Log the hasher path before returning a digest so operators can trace token hashing.

----- Uplink: internal below this line -----

Ticket: PROJ-2002
Uplink-Export-Author: Ben <ben@users.noreply.github.com>
```

**Uplink upstream preflight** should fail and comment suggested `Uplink-Depends-On` lines. Do not merge. Close the PR in the GitHub UI.

## Import Ben with the trailer

```bash
git uplink reset
git switch -C feat/ben-log main
git apply "$KIT/patches/ben-log.diff"
git add -A && git commit -m "Log token hashes"
git push -u origin feat/ben-log
```

Open a PR. Title:

```text
Log token hashes
```

Body (change `REPLACE_WITH_ASHA_ID` to Asha’s id):

```text
Log the hasher path before returning a digest so operators can trace token hashing.

----- Uplink: internal below this line -----

Ticket: PROJ-2002
Uplink-Export-Author: Ben <ben@users.noreply.github.com>
Uplink-Depends-On: REPLACE_WITH_ASHA_ID
```

Wait for checks. Merge.

```bash
git uplink reset
git uplink status
```

Queue: Asha then Ben; Ben lists `dependsOn`. `src/tokens.js` logs then returns `sha256`.

## Submit order

**Uplink submit** for Ben **first**. The job should fail: `Submit upl_asha before upl_ben`.

Then submit Asha (approve `to-upstream`), then Ben (approve `to-upstream`). Ben’s public PR is stacked on Asha’s contrib branch until Asha merges, or you wait until Asha is `merged` and submit Ben onto public `main`.
