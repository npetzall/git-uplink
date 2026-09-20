import { Link } from "react-router-dom";
import { AppShell } from "../components/app-shell";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { GITHUB_BLOB } from "../lib/links";

const STORIES = [
  {
    scenario: "solo-fix",
    href: `${GITHUB_BLOB}/examples/github/stories/01-solo-fix.md`,
    title: "01 — Solo fix",
    body: "Asha SHA-256: PR, import, to-upstream submit, upstream merge, drop-on-merge.",
  },
  {
    scenario: "parallel-independent",
    href: `${GITHUB_BLOB}/examples/github/stories/02-parallel-independent.md`,
    title: "02 — Parallel independent",
    body: "Asha hash + Ben TTL; merge Ben first; Asha stays queued.",
  },
  {
    scenario: "stacked-depends-on",
    href: `${GITHUB_BLOB}/examples/github/stories/03-stacked-depends-on.md`,
    title: "03 — Stacked depends-on",
    body: "Ben log needs Asha; preflight without the trailer; submit order.",
  },
  {
    scenario: "upstream-conflict",
    href: `${GITHUB_BLOB}/examples/github/stories/04-upstream-conflict.md`,
    title: "04 — Upstream conflict",
    body: "Sync conflict gated PR, work on -work, merge, rebuild.",
  },
  {
    scenario: "cam-two-deps",
    href: `${GITHUB_BLOB}/examples/github/stories/05-cam-two-deps.md`,
    title: "05 — Cam on two siblings",
    body: "Cam depends on Asha and Ben; wait until both merge before submitting Cam.",
  },
  {
    scenario: "internal-only",
    href: `${GITHUB_BLOB}/examples/github/stories/06-internal-only.md`,
    title: "06 — Internal-only",
    body: "Telemetry patch never goes through to-upstream / submit.",
  },
];

export function ExamplesPage() {
  return (
    <AppShell>
      <article className="mx-auto max-w-3xl space-y-10">
        <header className="space-y-3">
          <p className="text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">Examples</p>
          <h1 className="text-4xl font-semibold tracking-tight">Try it on GitHub</h1>
          <p className="text-lg leading-8 text-muted-foreground">
            <code className="rounded bg-muted px-1.5 py-0.5 text-[15px] text-foreground">examples/github/</code> is a
            walkthrough on three repositories. You apply patches, open PRs, and let Actions import, submit, and
            sync.             The{" "}
            <Link to="/lab" className="text-primary underline-offset-4 hover:underline">
              live lab
            </Link>{" "}
            hosts the same stories in the browser, with a Manual vs CI toggle for the commands.
          </p>
        </header>

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">Three repositories</h2>
          <div className="grid gap-3 md:grid-cols-3">
            <RepoCard name="uplink-example-upstream" role="Public tokenkit project" />
            <RepoCard name="uplink-example-upstream-contrib" role="Fork used as the contribution fork" />
            <RepoCard name="uplink-example-internal" role="Company product (Actions live here)" />
          </div>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Company <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">main</code> is always public{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">upstream/main</code> plus every patch that
            is not merged or dropped. After bootstrap, the queue already has one internal-only tooling patch.
          </p>
        </section>

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">Stories</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Reset all three repos at the start of each story (Actions → Reset example), then{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">git uplink reset</code> in the internal
            clone. Rationale for each flow is in the{" "}
            <Link to="/working" className="text-primary underline-offset-4 hover:underline">
              way of working
            </Link>
            .
          </p>
          <div className="grid gap-3">
            {STORIES.map((story) => (
              <Card key={story.title}>
                <CardHeader>
                  <CardTitle className="text-base">
                    <Link
                      to={`/lab?scenario=${story.scenario}`}
                      className="hover:text-primary"
                    >
                      {story.title}
                    </Link>
                  </CardTitle>
                </CardHeader>
                <CardContent className="space-y-2 text-sm leading-6 text-muted-foreground">
                  <p>{story.body}</p>
                  <p>
                    <a href={story.href} className="text-primary underline-offset-4 hover:underline">
                      Story on GitHub
                    </a>
                  </p>
                </CardContent>
              </Card>
            ))}
          </div>
        </section>

        <p className="text-sm text-muted-foreground">
          Setup, tokens, and the <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">to-upstream</code>{" "}
          Environment:{" "}
          <a
            href={`${GITHUB_BLOB}/examples/github/SETUP.md`}
            className="text-primary underline-offset-4 hover:underline"
          >
            examples/github/SETUP.md
          </a>
          . Walkthrough index:{" "}
          <a
            href={`${GITHUB_BLOB}/examples/github/README.md`}
            className="text-primary underline-offset-4 hover:underline"
          >
            examples/github/README.md
          </a>
          .
        </p>
      </article>
    </AppShell>
  );
}

function RepoCard({ name, role }: { name: string; role: string }) {
  return (
    <div className="rounded-xl border border-border bg-card p-5">
      <h3 className="font-mono text-sm font-medium">{name}</h3>
      <p className="mt-2 text-sm text-muted-foreground">{role}</p>
    </div>
  );
}
