import { Link } from "react-router-dom";
import { AppShell } from "../components/app-shell";
import { MermaidDiagram } from "../components/mermaid-diagram";

const BRANCH_CHART = `
flowchart TB
  subgraph privateForge [Private forge]
    state["uplink/state"]
    main["company main"]
    upstreamRef["uplink/upstream"]
    conflict["uplink/conflict/id"]
    work["uplink/conflict/id-work"]
    feat["feat branches"]
  end
  subgraph contribFork [Contribution fork public]
    forkBranch["uplink/id"]
  end
  subgraph canonical [Canonical upstream]
    upMain["main"]
  end
  state -->|"source of truth"| main
  upstreamRef -->|"rebuild base"| main
  feat -->|"merge PR"| main
  work -->|"gated PR"| conflict
  conflict -->|"resolve amends patch"| state
  state -->|"submit"| forkBranch
  forkBranch -->|"maintainer merge"| upMain
  upMain -->|"sync drop-on-merge"| upstreamRef
`;

const REBUILD_CHART = `
sequenceDiagram
  participant State as uplink_state
  participant Upstream as uplink_upstream
  participant Main as company_main
  State->>State: snapshot .uplink
  Main->>Upstream: detach at uplink/upstream
  loop active patches tooling then upstream then internal
    Upstream->>Upstream: git apply patch
    alt conflict
      Upstream-->>Main: stop do not move main
      Upstream->>State: mark conflict publish uplink/conflict/id plus work
    else empty upstream-bound
      Upstream->>State: mark merged
    else applied
      Upstream->>Upstream: last_good equals HEAD
    end
  end
  Upstream->>Main: force company main to last_good
  State->>Main: restore .uplink from snapshot
`;

const RESTACK_CHART = `
sequenceDiagram
  participant ImportA as Import_A
  participant Origin as origin_uplink_state
  participant ImportB as Import_B
  ImportA->>Origin: add upl_a and push
  ImportB->>ImportB: add upl_b on stale state
  ImportB->>Origin: push lease rejected
  Origin-->>ImportB: remote tip has upl_a
  ImportB->>ImportB: restack copy upl_b onto remote queue
  ImportB->>Origin: push retried
`;

const BRANCHES = [
  {
    ref: "uplink/state",
    where: "Private forge",
    role: "Orphan branch: .uplink/queue.json and patch files. Queue history. Source of truth.",
  },
  {
    ref: "company main",
    where: "Private forge",
    role: "Product tree: upstream/main + tooling + active upstream[] + active internal[]. Humans merge PRs; bot may rewrite on rebuild.",
  },
  {
    ref: "uplink/upstream",
    where: "Private forge",
    role: "Frozen pointer at last accepted public main. Rebuild base. Never a place to work.",
  },
  {
    ref: "uplink/conflict/<id>",
    where: "Private forge",
    role: "Protected conflict base at the apply prefix. Humans do not push it. Merge from -work runs resolve.",
  },
  {
    ref: "uplink/conflict/<id>-work",
    where: "Private forge",
    role: "Unprotected work branch. Humans fix files here and open a PR into the base.",
  },
  {
    ref: "uplink/transfer-to-*/<id>",
    where: "Private forge",
    role: "Protected transfer bases (to-upstream / to-internal). Not recorded on the queue until the move succeeds. Close the PR without merging to abort.",
  },
  {
    ref: "feat/*",
    where: "Private forge",
    role: "Ordinary internal PR branches. One branch per change.",
  },
  {
    ref: "uplink/<id>",
    where: "Public contribution fork",
    role: "Bot-generated, may be force-pushed. First public bytes of that change.",
  },
  {
    ref: "upstream main",
    where: "Canonical public",
    role: "Maintainers merge. Sync drops the internal copy.",
  },
];

