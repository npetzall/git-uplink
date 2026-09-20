import { useEffect, useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { ArrowRight, RotateCcw, Play } from "lucide-react";
import { Button } from "./ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";
import { StatusBadge } from "./status-badge";
import {
  operationsFor,
  runThrough,
  type LabMode,
  type LabOperation,
  type SimFileMap,
  type SimPatch,
} from "../lib/simulator";
import { DEFAULT_SCENARIO_ID, LAB_SCENARIOS, scenarioById } from "../lib/lab/scenarios";
import { cn } from "../lib/utils";

function FileView({ files, highlight }: { files: SimFileMap; highlight?: string }) {
  const entries = Object.entries(files);
  return (
    <div className="space-y-3">
      {entries.map(([name, contents]) => {
        const conflicted = contents.includes("<<<<<<<");
        return (
          <div key={name}>
            <p className="mb-1 font-mono text-[11px] tracking-wide text-muted-foreground uppercase">
              {name}
            </p>
            <pre
              className={cn(
                "overflow-x-auto rounded-lg bg-black/40 p-3 font-mono text-xs leading-5 text-zinc-200",
                highlight && contents.includes(highlight) && "ring-1 ring-teal-400/40",
                conflicted && "ring-1 ring-rose-400/40",
              )}
            >
              {contents}
            </pre>
          </div>
        );
      })}
    </div>
  );
}

function patchLink(patch: SimPatch, blockedBy?: string) {
  if (patch.mergedVia) return patch.mergedVia;
  if (patch.prNumber) return `upstream#${patch.prNumber}`;
  if (patch.status === "conflict") return `uplink/conflict/${patch.id}-work`;
  if (blockedBy) return `waiting on ${blockedBy}`;
  if (patch.dependsOn.length) return `depends ${patch.dependsOn.join(", ")}`;
  if (patch.queue === "internal") return "internal queue";
  return "queued";
}

function OperationsView({ operations, mode }: { operations: LabOperation[]; mode: LabMode }) {
  if (operations.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        {mode === "ci"
          ? "No CI job runs at this step. Switch to Manual for the local git commands, then push to trigger the workflow."
          : "No command-line operations at this step."}
      </p>
    );
  }
  return (
    <div className="space-y-4" data-testid="lab-operations">
      {operations.map((op, index) => (
        <div key={`${op.actor}-${index}`}>
          <p className="mb-1 text-xs font-medium tracking-wide text-muted-foreground uppercase">
            {op.workflow ?? op.actor}
          </p>
          {op.note ? <p className="mb-2 text-sm text-muted-foreground">{op.note}</p> : null}
          {op.commands.length > 0 ? (
            <pre className="overflow-x-auto rounded-lg bg-black/40 p-3 font-mono text-xs leading-5 text-zinc-200">
              {op.commands.join("\n")}
            </pre>
          ) : null}
        </div>
      ))}
    </div>
  );
}

function parseMode(value: string | null): LabMode {
  return value === "ci" ? "ci" : "manual";
}

