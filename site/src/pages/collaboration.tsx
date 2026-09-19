import { Link } from "react-router-dom";
import { AppShell } from "../components/app-shell";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { StatusBadge } from "../components/status-badge";

const GATES = [
  {
    when: "Engineering review + merge",
    status: "queued",
    name: "Approved for the internal",
    body: "The change is on company main. The product builds it. Other developers who branch from main get it. Nothing has left the private forge. IP has not run.",
  },
  {
    when: "Legal / IP · to-upstream Environment review",
    status: "approved",
    name: "Approved to leave the private forge",
    body: "Dispatch Uplink submit. IP reads the committed packet and GITHUB_STEP_SUMMARY, then approves the to-upstream Environment. GitHub records the reviewer. The same run then git uplink approve + submit. Internal-only patches never reach this gate.",
  },
  {
    when: "Same run, after to-upstream approval",
    status: "submitted",
    name: "Visible to upstream",
    body: "Bytes are pushed to the public contribution fork and opened as an upstream PR. This is the first time the work leaves the private forge. App credentials exist only on the to-upstream environment.",
  },
  {
    when: "Submitted patch hits a sync conflict, then resolve",
    status: "amended",
    name: "Delta waiting for IP",
    body: "Company main already has the resolved patch. The fork still has the last approved bytes. Resolve dispatches Uplink submit; IP reviews a delta packet. Approving force-pushes the same public PR.",
  },
];

export function CollaborationPage() {
  return (
    <AppShell>
      <article className="mx-auto max-w-3xl space-y-10">
        <header className="space-y-3">
          <p className="text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">
            Collaboration
          </p>
          <h1 className="text-4xl font-semibold tracking-tight">
            Multiple developers, concurrent adds, and when a change is internal
          </h1>
          <p className="text-lg leading-8 text-muted-foreground">
            Company <code className="rounded bg-muted px-1.5 py-0.5 text-[15px] text-foreground">main</code> is
            shared. Humans never push it; they merge PRs. Two reviewed PRs can land at the same time
            without dropping a patch. Landing on main is not the same event as approving a
            contribution.
          </p>
        </header>

        <section className="space-y-3 text-[15px] leading-7 text-muted-foreground">
          <p>
            Any number of developers branch from latest main, open one internal PR per change, and
            merge after review. Import records the patch on{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">uplink/state</code>.
            Concurrent imports serialize so two adds cannot drop a patch. Contribution approval is
            the GitHub Environment named{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">to-upstream</code>, not
            the merge.
          </p>
          <p>
            Multi-dev ruleset, concurrent adds, and the to-upstream Environment:{" "}
            <Link to="/playbook" className="text-primary underline-offset-4 hover:underline">
              playbook
            </Link>
            .
          </p>
        </section>

        <section className="space-y-4">
          <h2 className="text-xl font-semibold">When is something approved for the internal?</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            When the internal PR is merged. That is engineering review plus GitHub merge. Import
            then records the patch; status becomes{" "}
            <StatusBadge value="queued" /> and company main already includes it. That is the
            internal product gate. It is not IP approval. Legal can take as long as it needs. The
            company keeps shipping on the queued patch.
          </p>
          <div className="grid gap-3">
            {GATES.map((gate) => (
              <Card key={gate.status}>
                <CardHeader className="flex flex-row items-start justify-between gap-3 space-y-0">
                  <div>
                    <p className="text-xs tracking-wide text-muted-foreground uppercase">
                      {gate.when}
                    </p>
                    <CardTitle className="mt-1 text-base">{gate.name}</CardTitle>
                  </div>
                  <StatusBadge value={gate.status} />
                </CardHeader>
                <CardContent className="text-sm leading-6 text-muted-foreground">
                  {gate.body}
                </CardContent>
              </Card>
            ))}
          </div>
        </section>

        <p className="text-sm text-muted-foreground">
          The{" "}
          <Link to="/lab" className="text-primary underline-offset-4 hover:underline">
            live lab
          </Link>{" "}
          walks a single change through both gates.
        </p>
      </article>
    </AppShell>
  );
}
