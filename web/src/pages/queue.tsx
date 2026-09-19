import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { AppShell } from "../components/app-shell";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Button } from "../components/ui/button";
import { StatusBadge } from "../components/status-badge";
import {
  loadStatus,
  parseSource,
  refreshRemote,
  type Patch,
  type QueueSource,
  type StatusResponse,
} from "../lib/api";
import { cn } from "../lib/utils";

function PatchTable({
  patches,
  empty,
  source,
}: {
  patches: Patch[];
  empty: string;
  source: QueueSource;
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
              <Link
                to={`/patches/${encodeURIComponent(patch.id)}${query}`}
                className="text-primary hover:underline"
              >
                {patch.id}
              </Link>
            </td>
            <td className="py-2 pr-3">
              <Link
                to={`/patches/${encodeURIComponent(patch.id)}${query}`}
                className="hover:underline"
              >
                {patch.title}
              </Link>
            </td>
            <td className="py-2 pr-3">
              <StatusBadge value={patch.status} />
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

function syncLabel(data: StatusResponse): string {
  const state = data.state;
  if (!state) {
    return "no origin tracking";
  }
  if (state.ahead === 0 && state.behind === 0) {
    return "up to date with origin";
  }
  const parts: string[] = [];
  if (state.ahead) {
    parts.push(`ahead ${state.ahead}`);
  }
  if (state.behind) {
    parts.push(`behind ${state.behind}`);
  }
  if (!parts.length) {
    return "no origin tracking";
  }
  return `${parts.join("  ")} ${state.remoteRef ?? "origin/uplink/state"}`;
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
    !data?.tooling && !(data?.upstream?.length) && !(data?.internal?.length);

  function setSource(next: QueueSource) {
    if (next === "checkout") {
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
      <div className="mb-6 flex max-w-3xl flex-col gap-4 sm:flex-row sm:items-start sm:justify-between">
        <div className="space-y-2">
          <h1 className="text-3xl font-semibold tracking-tight">
            {source === "remote" ? "Remote queue" : "This checkout"}
          </h1>
          <p className="text-muted-foreground">
            {source === "remote" ? (
              <>
                Queue at <code className="rounded bg-muted px-1.5">origin/uplink/state</code>. Local
                branches are not moved.
              </>
            ) : (
              <>
                Live queue from the directory where you ran{" "}
                <code className="rounded bg-muted px-1.5">git uplink web-ui</code>. This is{" "}
                <code className="rounded bg-muted px-1.5">.uplink/queue.json</code> from{" "}
                <code className="rounded bg-muted px-1.5">uplink/state</code>.
              </>
            )}
          </p>
        </div>
        <div className="flex shrink-0 flex-col items-stretch gap-2 sm:items-end">
          <div className="flex rounded-lg border border-border p-0.5">
            <Button
              variant={source === "checkout" ? "secondary" : "ghost"}
              size="sm"
              onClick={() => setSource("checkout")}
            >
              Checkout
            </Button>
            <Button
              variant={source === "remote" ? "secondary" : "ghost"}
              size="sm"
              onClick={() => setSource("remote")}
            >
              Remote
            </Button>
          </div>
          <Button variant="outline" size="sm" onClick={onRefresh} disabled={refreshing}>
            {refreshing ? "Refreshing…" : "Refresh remote"}
          </Button>
        </div>
      </div>

      {data?.state ? (
        <p
          className={cn(
            "mb-6 text-sm",
            data.state.behind
              ? "text-amber-300"
              : "text-muted-foreground",
          )}
        >
          {syncLabel(data)}
          {data.state.uncommitted.length
            ? ` · ${data.state.uncommitted.length} uncommitted .uplink path(s)`
            : ""}
        </p>
      ) : null}
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
              Started in <span className="font-mono text-foreground">{data.cwd || "(unknown)"}</span>
              {data.error ? <> — {data.error}</> : null}.
            </p>
            <p>
              <code className="rounded bg-muted px-1.5">cd</code> into a product repository and run{" "}
              <code className="rounded bg-muted px-1.5">git uplink init</code>, then open the UI again.
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
              <CardContent className="font-mono text-xs text-muted-foreground">
                {data.companyHead ?? "—"}
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle className="text-base">uplink/upstream</CardTitle>
              </CardHeader>
              <CardContent className="font-mono text-xs text-muted-foreground">
                {data.upstreamHead ?? "not fetched"}
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle className="text-base">Counts</CardTitle>
              </CardHeader>
              <CardContent className="flex flex-wrap gap-2 text-sm">
                {Object.entries(data.counts ?? {}).map(([key, value]) => (
                  <span key={key} className="rounded-md border border-border px-2 py-1 font-mono text-xs">
                    {key} {value}
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
                <p className="text-sm text-muted-foreground">Queue is empty. Company main matches upstream.</p>
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
                    patches={data.upstream ?? []}
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
