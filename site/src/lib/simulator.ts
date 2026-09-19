export type QueueLayer = "upstream" | "internal";
export type Status =
  | "queued"
  | "approved"
  | "submitted"
  | "amended"
  | "merged"
  | "dropped"
  | "conflict";

export type SimFileMap = Record<string, string>;

export type SimPatch = {
  id: string;
  title: string;
  queue: QueueLayer;
  status: Status;
  dependsOn: string[];
  files: SimFileMap;
  prNumber?: number;
  mergedVia?: string;
};

export type SimConflict = {
  patchId: string;
  branch: string;
  blockedIds: string[];
  files: SimFileMap;
  phase: "stopped" | "fixed";
};

export type SimState = {
  stepId: string;
  upstream: SimFileMap;
  company: SimFileMap;
  contrib: { branch: string; files: SimFileMap; prNumber?: number }[];
  patches: SimPatch[];
  log: string[];
  conflict?: SimConflict;
};

export type LabMode = "manual" | "ci";

export type LabOperation = {
  modes: LabMode[];
  actor: string;
  workflow?: string;
  commands: string[];
  note?: string;
};

export type LabStep = {
  id: string;
  title: string;
  summary: string;
  why: string;
  apply: (state: SimState) => SimState;
  operations: LabOperation[];
};

export type LabScenario = {
  id: string;
  title: string;
  blurb: string;
  highlight?: string;
  initial: () => SimState;
  startOperations?: LabOperation[];
  steps: LabStep[];
};

function clone(files: SimFileMap): SimFileMap {
  return { ...files };
}

function applyPatches(upstream: SimFileMap, patches: SimPatch[]): SimFileMap {
  const next = clone(upstream);
  const active = (patch: SimPatch) => patch.status !== "merged" && patch.status !== "dropped";
  const ordered = [
    ...patches.filter((patch) => patch.queue === "upstream" && active(patch)),
    ...patches.filter((patch) => patch.queue === "internal" && active(patch)),
  ];
  for (const patch of ordered) {
    if (patch.status === "conflict") break;
    Object.assign(next, patch.files);
  }
  return next;
}

export function rebuild(state: SimState): SimState {
  return { ...state, company: applyPatches(state.upstream, state.patches) };
}

export function runThrough(scenario: LabScenario, count: number): SimState {
  let state = scenario.initial();
  for (const step of scenario.steps.slice(0, count)) {
    state = step.apply(state);
  }
  return state;
}

export function operationsFor(
  operations: LabOperation[] | undefined,
  mode: LabMode,
): LabOperation[] {
  return (operations ?? []).filter((op) => op.modes.includes(mode));
}
