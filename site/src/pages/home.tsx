import { Link } from "react-router-dom";
import { ArrowRight, GitPullRequest, ShieldCheck, Waypoints } from "lucide-react";
import { AppShell } from "../components/app-shell";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { GITHUB_REPO } from "../lib/links";

const GATES = [
  {
    title: "One branch per change",
    body: "Developers open a single internal PR on the private forge. git uplink extracts a patch object. The contribution fork branch is generated, never maintained by hand.",
  },
  {
    title: "Stay current, keep your deltas",
    body: "Company main is public upstream plus tooling, then queued upstream patches, then internal patches last. Merge lands a change on main; import records it on uplink/state. Upstream import rebuilds so the new patch sits under internal. Sync rebuilds main only when upstream moved. Merged work is dropped so later upstream fixes are not reverted.",
  },
  {
    title: "IP control before publicity",
    body: "Landing on company main is the internal product gate. The change stays on the private forge until IP approves the to-upstream GitHub Environment. That same run exports to the public contribution fork, then opens a normal upstream pull request.",
  },
];

export function HomePage() {
  return (
    <AppShell>
      <section className="max-w-3xl space-y-5">
        <p className="text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">
          Private forge → contribution fork → upstream → gated sync
        </p>
        <h1 className="text-4xl font-semibold tracking-tight text-balance md:text-5xl">
          Contribute upstream without a second copy of every change.
        </h1>
        <p className="text-lg leading-8 text-muted-foreground text-pretty">
          git uplink is the Git subcommand for a company on a private forge that must build on a
          public project, keep unreleased work private, pass IP review, and still make it easy for
          upstream maintainers to merge. EMU on GitHub Enterprise Cloud is one such forge.
        </p>
        <div className="flex flex-wrap gap-3">
          <Button asChild>
            <Link to="/lab">
              Run the live lab <ArrowRight />
            </Link>
          </Button>
          <Button asChild variant="outline">
            <Link to="/install">Install</Link>
          </Button>
          <Button asChild variant="outline">
            <Link to="/working">Way of working</Link>
          </Button>
          <Button asChild variant="outline">
            <Link to="/playbook">Read the playbook</Link>
          </Button>
          <Button asChild variant="outline">
            <Link to="/internals">Internals</Link>
          </Button>
          <Button asChild variant="outline">
            <a href={GITHUB_REPO}>GitHub</a>
          </Button>
        </div>
      </section>

      <div className="mt-12 grid gap-4 md:grid-cols-3">
        {GATES.map((gate) => (
          <Card key={gate.title}>
            <CardHeader>
              <CardTitle className="text-base">{gate.title}</CardTitle>
            </CardHeader>
            <CardContent className="text-sm leading-6 text-muted-foreground">{gate.body}</CardContent>
          </Card>
        ))}
      </div>

      <section className="mt-12 space-y-4">
        <h2 className="text-xl font-semibold">Two gates, not one</h2>
        <p className="max-w-3xl text-sm leading-6 text-muted-foreground">
          Multiple developers share company main through internal PRs. Importing a PR is{" "}
          <strong className="text-foreground">approved for the internal</strong> — the product builds
          it. Legal review is a later <strong className="text-foreground">to-upstream Environment</strong>{" "}
          approval on the submit workflow, before anything is pushed to the public contribution fork.
          Concurrent imports publish with <code className="rounded bg-muted px-1 py-0.5 text-foreground">git uplink push</code> so two adds cannot drop a patch.
          Those two gates are outbound. Inbound review of foreign public commits is the{" "}
          <strong className="text-foreground">from-upstream Environment</strong>.
        </p>
        <Button asChild variant="outline" size="sm">
          <Link to="/collaboration">
            How collaboration works <ArrowRight />
          </Link>
        </Button>
      </section>

      <section className="mt-12 space-y-4">
        <h2 className="text-xl font-semibold">Then it becomes public maintenance</h2>
        <p className="max-w-3xl text-sm leading-6 text-muted-foreground">
          After IP export and an ordinary upstream merge, the change is public. Sync flows it back,
          marks the patch merged, and drops the internal copy so company{" "}
          <code className="rounded bg-muted px-1 py-0.5 text-foreground">main</code> is rebuilt as
          public upstream plus remaining private patches. Flow-back of our own patches is automatic
          (trailer / <code className="rounded bg-muted px-1 py-0.5 text-foreground">patch-id</code>
          ). Foreign public commits wait on the{" "}
          <code className="rounded bg-muted px-1 py-0.5 text-foreground">from-upstream</code>{" "}
          Environment — the company does not silently take unrelated upstream onto{" "}
          <code className="rounded bg-muted px-1 py-0.5 text-foreground">main</code>.
        </p>
        <p className="max-w-3xl text-sm leading-6 text-muted-foreground">
          Everyone who branches from company{" "}
          <code className="rounded bg-muted px-1 py-0.5 text-foreground">main</code> now builds on
          that public form. New contributions must account for those submitted-and-flowed-back
          changes instead of carrying a private copy.
        </p>
        <div className="flex flex-wrap gap-3">
          <Button asChild variant="outline" size="sm">
            <Link to="/playbook">
              Inbound gate in the playbook <ArrowRight />
            </Link>
          </Button>
          <Button asChild variant="outline" size="sm">
            <Link to="/lab">Walk drop-on-merge in the lab</Link>
          </Button>
        </div>
      </section>

      <section className="mt-12 space-y-4">
        <h2 className="text-xl font-semibold">Three repositories, one patch identity</h2>
        <div className="grid gap-3 md:grid-cols-3">
          <RepoCard
            icon={<ShieldCheck className="size-4" />}
            name="Company product repo"
            host="Private forge"
            points={[
              "Synthetic main = upstream + queue",
              "Developers merge PRs here; bot rebuilds main on sync",
              "Merge = internal product; to-upstream environment = IP",
            ]}
          />
          <RepoCard
            icon={<Waypoints className="size-4" />}
            name="Contribution fork"
            host="Public · upstream-owned"
            points={[
              "Created by upstream, not the company",
              "GitHub App or machine user pushes here",
              "The change becomes public when submit lands here",
            ]}
          />
          <RepoCard
            icon={<GitPullRequest className="size-4" />}
            name="Canonical upstream"
            host="Public GitHub.com"
            points={[
              "Maintainers merge ordinary PRs",
              "Uplink-Patch-Id trailer survives squash",
              "Sync flows our merge back and drops the internal copy; foreign commits wait on from-upstream",
            ]}
          />
        </div>
      </section>
    </AppShell>
  );
}

function RepoCard({
  icon,
  name,
  host,
  points,
}: {
  icon: React.ReactNode;
  name: string;
  host: string;
  points: string[];
}) {
  return (
    <div className="rounded-xl border border-border bg-card p-5">
      <div className="mb-3 flex size-8 items-center justify-center rounded-md bg-primary/15 text-primary">
        {icon}
      </div>
      <h3 className="font-medium">{name}</h3>
      <p className="mt-1 text-xs text-muted-foreground">{host}</p>
      <ul className="mt-4 space-y-2 text-sm text-muted-foreground">
        {points.map((point) => (
          <li key={point}>{point}</li>
        ))}
      </ul>
    </div>
  );
}
