import { Link } from "react-router-dom";
import { AppShell } from "../components/app-shell";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { StatusBadge } from "../components/status-badge";

const GATES = [
  {
    when: "Engineering review + merge",
    status: "queued",
    name: "Approved for the internal",
    body: "The change is on company main. The product builds it. Other developers who branch from main get it. Nothing has left EMU. IP has not run.",
  },
  {
    when: "Legal / IP · to-upstream Environment review",
    status: "approved",
    name: "Approved to leave the enterprise",
    body: "Dispatch Uplink submit. IP reads the committed packet and GITHUB_STEP_SUMMARY, then approves the to-upstream Environment. GitHub records the reviewer. The same run then git uplink approve + submit. Internal-only patches never reach this gate.",
  },
  {
    when: "Same run, after to-upstream approval",
    status: "submitted",
    name: "Visible to upstream",
    body: "Bytes are pushed to the upstream-owned private fork and opened as a public PR. This is the first time the work can leave EMU. App credentials exist only on the to-upstream environment.",
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

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">Will this support multiple developers on company main?</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Yes. Treat company main like any protected integration branch: developers merge PRs
            after review; they never push main. Developers branch from latest main, open one
            internal PR per change, and review as usual. Import records the patch on{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">uplink/state</code>.
            Sync force-updates main only when public upstream moved. Rebase in-flight PRs onto that
            new main.
          </p>
          <ul className="space-y-2 text-[15px] leading-7 text-muted-foreground [&_li]:ms-5 [&_li]:list-disc">
            <li>N developers, N feature branches, N internal PRs.</li>
            <li>Zero developers pushing main. Zero second branches for upstream.</li>
            <li>
              Ruleset: require pull requests from humans (they merge); let the bot bypass and
              force-push on sync rebuilds.
            </li>
          </ul>
        </section>

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">How are concurrent adds handled?</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            The dangerous case is two imports that both read the same queue, each append one patch,
            and the later push of <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">uplink/state</code>{" "}
            drops the earlier patch. Uplink does not take last-write-wins on the queue.
          </p>
          <ol className="space-y-2 text-[15px] leading-7 text-muted-foreground [&_li]:ms-5 [&_li]:list-decimal">
            <li>
              Import, sync, and submit jobs share the GitHub Actions concurrency group{" "}
              <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">uplink-mutate</code>.
              Submit uses it per job so IP&apos;s environment wait does not block imports. A second
              job waits; it is not cancelled.
            </li>
            <li>
              In one checkout, <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">.git/uplink.lock</code>{" "}
              serializes queue writes.
            </li>
            <li>
              The product diff is taken from the PR&apos;s own base and head SHAs, so a main that
              moved under the job cannot fold someone else&apos;s patch into this one.
            </li>
            <li>
              The job then refreshes latest main and <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">uplink/state</code>,
              appends, and fast-forward pushes the state branch. If another import
              landed first, the push fails, the job fetches, and it retries.
              The same internal PR number is imported at most once.
            </li>
          </ol>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Overlapping diffs that do not apply are a conflict, not a silent drop. Independent
            files from two developers land on main without talking to each other, aside from
            rebasing after main moved.
          </p>
        </section>

        <section className="space-y-4">
          <h2 className="text-xl font-semibold">When is something approved for the internal?</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            When the internal PR is merged. That is engineering review plus GitHub merge. Import
            then records the patch; status becomes{" "}
            <StatusBadge value="queued" /> and company main already includes it. That is the
            internal product gate.
          </p>
          <p className="text-[15px] leading-7 text-muted-foreground">
            It is not IP approval. Legal can take as long as it needs. The company keeps shipping
            on the queued patch. Contribution approval is the GitHub Environment named{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">to-upstream</code>. Dispatch{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">Uplink submit</code>,
            review the packet, and approve the deployment. Only then may the same run{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">git uplink submit</code>{" "}
            push to the upstream-owned fork. GitHub&apos;s audit log records who approved, not who
            dispatched.
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

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">Can IP approval be a GitHub Environment?</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Yes. On GitHub Enterprise Cloud, create Environment{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">to-upstream</code> with
            IP/legal as required reviewers. Dispatch{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">Uplink submit</code>.
            The packet job commits{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">
              .uplink/reports/&lt;id&gt;/prepare.md
            </code>{" "}
            on <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">uplink/state</code>{" "}
            and writes the Actions job summary. Approving the waiting deployment is the IP gate;
            GitHub&apos;s audit log already records who clicked. The same run then commits{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">approval.md</code> and
            submits. Put App secrets on that environment only so the public fork cannot be pushed
            until review succeeds.
          </p>
        </section>

        <p className="text-sm text-muted-foreground">
          The git engine tests this with two clones racing <code>git uplink push</code> at the
          same time. The{" "}
          <Link to="/playbook" className="text-primary underline-offset-4 hover:underline">
            system playbook
          </Link>{" "}
          has the full operating model. The{" "}
          <Link to="/lab" className="text-primary underline-offset-4 hover:underline">
            live lab
          </Link>{" "}
          walks a single change through both gates.
        </p>
      </article>
    </AppShell>
  );
}
