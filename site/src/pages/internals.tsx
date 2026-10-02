import type React from "react";
import { DocPage, Section } from "../components/doc-page";
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
    ref: "uplink/amend/<id>",
    where: "Private forge",
    role: "Protected amend base: upstream plus the queue up to and including the patch. The -work branch starts one empty commit ahead so the draft PR opens at once. Merging runs amend --complete; closing aborts.",
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

const INIT_CHART = `
flowchart TD
  start(["git uplink init"]) --> args{"arguments?"}
  args -->|"none"| hydrate["Hydrate: fetch origin uplink/state,<br/>uplink/upstream and main,<br/>re-add remotes from stored URLs"]
  args -->|"--upgrade"| upgrade["Refresh the tooling patch<br/>in its slot"]
  args -->|"--upstream --contrib --forge"| exists{"uplink/state<br/>on origin?"}
  exists -->|"yes"| check["Use it; check the<br/>requested names match"]
  exists -->|"no"| create["Create orphan uplink/state<br/>with an empty queue.json"]
  create --> remotes["Add upstream and contrib remotes"]
  remotes --> seed["Fetch upstream, seed uplink/upstream"]
  seed --> compare{"company main<br/>vs upstream"}
  compare -->|"same"| tooling["Record the forge pack<br/>as the tooling patch"]
  tooling --> rebuild["Rebuild main =<br/>upstream + tooling"]
  compare -->|"ahead"| adopt["Record tooling, then turn each<br/>first-parent commit (or group)<br/>into a queued patch"]
  adopt --> preview["You preview with rebuild --branch,<br/>then publish with rebuild --push"]
  compare -->|"diverged"| refuse["Refuse: bring main<br/>up to date first"]
`;

const PR_CHART = `
sequenceDiagram
  actor Dev as Developer
  participant PR as Internal PR
  participant CI as uplink-pr.yml
  Dev->>PR: open or edit
  par assess
    CI->>CI: git uplink assess
    Note right of CI: message, cutoff, co-author,<br/>keyword and email scan
  and preflight
    CI->>CI: git uplink preflight
    Note right of CI: apply on upstream + declared deps<br/>+ tooling + queued upstream,<br/>run UPLINK_PREFLIGHT
  end
  CI-->>PR: report comment, required checks
`;

const IMPORT_CHART = `
sequenceDiagram
  actor Dev as Developer
  participant Main as company main
  participant CI as uplink-import.yml
  participant State as uplink/state
  Dev->>Main: merge PR
  Main->>CI: push event
  CI->>State: git uplink add (patch = PR base..head)
  alt upstream-bound and internal[] not empty
    CI->>Main: rebuild so the patch sits under internal[]
  end
  CI->>State: git uplink push (restack if origin moved)
`;

const SUBMIT_CHART = `
sequenceDiagram
  actor Op as Operator
  participant Packet as extras + packet jobs
  participant Env as to-upstream
  participant Submit as submit job
  participant Fork as contribution fork
  participant Up as upstream
  Op->>Packet: dispatch Uplink submit (id)
  Packet->>Packet: git uplink report (extras stored at import, else run the hook)
  Packet->>Submit: needs
  Submit->>Env: wait for IP reviewer
  Env-->>Submit: approved
  Submit->>Submit: git uplink approve
  Submit->>Submit: git uplink submit (export commit on uplink/upstream)
  Submit->>Fork: contrib_commit.py (Git Database API, signed by GitHub)
  Submit->>Up: open public PR
  Submit->>Submit: git uplink submitted (record PR on the queue)
`;

const SYNC_CHART = `
sequenceDiagram
  participant Up as upstream
  participant Inspect as inspect job
  participant Env as from-upstream
  participant Apply as apply job
  participant Main as company main
  Up->>Inspect: git uplink sync (fetch, do not move uplink/upstream)
  alt every new commit is a flow-back of our patch
    Inspect->>Main: promote, mark merged, rebuild
  else any foreign commit
    Inspect->>Inspect: write incoming.md packet
    Apply->>Env: wait for inbound reviewer
    Env-->>Apply: approved
    Apply->>Main: git uplink accept-upstream (promote, detect merges, rebuild)
  end
`;

const CONFLICT_CHART = `
sequenceDiagram
  participant Sync as sync or resolve job
  participant Base as uplink/conflict/id
  participant Work as uplink/conflict/id-work
  actor Owner as Patch owner
  participant Resolve as uplink-resolve.yml
  Sync->>Sync: rebuild stops on id, status conflict
  Sync->>Base: push protected base at the apply prefix
  Sync->>Work: push work branch with conflict markers
  Sync->>Sync: print gh.prCreate JSON, exit 0
  Sync->>Base: gh pr create, then git uplink gated
  Owner->>Work: fix and push
  Owner->>Base: merge the gated PR
  Base->>Resolve: pull_request closed (merged)
  Resolve->>Resolve: git uplink resolve id (same id, rebuild)
  opt already submitted
    Resolve->>Resolve: status amended, dispatch submit for a delta
  end
`;

