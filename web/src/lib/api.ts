import type { FileDelta } from "./patch";

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
  /** The internal commit message; its first line is the subject. */
  commitMessage: string;
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
  /** `origin/uplink/state`, read as last fetched; the only queue the UI shows. */
  stateRef: string;
  stateSha?: string;
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

export async function loadSubmitStream(): Promise<SubmitStreamResponse> {
  const response = await fetch("/api/submit-stream");
  return response.json();
}

export async function loadStatus(): Promise<StatusResponse> {
  const response = await fetch("/api/status");
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

export async function loadPatch(id: string): Promise<PatchResponse> {
  const response = await fetch(`/api/patches/${encodeURIComponent(id)}`);
  return response.json();
}

export async function loadFile(path: string, sha?: string): Promise<string> {
  const params = new URLSearchParams({ path });
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
  from: string,
  to: string,
): Promise<FileDelta[]> {
  const params = new URLSearchParams({ from, to });
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
