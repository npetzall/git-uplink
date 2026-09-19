import { useEffect, useState } from "react";
import { AppShell } from "../components/app-shell";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { StatusBadge } from "../components/status-badge";

type Patch = {
  id: string;
  title: string;
  status: string;
  dependsOn?: string[];
  depends_on?: string[];
};

type StatusResponse = {
  present: boolean;
  cwd: string;
  error?: string;
  companyHead?: string;
  upstreamHead?: string;
  counts?: Record<string, number>;
  tooling?: Patch | null;
  upstream?: Patch[];
  internal?: Patch[];
};

function PatchTable({ patches, empty }: { patches: Patch[]; empty: string }) {
  if (!patches.length) {
    return <p className="text-sm text-muted-foreground">{empty}</p>;
  }
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
            <td className="py-2 pr-3 font-mono text-xs">{patch.id}</td>
            <td className="py-2 pr-3">{patch.title}</td>
            <td className="py-2 pr-3">
              <StatusBadge value={patch.status} />
            </td>
            <td className="py-2 font-mono text-xs text-muted-foreground">
              {(patch.dependsOn ?? patch.depends_on ?? []).join(", ") || "—"}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function QueuePage() {
  const [data, setData] = useState<StatusResponse | null>(null);

  useEffect(() => {
    fetch("/api/status")
      .then((response) => response.json())
      .then(setData)
      .catch((err: Error) =>
        setData({ present: false, cwd: "", error: err.message }),
      );
  }, []);

  const empty =
    !data?.tooling && !(data?.upstream?.length) && !(data?.internal?.length);

  return (
    <AppShell>
      <div className="mb-6 max-w-3xl space-y-2">
        <h1 className="text-3xl font-semibold tracking-tight">This checkout</h1>
        <p className="text-muted-foreground">
          Live queue from the directory where you ran <code className="rounded bg-muted px-1.5">git uplink web-ui</code>.
          The lab on this dashboard is a walkthrough; this page is the real <code className="rounded bg-muted px-1.5">.uplink/queue.json</code> from <code className="rounded bg-muted px-1.5">uplink/state</code>.
          Apply order is tooling, then upstream, then internal last.
        </p>
      </div>

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
