import { useEffect, useMemo, useState } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { AppShell } from "../components/app-shell";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { StatusBadge } from "../components/status-badge";
import {
  loadFile,
  loadPatch,
  parseSource,
  type FileRevision,
  type PatchApproval,
  type PatchResponse,
} from "../lib/api";

function shortSha(sha: string): string {
  return sha === "worktree" ? "worktree" : sha.slice(0, 12);
}

function revisionLabel(revision: FileRevision): string {
  const subject = revision.subject || "update";
  const when = revision.at ? ` · ${revision.at}` : "";
  return `${shortSha(revision.sha)} — ${subject}${when}`;
}

function approvalLabel(approval: PatchApproval): string {
  return `v${approval.version} (${approval.kind}) · ${shortSha(approval.sha)}`;
}

export function PatchPage() {
  const { id = "" } = useParams();
  const [searchParams] = useSearchParams();
  const source = parseSource(searchParams.get("source"));
  const home = source === "remote" ? "/?source=remote" : "/";

  const [data, setData] = useState<PatchResponse | null>(null);
  const [patchSha, setPatchSha] = useState<string>("");
  const [patchText, setPatchText] = useState<string>("");
  const [approvalSha, setApprovalSha] = useState<string>("");
  const [approvalText, setApprovalText] = useState<string>("");
  const [prepareText, setPrepareText] = useState<string>("");
  const [approvedPatch, setApprovedPatch] = useState<string>("");
  const [fileError, setFileError] = useState<string | null>(null);

  useEffect(() => {
    if (!id) {
      return;
    }
    loadPatch(id, source)
      .then((body) => {
        setData(body);
        const first = body.revisions[0]?.sha ?? "";
        setPatchSha(first);
        setPatchText(body.patchFile ?? "");
        const last = body.patch?.approvals?.at(-1);
        setApprovalSha(last?.sha ?? "");
      })
      .catch((err: Error) =>
        setData({
          present: false,
          source,
          error: err.message,
          revisions: [],
        }),
      );
  }, [id, source]);

  const approvals = data?.patch?.approvals ?? [];
  const selectedApproval = useMemo(
    () => approvals.find((item) => item.sha === approvalSha) ?? approvals.at(-1),
    [approvals, approvalSha],
  );

  useEffect(() => {
    if (!id || !patchSha) {
      return;
    }
    if (data?.revisions[0]?.sha === patchSha && data.patchFile != null) {
      setPatchText(data.patchFile);
      return;
    }
    const path = `.uplink/patches/${id}.patch`;
    loadFile(path, source, patchSha)
      .then(setPatchText)
      .catch((err: Error) => setFileError(err.message));
  }, [id, patchSha, source, data]);

  useEffect(() => {
    if (!id || !selectedApproval) {
      setApprovalText("");
      setPrepareText("");
      setApprovedPatch("");
      return;
    }
    const dir = `.uplink/reports/${id}`;
    Promise.allSettled([
      loadFile(`${dir}/approval.md`, source, selectedApproval.sha),
      loadFile(`${dir}/prepare.md`, source, selectedApproval.sha),
      loadFile(`.uplink/patches/${id}.patch`, source, selectedApproval.sha),
    ]).then(([approval, prepare, patch]) => {
      setApprovalText(approval.status === "fulfilled" ? approval.value : "");
      setPrepareText(prepare.status === "fulfilled" ? prepare.value : "");
      setApprovedPatch(patch.status === "fulfilled" ? patch.value : "");
    });
  }, [id, source, selectedApproval]);

  const patch = data?.patch;

  return (
    <AppShell>
      <div className="mb-6 space-y-2">
        <p className="text-sm">
          <Link to={home} className="text-primary hover:underline">
            ← Queue
          </Link>
        </p>
        <h1 className="text-3xl font-semibold tracking-tight font-mono">
          {id || "Patch"}
        </h1>
        {patch ? (
          <div className="flex flex-wrap items-center gap-2">
            <StatusBadge value={patch.status} />
            {data?.layer ? <StatusBadge value={data.layer} /> : null}
            <span className="text-lg text-foreground">{patch.title}</span>
          </div>
        ) : null}
      </div>

      {!data ? (
        <p className="text-sm text-muted-foreground">Reading patch…</p>
      ) : !data.present || !patch ? (
        <Card>
          <CardHeader>
            <CardTitle>Patch not found</CardTitle>
          </CardHeader>
          <CardContent className="text-sm text-muted-foreground">
            {data.error ?? `No patch ${id} in this ${source} queue.`}
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-6">
          {fileError ? <p className="text-sm text-rose-300">{fileError}</p> : null}
          <Card>
            <CardHeader>
              <CardTitle className="text-base">Details</CardTitle>
            </CardHeader>
            <CardContent className="space-y-3 text-sm">
              <dl className="grid gap-2 sm:grid-cols-2">
                <div>
                  <dt className="text-xs text-muted-foreground uppercase">Created</dt>
                  <dd>{patch.createdAt ?? "—"}</dd>
                </div>
                <div>
                  <dt className="text-xs text-muted-foreground uppercase">Updated</dt>
                  <dd>{patch.updatedAt ?? "—"}</dd>
                </div>
                <div>
                  <dt className="text-xs text-muted-foreground uppercase">Depends on</dt>
                  <dd className="font-mono text-xs">
                    {(patch.dependsOn ?? []).join(", ") || "—"}
                  </dd>
                </div>
                <div>
                  <dt className="text-xs text-muted-foreground uppercase">Stable patch-id</dt>
                  <dd className="font-mono text-xs">{patch.patchIdStable ?? "—"}</dd>
                </div>
              </dl>
              {patch.upstream?.prUrl ? (
                <p>
                  Contrib PR:{" "}
                  <a
                    href={patch.upstream.prUrl}
                    className="text-primary hover:underline"
                    target="_blank"
                    rel="noreferrer"
                  >
                    {patch.upstream.prUrl}
                  </a>
                </p>
              ) : null}
              {patch.conflict ? (
                <p className="text-amber-300">
                  Conflict on {patch.conflict.branch}
                  {patch.conflict.workBranch ? (
                    <>
                      {" "}
                      · work {patch.conflict.workBranch}
                    </>
                  ) : null}
                  {patch.conflict.prUrl ? (
                    <>
                      {" "}
                      ·{" "}
                      <a
                        href={patch.conflict.prUrl}
                        className="text-primary hover:underline"
                        target="_blank"
                        rel="noreferrer"
                      >
                        company PR
                      </a>
                    </>
                  ) : null}
                </p>
              ) : null}
              {patch.commitMessage ? (
                <pre className="overflow-x-auto rounded-md bg-muted p-3 text-xs whitespace-pre-wrap">
                  {patch.commitMessage}
                </pre>
              ) : null}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">Patch</CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              {data.revisions.length ? (
                <label className="block text-sm">
                  <span className="mb-1 block text-xs text-muted-foreground uppercase">
                    Revision
                  </span>
                  <select
                    className="w-full max-w-xl rounded-md border border-border bg-background px-2 py-1.5 text-sm"
                    value={patchSha}
                    onChange={(event) => setPatchSha(event.target.value)}
                  >
                    {data.revisions.map((revision) => (
                      <option key={revision.sha} value={revision.sha}>
                        {revisionLabel(revision)}
                      </option>
                    ))}
                  </select>
                </label>
              ) : (
                <p className="text-sm text-muted-foreground">No patch file history.</p>
              )}
              <pre className="max-h-[32rem] overflow-auto rounded-md bg-muted p-3 text-xs">
                {patchText || "(empty)"}
              </pre>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">Approvals</CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              {!approvals.length ? (
                <p className="text-sm text-muted-foreground">No approvals recorded.</p>
              ) : (
                <>
                  <label className="block text-sm">
                    <span className="mb-1 block text-xs text-muted-foreground uppercase">
                      Approval
                    </span>
                    <select
                      className="w-full max-w-xl rounded-md border border-border bg-background px-2 py-1.5 text-sm"
                      value={selectedApproval?.sha ?? ""}
                      onChange={(event) => setApprovalSha(event.target.value)}
                    >
                      {approvals.map((approval) => (
                        <option key={`${approval.version}-${approval.sha}`} value={approval.sha}>
                          {approvalLabel(approval)}
                        </option>
                      ))}
                    </select>
                  </label>
                  {selectedApproval ? (
                    <dl className="grid gap-2 text-sm sm:grid-cols-2">
                      <div>
                        <dt className="text-xs text-muted-foreground uppercase">At</dt>
                        <dd>{selectedApproval.at}</dd>
                      </div>
                      <div>
                        <dt className="text-xs text-muted-foreground uppercase">Queue commit</dt>
                        <dd className="font-mono text-xs">{selectedApproval.sha}</dd>
                      </div>
                      {selectedApproval.runUrl ? (
                        <div className="sm:col-span-2">
                          <dt className="text-xs text-muted-foreground uppercase">Run</dt>
                          <dd>
                            <a
                              href={selectedApproval.runUrl}
                              className="text-primary hover:underline"
                              target="_blank"
                              rel="noreferrer"
                            >
                              {selectedApproval.runUrl}
                            </a>
                          </dd>
                        </div>
                      ) : null}
                    </dl>
                  ) : null}
                  {approvalText ? (
                    <pre className="max-h-80 overflow-auto rounded-md bg-muted p-3 text-xs whitespace-pre-wrap">
                      {approvalText}
                    </pre>
                  ) : null}
                  {prepareText ? (
                    <details>
                      <summary className="cursor-pointer text-sm text-muted-foreground">
                        Prepare packet at this approval
                      </summary>
                      <pre className="mt-2 max-h-80 overflow-auto rounded-md bg-muted p-3 text-xs whitespace-pre-wrap">
                        {prepareText}
                      </pre>
                    </details>
                  ) : null}
                  {approvedPatch ? (
                    <details>
                      <summary className="cursor-pointer text-sm text-muted-foreground">
                        Patch file at this approval
                      </summary>
                      <pre className="mt-2 max-h-80 overflow-auto rounded-md bg-muted p-3 text-xs">
                        {approvedPatch}
                      </pre>
                    </details>
                  ) : null}
                </>
              )}
            </CardContent>
          </Card>

          {patch.events?.length ? (
            <Card>
              <CardHeader>
                <CardTitle className="text-base">Events</CardTitle>
              </CardHeader>
              <CardContent>
                <ul className="space-y-2 text-sm">
                  {patch.events.map((event, index) => (
                    <li key={`${event.at}-${index}`}>
                      <span className="font-mono text-xs text-muted-foreground">{event.at}</span>{" "}
                      <span className="font-medium">{event.type}</span>{" "}
                      <span className="text-muted-foreground">{event.detail}</span>
                    </li>
                  ))}
                </ul>
              </CardContent>
            </Card>
          ) : null}
        </div>
      )}
    </AppShell>
  );
}
