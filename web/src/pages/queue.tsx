import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { AppShell } from "../components/app-shell";
import { CopyButton } from "../components/copy-button";
import { SourceToggle } from "../components/source-toggle";
import { StatusBadge } from "../components/status-badge";
import { Button } from "../components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "../components/ui/card";
import {
  loadStatus,
  type Patch,
  parseSource,
  type QueueSource,
  refreshRemote,
  type StateStatus,
  type StatusCounts,
  type StatusResponse,
} from "../lib/api";
import {
  LABEL_ORDER,
  NEEDS_APPROVAL,
  STATUS_INFO,
  STATUS_ORDER,
  shownStatus,
} from "../lib/status";
import { cn } from "../lib/utils";

const COUNTERS: { key: keyof StatusCounts; label: string }[] = [
  { key: "queued", label: "Queued" },
  { key: "waiting", label: "Waiting" },
  { key: "submitted", label: "Submitted" },
  { key: "merged", label: "Merged" },
  { key: "rejected", label: "Rejected" },
  { key: "internal", label: "Internal" },
  { key: "active", label: "Active patches" },
];

function LegendList({ values }: { values: string[] }) {
  return (
    <dl className="mt-3 space-y-3">
      {values.map((value) => (
        <div
          key={value}
          className="grid gap-1 sm:grid-cols-[7rem_1fr] sm:gap-3"
        >
          <dt>
            <StatusBadge value={value} />
          </dt>
          <dd className="space-y-1 text-muted-foreground">
            <p>{STATUS_INFO[value].meaning}</p>
            <p>
              <span className="text-foreground">Next:</span>{" "}
              {STATUS_INFO[value].next}
            </p>
          </dd>
        </div>
      ))}
    </dl>
  );
}

function StatusLegend() {
  return (
    <details className="mb-6 rounded-xl border border-border bg-card px-4 py-3 text-sm">
      <summary className="cursor-pointer font-medium">
        What do the statuses mean?
      </summary>
      <LegendList values={STATUS_ORDER} />
      <h3 className="mt-5 font-medium">Labels</h3>
      <LegendList values={LABEL_ORDER} />
    </details>
  );
}

