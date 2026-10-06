import { rebuild, type LabScenario, type SimPatch } from "../../simulator";
import { TOKENS_SHA256, exampleSeed, tree } from "../fixtures";
import {
  ciJob,
  importOps,
  resetStatus,
  startExample,
  submitOps,
  you,
} from "../ops";

export const assessmentHook: LabScenario = {
  id: "assessment-hook",
  title: "07 — Assessment hook",
  blurb:
    "Turn on the starter hook on uplink/hooks, which git uplink init created. Submit Asha; extras prepend onto the packet. The hook never enters the queue or leaves the private forge.",
  highlight: "Uplink assessment hook",
  initial: exampleSeed,
  startOperations: startExample(),
  steps: [
    {
      id: "set-up-hooks",
      title: "Add the hook on uplink/hooks",
      summary:
        "git uplink init created the orphan branch uplink/hooks with assessment-hook.md and a starter hook, and you pushed it at setup. Copy .github/workflows/uplink-assessment-hook-example.yml to .github/workflows/uplink-assessment-hook.yml there. Pushing a workflow file needs workflows write.",
      why: "Hooks are company-only, so they stay off main and out of the queue. GitHub only dispatches workflows whose file is on the default branch; the placeholder lets submit run the uplink/hooks version.",
      operations: [
        ciJob(
          "Uplink assessment hook",
          ["gh workflow run uplink-assessment-hook.yml --ref main"],
          "Placeholder on main. Prints assessment-hook.md from uplink/hooks; writes nothing.",
        ),
        you(
          [
            "git switch uplink/hooks",
            "git pull --ff-only origin uplink/hooks",
            "cp .github/workflows/uplink-assessment-hook-example.yml .github/workflows/uplink-assessment-hook.yml",
            "git add .github/workflows/uplink-assessment-hook.yml",
            'git commit -m "Add Uplink assessment hook"',
            "git push origin uplink/hooks",
          ],
          "With uplink-hooks-ruleset.json imported, open a pull request against uplink/hooks instead.",
        ),
      ],
      apply: (state) => ({
        ...state,
        stepId: "set-up-hooks",
        log: [
          ...state.log,
          "Added the hook workflow on uplink/hooks. The queue is unchanged.",
        ],
      }),
    },
    {
      id: "import-asha",
      title: "Import Asha’s SHA-256 change",
      summary:
        "Asha branches from company main, switches SHA-1 to SHA-256, and opens one internal PR. Merge plus import records upl_asha as queued.",
      why: "The assessment hook runs on this PR with pr and shares one PR comment with assess, updated on every push or edit. That is advice for the author and reviewers: the change is not a patch yet, so import stores nothing from it.",
      operations: [
        you(
          [
            "git checkout -b feat/sha256",
            'git apply "$KIT/patches/asha-sha256.diff"',
            "git add -A",
            'git commit -m "Use SHA-256 for tokens"',
            "git push -u origin feat/sha256",
          ],
        ),
        ...importOps({ title: "Use SHA-256 for tokens" }),
        resetStatus(),
      ],
      apply: (state) => {
        const patch: SimPatch = {
          id: "upl_asha",
          title: "Use SHA-256 for tokens",
          queue: "upstream",
          status: "queued",
          dependsOn: [],
          files: tree(TOKENS_SHA256),
        };
        return rebuild({
          ...state,
          stepId: "import-asha",
          patches: [...state.patches, patch],
          log: [
            ...state.log,
            "Imported upl_asha as queued. src/tokens.js on main calls sha256. Only the hook placeholder is on main.",
          ],
        });
      },
    },
    {
      id: "submit-asha",
      title: "Submit Asha; extras lead the packet",
      summary:
        "Dispatch Uplink submit. The assess job assesses the patch file and runs the hook on that assessment package, and the packet job stores both results: ## Company review notes sits above # Contribution packet. Then approve to-upstream. The contribution fork has SHA-256 only; no hook file is exported.",
      why: "Submit is the only writer of uplink/state. The assessment hook returns an artifact; it must not push state.",
      operations: submitOps("upl_asha"),
      apply: (state) => {
        const patches = state.patches.map((patch) =>
          patch.id === "upl_asha" ? { ...patch, status: "submitted" as const, prNumber: 412 } : patch,
        );
        const asha = patches.find((patch) => patch.id === "upl_asha")!;
        return {
          ...state,
          stepId: "submit-asha",
          patches,
          contrib: [{ branch: "uplink/upl_asha", files: asha.files, prNumber: 412 }],
          log: [
            ...state.log,
            "Assessed upl_asha; the hook read the assessment package; ## Company review notes sits above # Contribution packet.",
            "Approved to-upstream for upl_asha. Opened public PR #412 from uplink/upl_asha.",
          ],
        };
      },
    },
    {
      id: "remove-hook",
      title: "Remove the hook",
      summary:
        "Reset example does not touch uplink/hooks. Revert every commit on uplink/hooks since init created it, so the hook workflow disappears and the stubs are back.",
      why: "Later stories assume no assessment hook. Reverting keeps the branch history; uplink/hooks rejects force-pushes under its ruleset.",
      operations: [
        you(
          [
            "git switch uplink/hooks",
            "git pull --ff-only origin uplink/hooks",
            'git revert --no-edit "$(git rev-list --max-parents=0 HEAD)..HEAD"',
            "git push origin uplink/hooks",
            "git switch main",
          ],
          "With uplink-hooks-ruleset.json imported, push the reverts to a topic branch and open a pull request against uplink/hooks instead.",
        ),
      ],
      apply: (state) => ({
        ...state,
        stepId: "remove-hook",
        log: [
          ...state.log,
          "Reverted the hook commits on uplink/hooks. PR checks and submit skip the assessment hook again. The queue is unchanged.",
        ],
      }),
    },
  ],
};
