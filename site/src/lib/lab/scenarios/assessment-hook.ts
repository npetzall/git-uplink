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
    "Create uplink/hooks from the pack placeholder and add the hook there. Submit Asha; extras prepend onto the packet. The hook never enters the queue or leaves the private forge.",
  highlight: "Uplink assessment hook",
  initial: exampleSeed,
  startOperations: startExample(),
  steps: [
    {
      id: "set-up-hooks",
      title: "Create uplink/hooks and add the hook",
      summary:
        "Run the pack's Uplink assessment hook placeholder on main. It creates the orphan branch uplink/hooks with assessment-hook.md. Copy the starter YAML from that guide to .github/workflows/uplink-assessment-hook.yml on uplink/hooks. Pushing a workflow file needs workflows write.",
      why: "Hooks are company-only, so they stay off main and out of the queue. GitHub only dispatches workflows whose file is on the default branch; the placeholder lets submit run the uplink/hooks version.",
      operations: [
        ciJob(
          "Uplink assessment hook",
          ["gh workflow run uplink-assessment-hook.yml --ref main"],
          "Placeholder on main. Creates the orphan branch uplink/hooks if missing; otherwise updates assessment-hook.md only when it changed.",
        ),
        you(
          [
            "git fetch origin uplink/hooks",
            "git switch uplink/hooks",
            "mkdir -p .github/workflows",
            "# copy the YAML block from assessment-hook.md",
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
          "Created orphan branch uplink/hooks with assessment-hook.md and the hook workflow. The queue is unchanged.",
        ],
      }),
    },
    {
      id: "import-asha",
      title: "Import Asha’s SHA-256 change",
      summary:
        "Asha branches from company main, switches SHA-1 to SHA-256, and opens one internal PR. Merge plus import records upl_asha as queued.",
      why: "The assessment hook runs on this PR with pr and shares one PR comment with assess, updated on every push or edit. Import stores the hook's extras with the patch because they match what was merged.",
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
            "Imported upl_asha as queued with the PR check's hook extras stored. src/tokens.js on main calls sha256. Only the hook placeholder is on main.",
          ],
        });
      },
    },
    {
      id: "submit-asha",
      title: "Submit Asha; extras lead the packet",
      summary:
        "Dispatch Uplink submit. The patch is unchanged since import, so the packet reuses the stored extras and the hook does not run again: ## Company review notes sits above # Contribution packet. Then approve to-upstream. The contribution fork has SHA-256 only; no hook file is exported.",
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
            "Packet reused the extras stored at import; ## Company review notes sits above # Contribution packet.",
            "Approved to-upstream for upl_asha. Opened public PR #412 from uplink/upl_asha.",
          ],
        };
      },
    },
  ],
};