export function InternalsPage() {
  return (
    <AppShell>
      <article className="mx-auto max-w-3xl space-y-10">
        <header className="space-y-3">
          <p className="text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">Internals</p>
          <h1 className="text-4xl font-semibold tracking-tight">Branches, rebuild, and restack</h1>
          <p className="text-lg leading-8 text-muted-foreground">
            The engine is forge-agnostic. Unreleased work lives on the private forge.{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-[15px] text-foreground">submit</code> is
            the first time a change is public, because that is when the bot pushes{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-[15px] text-foreground">uplink/&lt;id&gt;</code>{" "}
            to the contribution fork. Every git ref except the patch object is derived.
          </p>
        </header>

        <section className="space-y-4 text-[15px] leading-7 text-muted-foreground [&_code]:rounded [&_code]:bg-muted [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:text-[13px] [&_code]:text-foreground">
          <h2 className="text-xl font-semibold text-foreground">Branches</h2>
          <MermaidDiagram chart={BRANCH_CHART} title="Where each ref lives, and which writes are derived" />
          <div className="overflow-x-auto">
            <table className="w-full min-w-[640px] text-left text-sm">
              <thead className="text-xs tracking-wide text-muted-foreground uppercase">
                <tr className="border-b">
                  <th className="py-2 pr-3 font-medium">Ref</th>
                  <th className="py-2 pr-3 font-medium">Where</th>
                  <th className="py-2 font-medium">Role</th>
                </tr>
              </thead>
              <tbody>
                {BRANCHES.map((row) => (
                  <tr key={row.ref} className="border-b border-border/60 align-top">
                    <td className="py-2 pr-3 font-mono text-xs text-foreground whitespace-nowrap">{row.ref}</td>
                    <td className="py-2 pr-3 whitespace-nowrap">{row.where}</td>
                    <td className="py-2 text-muted-foreground">{row.role}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p>
            Never base product work on <code>uplink/state</code>, <code>uplink/upstream</code>, contrib{" "}
            <code>uplink/&lt;id&gt;</code>, or a protected uplink base. Work on{" "}
            <code>uplink/conflict/&lt;id&gt;-work</code> (or a transfer <code>-work</code>) only to
            finish that gated PR. Branch new product work from latest company <code>main</code>.
          </p>
        </section>

        <section className="space-y-4 text-[15px] leading-7 text-muted-foreground [&_code]:rounded [&_code]:bg-muted [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:text-[13px] [&_code]:text-foreground">
          <h2 className="text-xl font-semibold text-foreground">Rebuild</h2>
          <p>
            Rebuild rewrites company <code>main</code> as a clean replay of accepted upstream plus
            every <strong className="text-foreground">active</strong> patch (not{" "}
            <code>merged</code> / <code>dropped</code>). Apply order is tooling, then{" "}
            <code>upstream[]</code>, then <code>internal[]</code>, each topo-sorted by{" "}
            <code>dependsOn</code>.
          </p>
          <ol className="list-decimal space-y-2 ps-5">
            <li>
              Snapshot <code>.uplink/</code> from <code>uplink/state</code>.
            </li>
            <li>
              Detach at <code>uplink/upstream</code> (or company <code>main</code> if that ref is
              missing).
            </li>
            <li>
              <code>git apply</code> each active patch. Empty apply of an upstream-bound patch marks
              it <code>merged</code>. Conflict stops the replay: later patches are not skipped;
              product <code>main</code> stays at the last good rebuild; the bot publishes{" "}
              <code>uplink/conflict/&lt;id&gt;</code> plus <code>-work</code>.
            </li>
            <li>
              Force company <code>main</code> to that HEAD, restore <code>.uplink/</code> from the
              snapshot, commit queue status.
            </li>
          </ol>
          <p>
            <code>git uplink rebuild --branch uplink/verify</code> is preview only: it does not move{" "}
            <code>main</code> and does not mutate the queue. Inspect with{" "}
            <code>git diff main uplink/verify</code>, then <code>git uplink rebuild --push</code> to
            publish.
          </p>
          <MermaidDiagram chart={REBUILD_CHART} title="rebuild_once: snapshot, replay, restore" />
        </section>

        <section className="space-y-4 text-[15px] leading-7 text-muted-foreground [&_code]:rounded [&_code]:bg-muted [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:text-[13px] [&_code]:text-foreground">
          <h2 className="text-xl font-semibold text-foreground">Add with restack retry</h2>
          <p>
            <code>git uplink add</code> records a patch on local <code>uplink/state</code>. It does
            not publish. Workflows (and operators) then call <code>git uplink push</code>.
          </p>
          <ol className="list-decimal space-y-2 ps-5">
            <li>
              Isolate the product diff <code>from..head</code> (excluding <code>.uplink/</code>),
              write <code>.uplink/patches/&lt;id&gt;.patch</code>, append to the local queue.
            </li>
            <li>
              Upstream-bound: rebuild so the new patch sits under any <code>internal[]</code>.
              Internal-only: add-only, no rebuild.
            </li>
            <li>
              <code>git uplink push</code> publishes <code>uplink/state</code>. Up to eight attempts,
              with exponential backoff, if the push lease is rejected.
            </li>
            <li>
              If origin moved: restack — reset local <code>uplink/state</code> to the remote tip,
              copy local-only patch files, append those patches to the remote queue, commit{" "}
              <code>uplink: restack onto origin</code>, push again. The same internal PR number is a
              no-op.
            </li>
          </ol>
          <MermaidDiagram
            chart={RESTACK_CHART}
            title="Two concurrent imports: lease reject, restack, retry"
          />
        </section>

        <p className="text-sm text-muted-foreground">
          Operating model and gates:{" "}
          <Link to="/playbook" className="text-primary underline-offset-4 hover:underline">
            playbook
          </Link>
          . Day-to-day stories:{" "}
          <Link to="/working" className="text-primary underline-offset-4 hover:underline">
            way of working
          </Link>
          .
        </p>
      </article>
    </AppShell>
  );
}
