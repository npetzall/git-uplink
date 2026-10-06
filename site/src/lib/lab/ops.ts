import type { LabMode, LabOperation } from "../simulator";

const MANUAL: LabMode[] = ["manual"];
const CI: LabMode[] = ["ci"];

export function you(commands: string[], note?: string): LabOperation {
  return { modes: MANUAL, actor: "You", commands, note };
}

export function ciJob(workflow: string, commands: string[], note?: string): LabOperation {
  return { modes: CI, actor: workflow, workflow, commands, note };
}

export function startExample(): LabOperation[] {
  return [
    you(
      ["git uplink reset", "git uplink status"],
      "After Actions → Reset example on all three repos.",
    ),
  ];
}

export function startLifecycle(): LabOperation[] {
  return [
    you(
      ["git uplink init --upstream <url> --contrib <url> --forge github"],
      "Greenfield: company main matches public upstream. Init installs the tooling pack.",
    ),
  ];
}

export function branchPush(branch: string, patchFile: string, message: string): LabOperation {
  return you([
    `git checkout -b ${branch}`,
    `git apply "$KIT/patches/${patchFile}"`,
    "git add -A",
    `git commit -m "${message}"`,
    `git push -u origin ${branch}`,
  ]);
}

export function resetStatus(): LabOperation {
  return you(["git uplink reset", "git uplink status"]);
}

function addCommand(opts: { title: string; internalOnly?: boolean; dependsOn?: string[] }): string {
  const flags = [
    opts.internalOnly ? "--internal-only" : "",
    ...(opts.dependsOn ?? []).map((id) => `--depends-on ${id}`),
  ]
    .filter(Boolean)
    .join(" ");
  return `git uplink add --title "${opts.title}" --message-file /tmp/uplink-msg.txt --from <base.sha> --head <head.sha> --pr <n>${flags ? ` ${flags}` : ""}`;
}

export function assessCi(title: string, extra = ""): LabOperation {
  const flag = extra ? ` ${extra}` : "";
  return ciJob(
    "Uplink upstream assess",
    [
      "git uplink init",
      `git uplink assess --title "${title}" --message-file /tmp/uplink-msg.txt --from <base.sha> --head <head.sha>${flag}`,
    ],
    "Runs when the internal PR is opened or updated, with the optional assessment hook (pr). One PR comment, updated in place. Skipped for uplink:internal-only.",
  );
}

export function preflightCi(title: string, extra = ""): LabOperation {
  const flag = extra ? ` ${extra}` : "";
  return ciJob(
    "Uplink upstream preflight",
    [
      "git uplink init",
      `git uplink preflight --json --title "${title}" --message-file /tmp/uplink-msg.txt --from <base.sha> --head <head.sha>${flag}`,
    ],
    "Same PR. Skipped for uplink:internal-only. preflight.sh runs in a step with no token; a separate job posts the comment.",
  );
}

export function importOps(opts: {
  title: string;
  internalOnly?: boolean;
  dependsOn?: string[];
}): LabOperation[] {
  const add = addCommand(opts);
  const publish = opts.internalOnly ? "git uplink push" : "git uplink rebuild --push";
  return [
    you([add, publish], "After engineering review. Merge is the internal product gate."),
    ...(opts.internalOnly
      ? []
      : [assessCi(opts.title), preflightCi(opts.title)]),
    ...(opts.internalOnly
      ? []
      : [
          ciJob(
            "Uplink import",
            [
              "git uplink init",
              `git uplink preflight --json --title "${opts.title}" --message-file /tmp/uplink-msg.txt --from <base.sha> --head <head.sha>`,
            ],
            "Preflight job, after the merge: read-only token, no App token. It runs preflight.sh and hands its result to the import job.",
          ),
        ]),
    ciJob(
      "Uplink import",
      [
        "git uplink init",
        opts.internalOnly ? add : `${add} --preflight-result <result>`,
        publish,
      ],
      opts.internalOnly
        ? "Runs after the internal PR is merged."
        : "Import job, with the write token. It takes the preflight job's result and never runs preflight.sh.",
    ),
  ];
}