function PatchTable({
  patches,
  empty,
  source,
  layer,
  stale = [],
}: {
  patches: Patch[];
  empty: string;
  source: QueueSource;
  layer: "tooling" | "upstream" | "internal";
  /** Ids whose approval no longer covers their content. */
  stale?: string[];
}) {
  if (!patches.length) {
    return <p className="text-sm text-muted-foreground">{empty}</p>;
  }
  const query = source === "remote" ? "?source=remote" : "";
  return (
    <table className="w-full min-w-[560px] text-left text-sm">
      <thead className="text-xs tracking-wide text-muted-foreground uppercase">
        <tr className="border-b">
          <th className="py-2 pr-3 font-medium">ID</th>
          <th className="py-2 pr-3 font-medium">Change</th>
          <th className="py-2 pr-3 font-medium">Status</th>
          <th className="py-2 font-medium">Depends on</th>
        </tr>
      </thead>
      <tbody>
        {patches.map((patch) => (
          <tr key={patch.id} className="border-b border-border/60">
            <td className="py-2 pr-3 font-mono text-xs">
              <span className="inline-flex items-center gap-1">
                <Link
                  to={`/patches/${encodeURIComponent(patch.id)}${query}`}
                  className="text-primary hover:underline"
                >
                  {patch.id}
                </Link>
                <CopyButton
                  value={patch.id}
                  label={`Copy patch id ${patch.id}`}
                />
              </span>
            </td>
            <td className="py-2 pr-3">
              <Link
                to={`/patches/${encodeURIComponent(patch.id)}${query}`}
                className="hover:underline"
              >
                {patch.title}
              </Link>
            </td>
            <td className="space-x-1 py-2 pr-3">
              <StatusBadge value={shownStatus(patch.status, layer)} />
              {stale.includes(patch.id) ? (
                <StatusBadge value={NEEDS_APPROVAL} />
              ) : null}
            </td>
            <td className="py-2 font-mono text-xs text-muted-foreground">
              {(patch.dependsOn ?? []).join(", ") || "—"}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** How the local uplink/state compares with origin. */
function syncLabel(state: StateStatus): string {
  if (!state.remote) {
    return "no origin tracking";
  }
  const parts: string[] = [];
  if (state.ahead) {
    parts.push(`${state.ahead} ahead of origin`);
  }
  if (state.behind) {
    parts.push(`${state.behind} behind origin`);
  }
  return parts.length ? parts.join(", ") : "up to date with origin";
}

function SyncPill({ state }: { state: StateStatus }) {
  return (
    <span
      className={cn(
        "rounded-full border px-2.5 py-0.5 align-middle text-xs font-normal tracking-normal",
        state.behind
          ? "border-amber-500/30 bg-amber-500/10 text-amber-300"
          : "border-border text-muted-foreground",
      )}
      title={`Local uplink/state against ${state.remoteRef ?? "origin/uplink/state"}`}
    >
      {syncLabel(state)}
    </span>
  );
}

export function QueuePage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const source = parseSource(searchParams.get("source"));
  const [data, setData] = useState<StatusResponse | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const fetchedOnce = useRef(false);

  useEffect(() => {
    const first = !fetchedOnce.current;
    fetchedOnce.current = true;
    loadStatus(source, first)
      .then(setData)
      .catch((err: Error) =>
        setData({
          present: false,
          cwd: "",
          source,
          error: err.message,
        }),
      );
  }, [source]);

  const empty =
    !data?.tooling && !data?.upstream?.length && !data?.internal?.length;

  function setSource(next: QueueSource) {
    if (next === "local") {
      setSearchParams({});
    } else {
      setSearchParams({ source: next });
    }
  }

  async function onRefresh() {
    setRefreshing(true);
    setRefreshError(null);
    try {
      await refreshRemote();
      setData(await loadStatus(source, false));
    } catch (err) {
      setRefreshError(err instanceof Error ? err.message : String(err));
    } finally {
      setRefreshing(false);
    }
  }

  return (
    <AppShell>
      <div className="mb-6 flex flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div className="space-y-2">
          <h1 className="flex flex-wrap items-center gap-3 text-3xl font-semibold tracking-tight">
            {source === "remote" ? "Remote" : "Local"}{" "}
            <span className="font-mono text-2xl">uplink/state</span>
            {source === "local" && data?.state ? (
              <SyncPill state={data.state} />
            ) : null}
          </h1>
          <p className="max-w-3xl text-muted-foreground">
            {source === "remote" ? (
              <>
                Queue at{" "}
                <code className="rounded bg-muted px-1.5">
                  origin/uplink/state
                </code>{" "}
                as last fetched. Local branches are not moved.
              </>
            ) : (
              <>
                Queue on the local{" "}
                <code className="rounded bg-muted px-1.5">uplink/state</code>{" "}
                branch of the repository where you ran{" "}
                <code className="rounded bg-muted px-1.5">
                  git uplink web-ui
                </code>
                , read with git. Uncommitted edits in{" "}
                <code className="rounded bg-muted px-1.5">.uplink/</code> are
                not shown.
              </>
            )}
          </p>
        </div>
        <div className="flex shrink-0 flex-col items-stretch gap-2 sm:items-end">
          <SourceToggle source={source} onChange={setSource} />
          <Button
            variant="outline"
            size="sm"
            onClick={onRefresh}
            disabled={refreshing}
          >
            {refreshing ? "Refreshing…" : "Refresh remote"}
          </Button>
        </div>
      </div>

      <StatusLegend />

      {refreshError ? (
        <p className="mb-6 text-sm text-rose-300">{refreshError}</p>
      ) : null}

      {!data ? (
        <p className="text-sm text-muted-foreground">Reading queue…</p>
      ) : !data.present ? (
        <Card>
          <CardHeader>
            <CardTitle>No Uplink queue here</CardTitle>
          </CardHeader>
          <CardContent className="space-y-2 text-sm text-muted-foreground">
            <p>
              Started in{" "}
              <span className="font-mono text-foreground">
                {data.cwd || "(unknown)"}
              </span>
              {data.error ? <> — {data.error}</> : null}.
            </p>
            <p>
              <code className="rounded bg-muted px-1.5">cd</code> into a product
              repository and run{" "}
              <code className="rounded bg-muted px-1.5">git uplink init</code>,
              then open the UI again.
            </p>
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-6">
          <div className="grid gap-3 md:grid-cols-3">
            <Card>
              <CardHeader>
                <CardTitle className="text-base">Company HEAD</CardTitle>
              </CardHeader>
              <CardContent className="font-mono text-xs break-all text-muted-foreground">
                {data.companyHead ?? "—"}
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle className="text-base">uplink/upstream</CardTitle>
              </CardHeader>
              <CardContent className="font-mono text-xs break-all text-muted-foreground">
                {data.upstreamHead ?? "not fetched"}
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle className="text-base">Counts</CardTitle>
              </CardHeader>
              <CardContent className="flex flex-wrap gap-2 text-sm">
                {COUNTERS.map(({ key, label }) => (
                  <span
                    key={key}
                    className="rounded-md border border-border px-2 py-1 font-mono text-xs"
                  >
                    {label} {data.counts?.[key] ?? 0}
                  </span>
                ))}
              </CardContent>
            </Card>
          </div>
          {empty ? (
            <Card>
              <CardHeader>
                <CardTitle className="text-base">Patch queue</CardTitle>
              </CardHeader>
              <CardContent>
                <p className="text-sm text-muted-foreground">
                  Queue is empty. Company main matches upstream.
                </p>
              </CardContent>
            </Card>
          ) : (
            <>
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Tooling</CardTitle>
                </CardHeader>
                <CardContent className="overflow-x-auto">
                  <PatchTable
                    source={source}
                    layer="tooling"
                    patches={data.tooling ? [data.tooling] : []}
                    empty="No tooling patch. Run git uplink init --forge …"
                  />
                </CardContent>
              </Card>
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Upstream</CardTitle>
                </CardHeader>
                <CardContent className="overflow-x-auto">
                  <PatchTable
                    source={source}
                    layer="upstream"
                    patches={data.upstream ?? []}
                    stale={data.staleApprovals}
                    empty="No upstream-bound patches."
                  />
                </CardContent>
              </Card>
              <Card>
                <CardHeader>
                  <CardTitle className="text-base">Internal</CardTitle>
                </CardHeader>
                <CardContent className="overflow-x-auto">
                  <PatchTable
                    source={source}
                    layer="internal"
                    patches={data.internal ?? []}
                    empty="No internal patches."
                  />
                </CardContent>
              </Card>
            </>
          )}
        </div>
      )}
    </AppShell>
  );
}
