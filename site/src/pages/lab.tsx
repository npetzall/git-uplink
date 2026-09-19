import { AppShell } from "../components/app-shell";
import { LabClient } from "../components/lab-client";

export function LabPage() {
  return (
    <AppShell>
      <div className="mb-6 max-w-3xl space-y-3">
        <p className="text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">Lab</p>
        <h1 className="text-4xl font-semibold tracking-tight">Live lab</h1>
        <p className="text-lg leading-8 text-muted-foreground">
          Pick a scenario and step through the same lifecycle the git uplink engine tests against
          real git. Toggle Manual to see every CLI command you would type, or CI for only the git
          uplink (and gh) lines the GHEC workflows run.
        </p>
      </div>
      <LabClient />
    </AppShell>
  );
}