export function submitOps(id: string): LabOperation[] {
  return [
    you([
      `git uplink report ${id}`,
      `git uplink approve ${id}`,
      `git uplink submit ${id} --push`,
      `git uplink submitted ${id} --pr-url <url>`,
    ], "Locally, --push force-pushes the export commit unsigned. The workflow lets GitHub create a signed one."),
    ciJob(
      "Uplink submit",
      [`gh workflow run uplink-assessment-hook.yml --ref uplink/hooks -f patch=${id} -f state=<uplink/state commit>`],
      "Dispatch Uplink submit from company main. Extras job, without the queue lock: runs the optional hook with the patch id and the uplink/state commit to read it from, unless an earlier submit run stored its result and the patch is unchanged. A failed hook is noted in the packet, not fatal, and not stored.",
    ),
    ciJob(
      "Uplink submit",
      ["git uplink init", `git uplink report ${id} --extra-dir <artifact> --extra-state <uplink/state commit> --store-extras`],
      "Packet job, under uplink-mutate: assesses the patch file, then writes assessment.md once, extras first. Stops on findings, or when the patch is no longer what the hook read. No environment secrets yet.",
    ),
    ciJob(
      "Uplink submit",
      ["git uplink init", `git uplink preflight ${id} --json`],
      "Preflight job: read-only token, no Environment. It runs preflight.sh on the export tree and hands its result to the submit job.",
    ),
    ciJob(
      "Uplink submit",
      [
        "git uplink init",
        `git uplink approve ${id}`,
        `git uplink submit ${id} --preflight-result <result>`,
        `gh api --method POST /repos/<upstream>/pulls -f head=<contrib_org>:uplink/${id} -f base=main -f title=<title> -F maintainer_can_modify=false`,
        `git uplink submitted ${id} --pr-url <url>`,
      ],
      "Submit job waits on Environment to-upstream. Approve the deployment, then the same run exports with the preflight job's result (preflight.sh never runs next to the fork-write credential): submit builds the commit on uplink/upstream, and contrib_commit.py recreates it on the fork through the Git Database API, so GitHub signs it (Verified with a GitHub App token).",
    ),
  ];
}

export function syncFlowBack(): LabOperation[] {
  return [
    you(["git uplink sync", "git uplink reset", "git uplink status"]),
    ciJob(
      "Uplink sync",
      ["git uplink init", "git uplink sync"],
      "Inspect applies immediately when merged company patches explain every new public change: a commit with the patch's patch-id, or its merged public PR. The trailer alone is not enough. No from-upstream wait.",
    ),
  ];
}

export function syncFromUpstream(conflictId?: string): LabOperation[] {
  return [
    you(["git uplink sync", "git uplink accept-upstream", "git uplink reset", "git uplink status"]),
    ciJob(
      "Uplink sync",
      ["git uplink init", "git uplink sync"],
      "Inspect writes .uplink/reports/from-upstream/incoming.md. Apply job waits on Environment from-upstream.",
    ),
    ciJob(
      "Uplink sync",
      [
        "git uplink init",
        "git uplink accept-upstream",
        ...(conflictId ? [`git uplink gated ${conflictId} --pr-url <url>`] : []),
      ],
      "After from-upstream approval. Patch apply conflicts are recorded after promotion.",
    ),
  ];
}

export function fixConflict(id: string): LabOperation[] {
  return [
    you(
      [
        "git fetch origin",
        `git checkout uplink/conflict/${id}-work`,
        "# fix conflict markers",
        "git add -A",
        `git commit -m "Resolve ${id} onto the new upstream"`,
        `git push origin uplink/conflict/${id}-work`,
      ],
      "Work on -work. Merge the gated PR into the protected base. Status stays conflict until resolve.",
    ),
  ];
}

export function resolveOps(id: string): LabOperation[] {
  return [
    you([`git uplink resolve ${id}`], "If the resolve workflow is not installed."),
    ciJob(
      "Uplink resolve",
      ["git uplink init", `git uplink resolve ${id}`],
      `Runs when the gated PR is merged into uplink/conflict/${id}.`,
    ),
  ];
}

export function mergeUpstream(pr: string): LabOperation {
  return you([`gh pr merge ${pr} --squash`], "On the public upstream pull request.");
}
