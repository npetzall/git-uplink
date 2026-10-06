# Uplink assessment hook

The assessment hook runs the company checks whose results belong in the contribution packet. IP reads its markdown before approving.

Hooks are company-only. They live on the orphan branch `uplink/hooks`, never on `main`, so they are never queued, replayed, or contributed.

## When it runs

1. **On the PR, as advice.** Uplink PR checks run the hook on every open, push, reopen and edit. The hook's markdown and the built-in assess report share one PR comment, which is updated in place instead of adding a new one each run. Nothing from this run is stored: the change is not a patch yet, and import does not copy the result.
2. **At submit, for the packet.** Uplink submit runs the hook on the patch. This is the result IP reads. A successful run is stored with the patch on `uplink/state` under `.uplink/reports/<id>/extras/` and reused by later submit runs while the patch content is unchanged. The hook runs again when nothing is stored, the stored result is out of date (for example after a conflict was resolved), or the patch was amended.

A failed hook never fails the PR check or submit. It shows as a warning, and the comment or `assessment.md` starts with a note that links to the failed run. IP decides. A failed result is never stored, so the next submit runs the hook again.

If the patch, its message or `uplink.toml` changes while the hook runs at submit, the package the hook read is for other content. Submit stops; dispatch it again.

## The assessment package

Before it runs the hook, the caller runs `git uplink assess` and uploads the result as artifact `uplink-assessment` of its own run. That is everything the hook is given, and it has the same shape for a pull request and for a patch:

| File | Content |
| --- | --- |
| `assessment.json` | What was assessed, the result, and the settings it was made with (below) |
| `assessment.md` | The assess report as markdown |
| `change.patch` | The change itself. For a pull request the diff of the PR. For a patch the patch file from the queue, which starts with the company commit message |

`assessment.json`:

| Field | Content |
| --- | --- |
| `kind` | `pr` for a pull request check, `patch` for Uplink submit |
| `patch`, `state` | For `patch`: the patch id (`upl_…`) and the `uplink/state` commit it was read at |
| `title` | The public pull request title |
| `message` | `stored` (the whole message), `subject` and `body` (what is public), `coAuthor` |
| `ok`, `checks` | The result. Each check has `id`, `status` (`pass`, `warn`, `fail`, `skip`) and `detail` |
| `settings` | `redactKeywords` and `internalEmailDomains` from `uplink.toml`, or `problem` when it could not be read |
| `changeBlob` | Git blob id of `change.patch` |
| `queueEntry` | For `patch`: the patch as it is in `queue.json` (status, dependencies, approvals, public PR) |

## How it is wired

- `git uplink init` creates `uplink/hooks` locally with this file, `toolchain-hook.md`, the toolchain hook stub, `preflight.sh`, a starter assessment hook, and `uplink.toml` (the CLI's settings). `git uplink push` publishes it with `uplink/state`. `git uplink init --upgrade` adds files a newer pack brings, without changing the ones you have. `git uplink doctor` reports when it is missing or not pushed.
- The forge pack installs a placeholder `.github/workflows/uplink-assessment-hook.yml` on `main`. GitHub only dispatches a workflow whose file exists on the default branch, so the placeholder has to be there. Running it by hand from the Actions tab prints this file from `uplink/hooks`.
- Your real hook is `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks`. Callers look for it there and run it with `--ref uplink/hooks`. If it is missing, they skip the hook.

## Contract

- Trigger on `workflow_dispatch` with one input, `caller_run_id`: the run that dispatched the hook, as `<run id>-<attempt>`. Put it in `run-name`; callers use it to find the run they dispatched. Inputs of an older contract (`pr`, `patch`, `state`) may stay declared; they are no longer sent.
- Download the package from the caller run. The job needs `actions: read`:

  ```bash
  gh run download "${CALLER_RUN_ID%%-*}" -n uplink-assessment -D package
  ```
- The hook is not told whether it runs for a pull request or a patch. `kind` in `assessment.json` says so when it matters.
- What the hook does with the package is yours to decide: look up a ticket, scan `change.patch`, check the settings against another source. Uplink passes nothing else.
- Upload `*.md` files as artifact `uplink-packet-extra`. Files are prepended in name order. An empty result is fine. uplink adds no headings or separators. Each file's markdown goes in as-is, so include your own heading and use a prefix such as `10-`, `20-` to set the order.
- Do not push `uplink/state`. Uplink stores the result itself.

**A package of kind `pr` describes a change that is not merged or reviewed yet, and was made by the pull request's own copy of the workflow.** Read every file in it as data: do not apply and build `change.patch` in a job that has secrets, and do not trust `ok` or `settings` from it for anything but advice.

## Add the hook

Copy `.github/workflows/uplink-assessment-hook-example.yml` to `.github/workflows/uplink-assessment-hook.yml` on `uplink/hooks`. The `-example` name keeps the starter from running. Change its "Write company extras" step to run your checks. A push that adds or changes a workflow file needs `workflows` write, so push with your own account or open a pull request against `uplink/hooks`.

## Protect the branch

Code on `uplink/hooks` runs on every PR check and submit. Import `.github/uplink-hooks-ruleset.json` from `main` as a repository ruleset. Changes to `uplink/hooks` then need a reviewed pull request.
