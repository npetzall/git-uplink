import { useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { AppShell } from "../components/app-shell";
import { FinishedFilter } from "../components/finished-filter";
import { PatchTable } from "../components/patch-table";
import { SourceToggle } from "../components/source-toggle";
import { StatusBadge } from "../components/status-badge";
import { SubmitStream } from "../components/submit-stream";
import { Button } from "../components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "../components/ui/card";
import { Tabs } from "../components/ui/tabs";
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
import { LABEL_ORDER, STATUS_INFO, STATUS_ORDER } from "../lib/status";
import { cn } from "../lib/utils";
import {
  finishedStatuses,
  parseShown,
  parseView,
  type QueueView,
  type ShownFinished,
  sourceBranch,
  viewSearch,
  visiblePatches,
} from "../lib/view";

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

/** The Upstream or Internal card, with a dropdown to show its finished patches. */
function LayerCard({
  title,
  layer,
  patches,
  shown,
  onShownChange,
  search,
  stale,
  empty,
}: {
  title: string;
  layer: "upstream" | "internal";
  patches: Patch[];
  shown: ShownFinished[];
  onShownChange: (shown: ShownFinished[]) => void;
  search: string;
  stale?: string[];
  empty: string;
}) {
  const visible = visiblePatches(patches, layer, shown);
  const hidden = patches.length - visible.length;
  const options = finishedStatuses(layer).map((status) => {
    const token = `${layer}-${status}` as ShownFinished;
    return {
      status,
      checked: shown.includes(token),
      onChange: (checked: boolean) =>
        onShownChange(
          checked
            ? [...shown, token]
            : shown.filter((other) => other !== token),
        ),
    };
  });
  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between gap-3">
        <CardTitle className="text-base">{title}</CardTitle>
        <FinishedFilter label={title} options={options} hiddenCount={hidden} />
      </CardHeader>
      <CardContent className="overflow-x-auto">
        <PatchTable
          search={search}
          layer={layer}
          patches={visible}
          stale={stale}
          empty={
            hidden
              ? `${hidden} finished ${hidden === 1 ? "patch is" : "patches are"} hidden.`
              : empty
          }
        />
      </CardContent>
    </Card>
  );
}

export function QueuePage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const source = parseSource(searchParams.get("source"));
  const view = parseView(searchParams.get("view"));
  const shown = parseShown(searchParams.get("show"));
  const search = viewSearch(source, view, shown);
  const [data, setData] = useState<StatusResponse | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
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
  const ready = (data?.readyToSubmit ?? []).flatMap((id) =>
    (data?.upstream ?? []).filter((patch) => patch.id === id),
  );

  function show(
    nextSource: QueueSource,
    nextView: QueueView,
    nextShown: ShownFinished[] = shown,
  ) {
    setSearchParams(
      new URLSearchParams(viewSearch(nextSource, nextView, nextShown)),
    );
  }

  async function onRefresh() {
    setRefreshing(true);
    setRefreshError(null);
    try {
      await refreshRemote();
      setData(await loadStatus(source, false));
      setReload((n) => n + 1);
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
                branch.
              </>
            )}
          </p>
        </div>
        <div className="flex shrink-0 flex-col items-stretch gap-2 sm:items-end">
          <SourceToggle source={source} onChange={(next) => show(next, view)} />
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

          <Tabs
            label="Queue views"
            value={view}
            onChange={(next) => show(source, next)}
            items={[
              { value: "queue", label: "Queue" },
              {
                value: "ready",
                label: (
                  <>
                    Ready to submit
                    <span className="rounded-full bg-muted px-1.5 font-mono text-xs">
                      {ready.length}
                    </span>
                  </>
                ),
              },
              { value: "stream", label: "Submit stream" },
            ]}
          />

          {view === "ready" ? (
            <Card>
              <CardHeader>
                <CardTitle className="text-base">Ready to submit</CardTitle>
                <p className="text-sm text-muted-foreground">
                  Queued upstream patches on {sourceBranch(source)} with no
                  unmerged upstream dependency and no public pull request yet.
                  Dispatch Uplink submit for any of them.
                </p>
              </CardHeader>
              <CardContent className="overflow-x-auto">
                <PatchTable
                  search={search}
                  layer="upstream"
                  patches={ready}
                  stale={data.staleApprovals}
                  empty="Nothing ready to submit."
                />
              </CardContent>
            </Card>
          ) : view === "stream" ? (
            <SubmitStream source={source} search={search} reload={reload} />
          ) : empty ? (
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
                    search={search}
                    layer="tooling"
                    patches={data.tooling ? [data.tooling] : []}
                    empty="No tooling patch. Run git uplink init --forge …"
                  />
                </CardContent>
              </Card>
              <LayerCard
                title="Upstream"
                layer="upstream"
                patches={data.upstream ?? []}
                shown={shown}
                onShownChange={(next) => show(source, view, next)}
                search={search}
                stale={data.staleApprovals}
                empty="No upstream-bound patches."
              />
              <LayerCard
                title="Internal"
                layer="internal"
                patches={data.internal ?? []}
                shown={shown}
                onShownChange={(next) => show(source, view, next)}
                search={search}
                empty="No internal patches."
              />
            </>
          )}
        </div>
      )}
    </AppShell>
  );
}