const GATE_CHART = `
flowchart LR
  a["Job 1<br/>command A"] -->|"packet or JSON,<br/>exit 0"| review{{"Human review<br/>Environment or PR"}}
  review -->|"approve / merge"| b["Job 2<br/>command B"]
  review -->|"reject / close"| stop(["nothing changes"])
`;

const GATES = [
  ["Contribution (IP)", "git uplink report", "to-upstream Environment", "git uplink approve + submit"],
  ["Inbound upstream", "git uplink sync", "from-upstream Environment", "git uplink accept-upstream"],
  ["Conflict", "git uplink sync / resolve", "gated PR into uplink/conflict/<id>", "git uplink resolve"],
  ["Transfer", "git uplink transfer", "transfer PR", "git uplink transfer --complete"],
  ["Amend", "git uplink amend", "draft PR into uplink/amend/<id>", "git uplink amend --complete"],
  ["Withdraw contribution", "git uplink transfer --to-internal", "abandon-contrib Environment", "close public PR, delete fork branch"],
];

function Step({ n, title, workflow, children }: { n: number; title: string; workflow: string; children: React.ReactNode }) {
  return (
    <div className="space-y-3">
      <h3 className="text-base font-semibold text-foreground">
        {n}. {title} <span className="ms-2 font-mono text-xs font-normal text-muted-foreground">{workflow}</span>
      </h3>
      {children}
    </div>
  );
}

