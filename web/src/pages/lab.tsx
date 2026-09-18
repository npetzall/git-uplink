import { AppShell } from "../components/app-shell";
import { LabClient } from "../components/lab-client";

export function LabPage() {
  return (
    <AppShell>
      <div className="mb-6 max-w-3xl space-y-2">
        <h1 className="text-3xl font-semibold tracking-tight">Live lab</h1>
        <p className="text-muted-foreground">
          Same lifecycle the git uplink engine tests against real git: carry patches, stack on
          unmerged work, keep an internal-only escape hatch, export after approval, drop on merge so
          a later upstream fix survives, then walk a sync conflict through checkout, resolve, and a
          second to-upstream approval before the amended patch reaches the fork.
        </p>
      </div>
      <LabClient />
    </AppShell>
  );
}
