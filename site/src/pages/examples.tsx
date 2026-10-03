import { Link, useSearchParams } from "react-router-dom";
import { DocPage } from "../components/doc-page";
import { SetupGuide } from "../components/setup/guide";
import type { Mode } from "../components/setup/step";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Modal } from "../components/ui/modal";
import { Tabs } from "../components/ui/tabs";
import { GITHUB_BLOB } from "../lib/links";
import { renderRepoMarkdown } from "../lib/markdown";
import { EXAMPLE_FIELDS, ExampleSteps, exampleDerive } from "./examples/example-steps";
import story01 from "../../../examples/github/stories/01-solo-fix.md?raw";
import story02 from "../../../examples/github/stories/02-parallel-independent.md?raw";
import story03 from "../../../examples/github/stories/03-stacked-depends-on.md?raw";
import story04 from "../../../examples/github/stories/04-upstream-conflict.md?raw";
import story05 from "../../../examples/github/stories/05-cam-two-deps.md?raw";
import story06 from "../../../examples/github/stories/06-internal-only.md?raw";
import story07 from "../../../examples/github/stories/07-assessment-hook.md?raw";
import exampleWorkflows from "../../../templates/try-it-on-github/README.md?raw";

const STORIES = [
  {
    scenario: "solo-fix",
    file: "01-solo-fix.md",
    markdown: story01,
    title: "01 — Solo fix",
    body: "Asha SHA-256: PR, import, to-upstream submit, upstream merge, drop-on-merge.",
  },
  {
    scenario: "parallel-independent",
    file: "02-parallel-independent.md",
    markdown: story02,
    title: "02 — Parallel independent",
    body: "Asha hash + Ben TTL; merge Ben first; Asha stays queued.",
  },
  {
    scenario: "stacked-depends-on",
    file: "03-stacked-depends-on.md",
    markdown: story03,
    title: "03 — Stacked depends-on",
    body: "Ben log needs Asha; preflight without the trailer; submit order.",
  },
  {
    scenario: "upstream-conflict",
    file: "04-upstream-conflict.md",
    markdown: story04,
    title: "04 — Upstream conflict",
    body: "Sync conflict gated PR, work on -work, merge, rebuild.",
  },
  {
    scenario: "cam-two-deps",
    file: "05-cam-two-deps.md",
    markdown: story05,
    title: "05 — Cam on two siblings",
    body: "Cam depends on Asha and Ben; wait until both merge before submitting Cam.",
  },
  {
    scenario: "internal-only",
    file: "06-internal-only.md",
    markdown: story06,
    title: "06 — Internal-only",
    body: "Telemetry patch never goes through to-upstream / submit.",
  },
  {
    scenario: "assessment-hook",
    file: "07-assessment-hook.md",
    markdown: story07,
    title: "07 — Assessment hook",
    body: "Internal-only assessment hook; extras prepended on Asha’s IP packet.",
  },
];

const VIEWS = ["setup", "stories", "workflows"] as const;
type View = (typeof VIEWS)[number];

const link = "text-primary underline-offset-4 hover:underline";

export function ExamplesPage() {
  const [params, setParams] = useSearchParams();
  const view: View = VIEWS.find((v) => v === params.get("view")) ?? "setup";
  const mode: Mode = params.get("mode") === "gh" ? "gh" : "manual";
  const story = STORIES.find((s) => s.scenario === params.get("story"));

  function update(next: Record<string, string | null>) {
    const merged = new URLSearchParams(params);
    for (const [key, value] of Object.entries(next)) {
      if (value === null) merged.delete(key);
      else merged.set(key, value);
    }
    setParams(merged, { replace: true });
  }

  return (
    <DocPage
      eyebrow="Try it yourself"
      title="Try it on GitHub"
      lead={
        <>
          Run the whole model on three repositories in a new GitHub organization, wired with fine-grained tokens. Set it
          up once, then walk the stories: apply a patch, open a PR, and let Actions import, submit, and sync. The{" "}
          <Link to="/lab" className={link}>
            live lab
          </Link>{" "}
          plays the same stories in the browser.
        </>
      }
    >
      <section className="space-y-3">
        <div className="grid gap-3 md:grid-cols-3">
          <RepoCard name="uplink-example-upstream" role="Public tokenkit project" />
          <RepoCard name="uplink-example-upstream-contrib" role="Fork used as the contribution fork" />
          <RepoCard name="uplink-example-internal" role="Company product (Actions live here)" />
        </div>
      </section>

      <Tabs
        label="Try it yourself"
        value={view}
        onChange={(v) => update({ view: v })}
        items={[
          {
            id: "setup",
            label: "Setup",
            content: (
              <SetupGuide
                fields={EXAMPLE_FIELDS}
                storageKey="uplink-setup-values-example"
                derive={exampleDerive}
                mode={mode}
                onMode={(m) => update({ mode: m })}
                intro={<>From an empty organization to three wired repositories, ready for the first story.</>}
              >
                <ExampleSteps />
              </SetupGuide>
            ),
          },
          {
            id: "stories",
            label: "Stories",
            content: (
              <div className="space-y-4">
                <p className="text-[15px] leading-7 text-muted-foreground">
                  Reset all three repos at the start of each story (<strong className="text-foreground">Actions → Reset
                  example</strong>), then <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">git uplink reset</code>{" "}
                  in the internal clone. What each flow means for developers is in{" "}
                  <Link to="/day-to-day" className={link}>
                    day to day
                  </Link>
                  .
                </p>
                <div className="grid gap-3">
                  {STORIES.map((s) => (
                    <Card key={s.scenario}>
                      <CardHeader>
                        <CardTitle className="text-base">
                          <button
                            type="button"
                            onClick={() => update({ story: s.scenario })}
                            className="text-left hover:text-primary focus-visible:outline-2 focus-visible:outline-primary"
                          >
                            {s.title}
                          </button>
                        </CardTitle>
                      </CardHeader>
                      <CardContent className="space-y-2 text-sm leading-6 text-muted-foreground">
                        <p>{s.body}</p>
                        <p className="flex flex-wrap gap-x-4">
                          <button type="button" onClick={() => update({ story: s.scenario })} className={link}>
                            Read the story
                          </button>
                          <Link to={`/lab?scenario=${s.scenario}`} className={link}>
                            Play it in the lab
                          </Link>
                        </p>
                      </CardContent>
                    </Card>
                  ))}
                </div>
              </div>
            ),
          },
          {
            id: "workflows",
            label: "Workflows",
            content: (
              <div
                className="markdown-body"
                dangerouslySetInnerHTML={{
                  __html: renderRepoMarkdown(exampleWorkflows, "templates/try-it-on-github/README.md"),
                }}
              />
            ),
          },
        ]}
      />

      <Modal
        open={Boolean(story)}
        onClose={() => update({ story: null })}
        title={story?.title ?? ""}
        actions={
          story ? (
            <span className="flex gap-3 text-sm">
              <Link to={`/lab?scenario=${story.scenario}`} className={link}>
                Lab
              </Link>
              <a href={`${GITHUB_BLOB}/examples/github/stories/${story.file}`} className={link}>
                GitHub
              </a>
            </span>
          ) : null
        }
      >
        {story ? (
          <div
            className="markdown-body"
            dangerouslySetInnerHTML={{
              __html: renderRepoMarkdown(story.markdown, `examples/github/stories/${story.file}`),
            }}
          />
        ) : null}
      </Modal>
    </DocPage>
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
