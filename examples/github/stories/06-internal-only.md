# Story 6 — Internal-only telemetry

A company-only change must never pass the IP gate. `companyTelemetry` is in `UPLINK_REDACT_KEYWORDS`. This is the internal-only escape hatch in [way-of-working.md](../../../way-of-working.md).

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

## Prepare fails without the label

```bash
git checkout -b feat/telemetry-public
git apply "$KIT/patches/internal-telemetry.diff"
git add -A && git commit -m "Vendor telemetry"
git push -u origin feat/telemetry-public
```

Open a PR in the GitHub UI. Title `Vendor telemetry`. Body: [`internal-telemetry.pr.md`](../patches/internal-telemetry.pr.md). Do **not** add `uplink:internal-only`.

**Uplink prepare for upstream** should fail: the export surface contains `companyTelemetry`. Close this PR.

## Import with uplink:internal-only

```bash
git checkout main && git reset --hard origin/main
git checkout -b feat/telemetry
git apply "$KIT/patches/internal-telemetry.diff"
git add -A && git commit -m "Vendor telemetry"
git push -u origin feat/telemetry
```

Open a PR. On the **Open pull request** page, add label `uplink:internal-only` **before** you click Create (prepare only sees labels that exist when the check runs). Wait for checks. Merge.

```bash
git fetch origin && git reset --hard origin/main
git fetch origin '+refs/heads/uplink/state:refs/heads/uplink/state'
git uplink status
```

The telemetry patch is `queued` with intent `internal-only`. Company `main` calls `companyTelemetry()`.

## Submit is refused

**Actions → Uplink submit** with that patch id. `git uplink approve` / `submit` refuse internal-only. Nothing is pushed to the contrib fork.

The workflows patch from bootstrap is the same class of change: product-only, never submitted.
