# Story 7 — Assessment hook extras on the IP packet

Company scans that belong in the contribution packet run **after** `git uplink report` and **before** Environment **to-upstream**. The forge pack does not ship this file. You add `.github/workflows/uplink-assessment-hook.yml` as an internal-only product patch. Submit’s finalize job dispatches it and prepends artifact `uplink-packet-extra` onto `assessment.md`.

This is the walkthrough for [templates/README.md](../../../templates/README.md) **Assessment hook**. The copy-paste YAML is [`uplink-assessment-hook.yml`](../patches/uplink-assessment-hook.yml).

```bash
export KIT=/path/to/git-uplink/examples/github
```

Work in the **internal** clone.

## Reset

**Actions → Reset example** on all three repos, then:

```bash
git uplink reset
```

Queue: only the internal-only tooling patch. Stories 01–06 stay pack-only; this file is not on seed.

## Import the assessment hook (internal-only)

Merging a workflow file needs **workflows** write (example org owner).

```bash
git checkout -b feat/assessment-hook
mkdir -p .github/workflows
cp "$KIT/patches/uplink-assessment-hook.yml" .github/workflows/uplink-assessment-hook.yml
git add .github/workflows/uplink-assessment-hook.yml
git commit -m "Add Uplink assessment hook"
git push -u origin feat/assessment-hook
```

Open a PR. On the **Open pull request** page, add label `uplink:internal-only` **before** you click Create. Title `Add Uplink assessment hook`. Body: [`uplink-assessment-hook.pr.md`](../patches/uplink-assessment-hook.pr.md). Wait for checks. Merge.

```bash
git uplink reset
git uplink status
```

The hook patch is `queued` on the **internal** queue. Company `main` has `.github/workflows/uplink-assessment-hook.yml`. `--upgrade` will not overwrite it (it is not in the forge pack).

## Land Asha (upstream-bound)

```bash
git checkout -b feat/sha256
git apply "$KIT/patches/asha-sha256.diff"
git add -A
git commit -m "Use SHA-256 for tokens"
git push -u origin feat/sha256
```

Open a PR. Title: `Use SHA-256 for tokens`. Body: [`asha-sha256.pr.md`](../patches/asha-sha256.pr.md). Wait until **Uplink upstream assess** and **Uplink upstream preflight** are green. Merge.

```bash
git uplink reset
git uplink status
```

Copy Asha’s patch id (`upl_` + 10 hex digits).

## Submit: extras first, then to-upstream

**Actions → Uplink submit → Run workflow** on internal `main`, input `patch_id` = Asha’s id.

Watch **Uplink assessment hook** start from the finalize job. Open finalize’s `GITHUB_STEP_SUMMARY` (or `.uplink/reports/<id>/assessment.md` on `uplink/state`): `## Company review notes` sits **above** `# Contribution packet`.

Then **Review deployments** → approve `to-upstream`. Full upstream merge and flow-back are [story 01](01-solo-fix.md).
