import type { FileDelta } from "./patch";

/** `local` is the uplink/state branch in this repository; `remote` is origin/uplink/state. Both are read with git. */
export type QueueSource = "local" | "remote";

export type PatchSource = {
  /** Internal authors, `Name <email>`. Co-authors on company main; never exported. */
  authors?: string[];
  note?: string;
  internalPrNumber?: number;
  internalPrUrl?: string;
};

export type PatchUpstream = {
  contribBranch: string;
  prNumber?: number;
  prUrl?: string;
  submittedAt?: string;
};

export type PatchApproval = {
  at: string;
  version: number;
  kind: string;
  sha: string;
  patchIdStable?: string;
  runUrl?: string;
  reviewed?: string;
};

export type PatchEvent = {
  at: string;
  type: string;
  detail: string;
};

export type PatchConflict = {
  branch: string;
  workBranch?: string;
  files: string[];
  message: string;
  onto?: string;
  prNumber?: number;
  prUrl?: string;
};

export type PatchMerged = {
  via: string;
  at: string;
  upstreamSha?: string;
};

export type Patch = {
  id: string;
  title: string;
  commitMessage?: string;
  status: string;
  dependsOn?: string[];
  createdAt?: string;
  updatedAt?: string;
  patchIdStable?: string | null;
  source?: PatchSource;
  upstream?: PatchUpstream;
  merged?: PatchMerged;
  conflict?: PatchConflict;
  approvals?: PatchApproval[];
  events?: PatchEvent[];
  kind?: string;
};

export type StateStatus = {
  branch: string;
  local?: string;
  remote?: string;
  remoteRef?: string;
  ahead?: number;
  behind?: number;
};

/** Queued, waiting, submitted, merged and rejected count the upstream layer; internal counts queued internal patches; active counts queued patches in every layer. */
export type StatusCounts = {
  queued: number;
  waiting: number;
  submitted: number;
  merged: number;
  rejected: number;
  internal: number;
  active: number;
};

export type StatusResponse = {
  present: boolean;
  cwd: string;
  source: string;
  error?: string;
  companyHead?: string;
  upstreamHead?: string;
  counts?: StatusCounts;
  tooling?: Patch | null;
  upstream?: Patch[];
  internal?: Patch[];
  lastSync?: {
    at: string;
    upstreamSha: string;
    result: string;
    message?: string;
  };
  state?: StateStatus;
  /** Approved or submitted patches that changed since their last approval. */
  staleApprovals?: string[];
  /** Queued upstream patches submit can take now, in queue order. */
  readyToSubmit?: string[];
};

export type StreamNode = {
  id: string;
  title: string;
  status: string;
  dependsOn: string[];
  /** Dependencies that are not active upstream patches. */
  missing: string[];
  /** The newest approval does not cover the patch as it is now. */
  stale: boolean;
};

export type SubmitStreamResponse = {
  present: boolean;
  source: string;
  error?: string;
  /** Column 0 has no open dependencies; a patch in column N goes once its dependencies are merged. */
  layers: StreamNode[][];
};

export type FileRevision = {
  sha: string;
  at: string;
  subject: string;
};

export type PatchResponse = {
  present: boolean;
  source: string;
  error?: string;
  layer?: string;
  patch?: Patch;
  revisions: FileRevision[];
  patchFile?: string;
  /** The newest approval does not cover the patch as it is now. */
  approvalStale?: boolean;
};

export type FileResponse = {
  path: string;
  sha?: string;
  content: string;
};

export function parseSource(value: string | null): QueueSource {
  return value === "remote" ? "remote" : "local";
}

export async function loadSubmitStream(
  source: QueueSource,
): Promise<SubmitStreamResponse> {
  const params = new URLSearchParams({ source });
  const response = await fetch(`/api/submit-stream?${params}`);
  return response.json();
}

export async function loadStatus(
  source: QueueSource,
  fetchRemote: boolean,
): Promise<StatusResponse> {
  const params = new URLSearchParams({
    source,
    fetch: fetchRemote ? "1" : "0",
  });
  const response = await fetch(`/api/status?${params}`);
  return response.json();
}

export async function refreshRemote(): Promise<void> {
  const response = await fetch("/api/refresh", { method: "POST" });
  if (!response.ok) {
    const body = (await response.json().catch(() => ({}))) as {
      error?: string;
    };
    throw new Error(body.error || `refresh failed (${response.status})`);
  }
}

export async function loadPatch(
  id: string,
  source: QueueSource,
): Promise<PatchResponse> {
  const params = new URLSearchParams({ source });
  const response = await fetch(
    `/api/patches/${encodeURIComponent(id)}?${params}`,
  );
  return response.json();
}

export async function loadFile(
  path: string,
  source: QueueSource,
  sha?: string,
): Promise<string> {
  const params = new URLSearchParams({ source, path });
  if (sha) {
    params.set("sha", sha);
  }
  const response = await fetch(`/api/file?${params}`);
  const body = (await response.json()) as FileResponse & { error?: string };
  if (!response.ok) {
    throw new Error(body.error || `could not read ${path}`);
  }
  return body.content;
}

/** What changed in a patch between two revisions of its patch file, file by file. */
export async function loadPatchDiff(
  id: string,
  source: QueueSource,
  from: string,
  to: string,
): Promise<FileDelta[]> {
  const params = new URLSearchParams({ source, from, to });
  const response = await fetch(
    `/api/patches/${encodeURIComponent(id)}/diff?${params}`,
  );
  const body = (await response.json()) as {
    files?: FileDelta[];
    error?: string;
  };
  if (!response.ok || !body.files) {
    throw new Error(body.error || `could not compare revisions of ${id}`);
  }
  return body.files;
}
