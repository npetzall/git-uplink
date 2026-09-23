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
git uplink reset
git fetch origin --prune
git uplink status
```

## Import Asha and Ben (independent)

Same as [story 2](02-parallel-independent.md) through import. Both patches apply to seed `main`.

```bash
git switch -C feat/sha256 main
git apply "$KIT/patches/asha-sha256.diff"
git add -A && git commit -m "Use SHA-256 for tokens"
git push -u origin feat/sha256

git switch -C feat/ttl main
git apply "$KIT/patches/ben-ttl.diff"
git add -A && git commit -m "Extend TTL"
git push -u origin feat/ttl
```

Open both PRs in the GitHub UI.

Asha — title:

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

Ben — title:

```text
Extend TTL
```

Body:

```text
Extend the default token TTL from one hour to two.

----- Uplink: internal below this line -----

Ticket: PROJ-2001
Uplink-Export-Author: Ben <ben@users.noreply.github.com>
```

Wait for checks. Merge both.

```bash
git uplink reset
git uplink status
```

Copy `ASHA_ID` and `BEN_ID`. Company `main` has `sha256` and `ttl() == 7200`.

## Import Cam with both trailers

```bash
git uplink reset
git switch -C feat/cam main
git apply "$KIT/patches/cam-wire.diff"
git add -A && git commit -m "Wire hash into a describe helper"
git push -u origin feat/cam
```

Open the PR. Title:

```text
Wire hash into a describe helper
```

Body (fill in both `REPLACE_WITH_*` ids):

```text
Expose a describeToken helper that reports TTL and the current hash so callers can wire both APIs together.

----- Uplink: internal below this line -----

Ticket: PROJ-3001
Uplink-Export-Author: Cam <cam@users.noreply.github.com>
Uplink-Depends-On: REPLACE_WITH_ASHA_ID
Uplink-Depends-On: REPLACE_WITH_BEN_ID
```

Wait for checks. Merge.

```bash
git uplink reset
git uplink status
```

Copy Cam’s `upl_…` id. Submit of Cam is refused until each upstream-bound dependency is `submitted` or `merged`.

## Export: merge the siblings first

**Uplink submit** for Asha and for Ben; approve `to-upstream` each time. Squash-merge **both** upstream PRs in the GitHub UI. **Uplink sync** on internal.

After sync, Asha and Ben are `merged`. Cam is the leftover delta on public `main`.

```bash
git uplink reset
git uplink status
```

**Uplink submit** for Cam; approve `to-upstream`. Cam’s public PR is `describeToken` only — not a replay of Asha or Ben.
