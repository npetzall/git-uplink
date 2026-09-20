export type QueueSource = "checkout" | "remote";

export type PatchSource = {
  author?: string;
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
  uncommitted: string[];
};

export type StatusResponse = {
  present: boolean;
  cwd: string;
  source: string;
  error?: string;
  companyHead?: string;
  upstreamHead?: string;
  counts?: Record<string, number>;
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
};

export type FileResponse = {
  path: string;
  sha?: string;
  content: string;
};

export function parseSource(value: string | null): QueueSource {
  return value === "remote" ? "remote" : "checkout";
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
    const body = (await response.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error || `refresh failed (${response.status})`);
  }
}

export async function loadPatch(
  id: string,
  source: QueueSource,
): Promise<PatchResponse> {
  const params = new URLSearchParams({ source });
  const response = await fetch(`/api/patches/${encodeURIComponent(id)}?${params}`);
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