export function InternalsPage() {
  return (
    <DocPage
      eyebrow="Internals"
      title="From init to sync, step by step"
      lead={
        <>
          The engine is forge-agnostic and every command is a plain CLI call. CI workflows are thin wrappers that
          run those commands and put a human review between them.
        </>
      }
    >
      <Section title="What init does">
        <p>
          <code>git uplink init --upstream &lt;url&gt; --contrib &lt;url&gt; --forge ghec</code> runs once per
          product repository. Every CI job then runs a bare <code>git uplink init</code> to hydrate its checkout.
        </p>
        <MermaidDiagram chart={INIT_CHART} title="init: create, adopt, hydrate, or upgrade" />
        <ul>
          <li>
            The queue lives on the orphan branch <code>uplink/state</code> as <code>.uplink/queue.json</code> and{" "}
            <code>.uplink/patches/*.patch</code>, with remote URLs, branch names, and the forge.
          </li>
          <li>
            The forge pack (workflows and the PR template) is itself a patch, the <strong>tooling</strong> slot,
            applied right after upstream.
          </li>
          <li>
            <code>uplink/upstream</code> is the last accepted public <code>main</code>. It only moves through sync.
          </li>
        </ul>
      </Section>

      <Section title="From pull request to upstream and back">
        <Step n={1} title="Pull request opened" workflow="uplink-pr.yml">
          <MermaidDiagram chart={PR_CHART} title="Assess and preflight run in parallel on every upstream-bound PR" />
        </Step>
        <Step n={2} title="Merge: the product gate" workflow="uplink-import.yml">
          <MermaidDiagram chart={IMPORT_CHART} title="Import records the merged change as a queued patch" />
          <p>
            The merge already put the change on <code>main</code>. Import only records it. An internal-only
            import, or an upstream import with nothing in <code>internal[]</code>, does not rebuild.
          </p>
        </Step>
        <Step n={3} title="Submit: the IP gate" workflow="uplink-submit.yml">
          <MermaidDiagram chart={SUBMIT_CHART} title="Packet, wait for to-upstream, then approve and submit in the same run" />
          <p>
            The packet (<code>.uplink/reports/&lt;id&gt;/assessment.md</code>) is committed to{" "}
            <code>uplink/state</code> before the wait, so reviewers and auditors read the same bytes. Fork-write
            credentials exist only in the job after the Environment approval.
          </p>
        </Step>
        <Step n={4} title="Upstream merges" workflow="maintainers">
          <p>
            Maintainers merge the public PR however they like. The exported commit carries an{" "}
            <code>Uplink-Patch-Id</code> trailer, and the PR number is on the queue, so squash and rebase merges
            are still recognized.
          </p>
        </Step>
        <Step n={5} title="Sync: the inbound gate" workflow="uplink-sync.yml">
          <MermaidDiagram chart={SYNC_CHART} title="Flow-back applies at once; foreign commits wait for from-upstream" />
          <p>
            Merge detection runs in this order: recorded PR, then trailer, then <code>git patch-id --stable</code>,
            then empty apply. Merged patches are sticky: never applied again.
          </p>
        </Step>
        <Step n={6} title="When a patch no longer applies" workflow="uplink-sync.yml → uplink-resolve.yml">
          <MermaidDiagram chart={CONFLICT_CHART} title="Conflicts become a gated PR; merging it runs resolve" />
          <p>
            A blocked patch blocks the rest of the rebuild. There is no skip, so company <code>main</code> stays at
            the last good rebuild until the gated PR merges.
          </p>
        </Step>
      </Section>

      <Section title="How gates work">
        <p>
          Every gate has the same shape: <strong>command A</strong> prepares something and stops,{" "}
          <strong>a human reviews</strong>, and <strong>command B</strong> acts on it. In CI these are separate
          jobs, split exactly at the review: a GitHub Environment approval or a pull request merge.
        </p>
        <MermaidDiagram chart={GATE_CHART} title="Command A, review, command B" />
        <div className="overflow-x-auto">
          <table className="w-full min-w-[640px] text-left text-sm">
            <thead className="text-xs tracking-wide uppercase">
              <tr className="border-b">
                <th className="py-2 pr-3 font-medium">Gate</th>
                <th className="py-2 pr-3 font-medium">Command A</th>
                <th className="py-2 pr-3 font-medium">Review</th>
                <th className="py-2 font-medium">Command B</th>
              </tr>
            </thead>
            <tbody>
              {GATES.map(([gate, a, review, b]) => (
                <tr key={gate} className="border-b border-border/60 align-top">
                  <td className="py-2 pr-3 text-foreground">{gate}</td>
                  <td className="py-2 pr-3 font-mono text-xs">{a}</td>
                  <td className="py-2 pr-3">{review}</td>
                  <td className="py-2 font-mono text-xs">{b}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <p>
          That is why command A prints JSON (<code>gh.prCreate</code>, <code>gh.prClose</code>) and exits 0 when it
          opens a gate: the workflow reads the JSON to open the PR or dispatch the next job. A gate is a normal
          outcome, not a failure. Locally, you can run A and B yourself. Nothing in the engine requires CI.
        </p>
      </Section>

      <Section title="Branches">
        <MermaidDiagram chart={BRANCH_CHART} title="Where each ref lives, and which writes are derived" />
        <div className="overflow-x-auto">
          <table className="w-full min-w-[640px] text-left text-sm">
            <thead className="text-xs tracking-wide uppercase">
              <tr className="border-b">
                <th className="py-2 pr-3 font-medium">Ref</th>
                <th className="py-2 pr-3 font-medium">Where</th>
                <th className="py-2 font-medium">Role</th>
              </tr>
            </thead>
            <tbody>
              {BRANCHES.map((row) => (
                <tr key={row.ref} className="border-b border-border/60 align-top">
                  <td className="py-2 pr-3 font-mono text-xs whitespace-nowrap text-foreground">{row.ref}</td>
                  <td className="py-2 pr-3 whitespace-nowrap">{row.where}</td>
                  <td className="py-2">{row.role}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Section>

      <Section title="Rebuild">
        <p>
          Rebuild rewrites company <code>main</code> as a clean replay of accepted upstream plus every{" "}
          <strong>active</strong> patch (not <code>merged</code> / <code>dropped</code>). Apply order is tooling,
          then <code>upstream[]</code>, then <code>internal[]</code>, each topo-sorted by <code>dependsOn</code>.
        </p>
        <ol>
          <li>
            Snapshot <code>.uplink/</code> from <code>uplink/state</code>.
          </li>
          <li>
            Detach at <code>uplink/upstream</code> (or company <code>main</code> if that ref is missing).
          </li>
          <li>
            <code>git apply</code> each active patch. Empty apply of an upstream-bound patch marks it{" "}
            <code>merged</code>. A conflict stops the replay: <code>main</code> stays at the last good rebuild and
            the conflict branches are published.
          </li>
          <li>
            Force company <code>main</code> to that HEAD, restore <code>.uplink/</code> from the snapshot, and
            commit queue status.
          </li>
        </ol>
        <p>
          <code>git uplink rebuild --branch uplink/preview/verify</code> is preview only: it does not move{" "}
          <code>main</code> and does not change the queue.
        </p>
        <MermaidDiagram chart={REBUILD_CHART} title="rebuild_once: snapshot, replay, restore" />
      </Section>

      <Section title="Concurrency">
        <p>
          Two PRs can merge at the same time. They must not both rewrite <code>queue.json</code> from a stale
          checkout, or the later push would drop the earlier patch. Three layers prevent that:
        </p>
        <ul>
          <li>
            GitHub Actions concurrency group <code>uplink-mutate</code> on every job that writes the queue. Later
            jobs queue instead of being cancelled. Jobs waiting on an Environment do <strong>not</strong> hold the
            group, so a pending IP or inbound review never freezes imports.
          </li>
          <li>
            A process lock in <code>.git/uplink.lock</code>, so two CLI processes in one checkout cannot interleave.
          </li>
          <li>
            Restack on push: if origin moved, local-only patches are replayed onto the remote queue and the push is
            retried (up to eight times, with backoff). Re-importing the same PR number is a no-op.
          </li>
        </ul>
        <MermaidDiagram chart={RESTACK_CHART} title="Two concurrent imports: lease reject, restack, retry" />
      </Section>
    </DocPage>
  );
}