export function LabClient() {
  const [params, setParams] = useSearchParams();
  const [stepCount, setStepCount] = useState(0);
  const scenario = scenarioById(params.get("scenario") ?? DEFAULT_SCENARIO_ID);
  const mode = parseMode(params.get("mode"));

  useEffect(() => {
    setStepCount(0);
  }, [scenario.id]);

  const state = useMemo(() => runThrough(scenario, stepCount), [scenario, stepCount]);
  const current = stepCount === 0 ? undefined : scenario.steps[stepCount - 1];
  const canAdvance = stepCount < scenario.steps.length;
  const conflictIndex = state.patches.findIndex((patch) => patch.status === "conflict");
  const blockedBy = conflictIndex >= 0 ? state.patches[conflictIndex]?.id : undefined;
  const operations = operationsFor(
    current?.operations ?? scenario.startOperations,
    mode,
  );

  function writeParams(next: { scenario?: string; mode?: LabMode }) {
    setParams(
      {
        scenario: next.scenario ?? scenario.id,
        mode: next.mode ?? mode,
      },
      { replace: true },
    );
  }

  return (
    <div className="space-y-6">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
        <label className="grid gap-1.5 text-sm">
          <span className="text-xs tracking-wide text-muted-foreground uppercase">Scenario</span>
          <select
            data-testid="lab-scenario"
            value={scenario.id}
            onChange={(event) => {
              setStepCount(0);
              writeParams({ scenario: event.target.value });
            }}
            className="h-9 min-w-[16rem] rounded-lg border border-border bg-background px-3 text-sm text-foreground"
          >
            {LAB_SCENARIOS.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.title}
              </option>
            ))}
          </select>
        </label>
        <div className="flex rounded-lg border border-border p-0.5" role="group" aria-label="Command mode">
          <Button
            type="button"
            size="sm"
            data-testid="lab-mode-manual"
            variant={mode === "manual" ? "default" : "ghost"}
            onClick={() => writeParams({ mode: "manual" })}
          >
            Manual
          </Button>
          <Button
            type="button"
            size="sm"
            data-testid="lab-mode-ci"
            variant={mode === "ci" ? "default" : "ghost"}
            onClick={() => writeParams({ mode: "ci" })}
          >
            CI
          </Button>
        </div>
      </div>

      <div className="grid gap-6 lg:grid-cols-[260px_1fr]">
        <aside className="space-y-2">
          <button
            type="button"
            data-testid="lab-step-0"
            onClick={() => setStepCount(0)}
            className={cn(
              "w-full rounded-lg border px-3 py-2 text-left text-sm",
              stepCount === 0
                ? "border-primary/40 bg-primary/10 text-foreground"
                : "border-border text-muted-foreground hover:bg-muted",
            )}
          >
            0. Fresh company mirror
          </button>
          {scenario.steps.map((step, index) => (
            <button
              key={step.id}
              type="button"
              data-testid={`lab-step-${index + 1}`}
              onClick={() => setStepCount(index + 1)}
              className={cn(
                "w-full rounded-lg border px-3 py-2 text-left text-sm",
                stepCount === index + 1
                  ? "border-primary/40 bg-primary/10 text-foreground"
                  : "border-border text-muted-foreground hover:bg-muted",
              )}
            >
              {index + 1}. {step.title}
            </button>
          ))}
        </aside>

        <div className="space-y-6">
          <Card>
            <CardHeader className="gap-3">
              <p className="text-xs font-medium tracking-[0.2em] text-teal-400 uppercase">
                Scenario · {scenario.title} · {mode} · step {stepCount} of {scenario.steps.length}
              </p>
              <CardTitle className="text-2xl">{current?.title ?? scenario.title}</CardTitle>
              <p className="max-w-3xl text-sm leading-6 text-muted-foreground">
                {current?.summary ?? scenario.blurb}
              </p>
              {current ? (
                <p className="max-w-3xl rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-sm text-amber-100/90">
                  {current.why}
                </p>
              ) : null}
              <div className="flex flex-wrap gap-2">
                <Button
                  type="button"
                  data-testid="lab-next"
                  onClick={() => setStepCount((count) => Math.min(count + 1, scenario.steps.length))}
                  disabled={!canAdvance}
                >
                  {stepCount === 0 ? (
                    <>
                      <Play /> Start
                    </>
                  ) : canAdvance ? (
                    <>
                      Next <ArrowRight />
                    </>
                  ) : (
                    "Lab complete"
                  )}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  data-testid="lab-reset"
                  onClick={() => setStepCount(0)}
                >
                  <RotateCcw /> Reset lab
                </Button>
              </div>
            </CardHeader>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">
                {mode === "ci" ? "CI operations" : "Command-line operations"}
              </CardTitle>
              <p className="text-xs text-muted-foreground">
                {mode === "ci"
                  ? "What the matching GHEC workflow runs. GitHub UI (open PR, merge, dispatch, approve environment) is the trigger, not extra shell."
                  : "Everything you would type locally: git plus git uplink. No Actions."}
              </p>
            </CardHeader>
            <CardContent>
              <OperationsView operations={operations} mode={mode} />
            </CardContent>
          </Card>

          <div className="grid gap-4 xl:grid-cols-3">
            <Card>
              <CardHeader>
                <CardTitle className="text-base">Public upstream</CardTitle>
                <p className="text-xs text-muted-foreground">github.com/upstream/tokenkit</p>
              </CardHeader>
              <CardContent>
                <FileView files={state.upstream} highlight={scenario.highlight} />
              </CardContent>
            </Card>
            <Card className={cn("ring-1 ring-teal-400/20", state.conflict && "ring-rose-400/30")}>
              <CardHeader>
                <CardTitle className="text-base">Company main (what you build)</CardTitle>
                <p className="text-xs text-muted-foreground">
                  {state.conflict
                    ? "Rebuild blocked · last successful main"
                    : "Private forge · upstream + active patches"}
                </p>
              </CardHeader>
              <CardContent>
                <FileView files={state.company} highlight={scenario.highlight} />
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle className="text-base">Contribution fork</CardTitle>
                <p className="text-xs text-muted-foreground">
                  Public, upstream-owned. First place a change becomes public.
                </p>
              </CardHeader>
              <CardContent>
                {state.contrib.length === 0 ? (
                  <p className="text-sm text-muted-foreground">
                    Nothing exported yet. Internal work stays on the private forge until to-upstream
                    Environment approval.
                  </p>
                ) : (
                  <div className="space-y-4">
                    {state.contrib.map((branch) => (
                      <div key={branch.branch} className="space-y-2">
                        <p className="font-mono text-xs text-teal-300">
                          {branch.branch}
                          {branch.prNumber ? ` → PR #${branch.prNumber}` : ""}
                        </p>
                        <FileView files={branch.files} />
                      </div>
                    ))}
                  </div>
                )}
              </CardContent>
            </Card>
          </div>

          {state.conflict ? (
            <Card data-testid="lab-conflict-branch" className="border-rose-500/30">
              <CardHeader>
                <CardTitle className="text-base">Conflict working tree</CardTitle>
                <p className="font-mono text-xs text-rose-200">{state.conflict.branch}</p>
                <p className="text-xs text-muted-foreground">
                  {state.conflict.phase === "fixed"
                    ? "Markers resolved and staged. Status stays conflict until git uplink resolve."
                    : "HEAD is here, not on company main. Later patches are not applied."}
                </p>
              </CardHeader>
              <CardContent>
                <FileView files={state.conflict.files} />
              </CardContent>
            </Card>
          ) : null}

          <Card>
            <CardHeader>
              <CardTitle className="text-base">Patch queue</CardTitle>
            </CardHeader>
            <CardContent className="overflow-x-auto">
              {state.patches.length === 0 ? (
                <p className="text-sm text-muted-foreground">
                  No carried patches. Company main matches upstream.
                </p>
              ) : (
                <table className="w-full min-w-[640px] text-left text-sm">
                  <thead className="text-xs tracking-wide text-muted-foreground uppercase">
                    <tr className="border-b">
                      <th className="py-2 pr-3 font-medium">ID</th>
                      <th className="py-2 pr-3 font-medium">Change</th>
                      <th className="py-2 pr-3 font-medium">Queue</th>
                      <th className="py-2 pr-3 font-medium">Status</th>
                      <th className="py-2 font-medium">Link</th>
                    </tr>
                  </thead>
                  <tbody>
                    {state.patches.map((patch, index) => {
                      const waiting = conflictIndex >= 0 && index > conflictIndex;
                      return (
                        <tr key={patch.id} className="border-b border-border/60">
                          <td className="py-2 pr-3 font-mono text-xs">{patch.id}</td>
                          <td className="py-2 pr-3">{patch.title}</td>
                          <td className="py-2 pr-3">
                            <StatusBadge value={patch.queue} />
                          </td>
                          <td className="py-2 pr-3">
                            <div className="flex flex-wrap items-center gap-2">
                              <StatusBadge value={patch.status} />
                              {waiting ? (
                                <span className="text-xs text-rose-200/80">· blocked</span>
                              ) : null}
                            </div>
                          </td>
                          <td className="py-2 font-mono text-xs text-muted-foreground">
                            {patchLink(patch, waiting ? blockedBy : undefined)}
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              )}
              {state.conflict ? (
                <div
                  data-testid="lab-conflict"
                  className="mt-4 rounded-lg border border-rose-500/30 bg-rose-500/10 p-3 text-sm text-rose-100"
                >
                  {state.conflict.phase === "fixed" ? (
                    <>
                      Working tree on {state.conflict.branch} is clean.{" "}
                      <code className="rounded bg-black/30 px-1.5 py-0.5 font-mono text-xs">
                        git uplink resolve {state.conflict.patchId}
                      </code>{" "}
                      will refresh that patch file and rebuild. It will not push the contribution
                      fork.
                    </>
                  ) : (
                    <>
                      Queue stopped on {state.conflict.patchId}. Company main is frozen.{" "}
                      {state.conflict.blockedIds.length
                        ? `${state.conflict.blockedIds.join(", ")} wait. `
                        : null}
                      Checkout{" "}
                      <code className="rounded bg-black/30 px-1.5 py-0.5 font-mono text-xs">
                        {state.conflict.branch}
                      </code>
                      , fix the files, then resolve the same id.
                    </>
                  )}
                </div>
              ) : null}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-base">Bot log</CardTitle>
            </CardHeader>
            <CardContent>
              <ol className="space-y-2 font-mono text-xs leading-5 text-zinc-400">
                {state.log.map((line, index) => (
                  <li key={`${index}-${line}`}>
                    <span className="text-teal-500">{String(index + 1).padStart(2, "0")}</span> {line}
                  </li>
                ))}
              </ol>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
