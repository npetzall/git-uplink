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
      ["git uplink init --upstream <url> --contrib <url> --forge ghec"],
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
    "Uplink assess for upstream",
    [
      "git uplink init",
      `git uplink assess --title "${title}" --message-file /tmp/uplink-msg.txt --from <base.sha> --head <head.sha>${flag}`,
    ],
    "Runs when the internal PR is opened or updated.",
  );
}

export function preflightCi(title: string, extra = ""): LabOperation {
  const flag = extra ? ` ${extra}` : "";
  return ciJob(
    "Uplink export preflight",
    [
      "git uplink init",
      `git uplink preflight --title "${title}" --message-file /tmp/uplink-msg.txt --from <base.sha> --head <head.sha>${flag}`,
    ],
    "Same PR. Skipped for uplink:internal-only.",
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
    assessCi(opts.title, opts.internalOnly ? "--internal-only" : ""),
    ...(opts.internalOnly ? [] : [preflightCi(opts.title)]),
    ciJob("Uplink import", ["git uplink init", add, publish], "Runs after the internal PR is merged."),
  ];
}

export function submitOps(id: string): LabOperation[] {
  return [
    you([
      `git uplink report ${id}`,
      `git uplink approve ${id}`,
      `git uplink submit ${id}`,
      `git uplink submitted ${id} --pr-url <url>`,
    ]),
    ciJob(
      "Uplink submit",
      ["git uplink init", `git uplink report ${id}`],
      "Dispatch Uplink submit from company main. Packet job; no environment secrets yet.",
    ),
    ciJob(
      "Uplink submit",
      ["git uplink init", `git uplink report ${id} --extra-dir <artifact>`],
      "Finalize: optional uplink-assessment-hook.yml; extras prepended. Skipped if the file is absent.",
    ),
    ciJob(
      "Uplink submit",
      [
        "git uplink init",
        `git uplink approve ${id}`,
        `git uplink submit ${id}`,
        `gh pr create -R <upstream> --head <contrib:uplink/${id}> --base main --title <title> --body-file <body>`,
        `git uplink submitted ${id} --pr-url <url>`,
      ],
      "Submit job waits on Environment to-upstream. Approve the deployment, then the same run exports.",
    ),
  ];
}

export function syncFlowBack(): LabOperation[] {
  return [
    you(["git uplink sync", "git uplink reset", "git uplink status"]),
    ciJob(
      "Uplink sync",
      ["git uplink init", "git uplink sync"],
      "Inspect applies immediately when every new public commit matches a company patch (trailer / patch-id). No from-upstream wait.",
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
