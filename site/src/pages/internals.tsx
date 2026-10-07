import type React from "react";
import { Link } from "react-router-dom";
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
    ref: "uplink/hooks",
    where: "Private forge",
    role: "Orphan branch, company-only: uplink.toml, preflight.sh, the toolchain hook and the assessment hook. Never queued, replayed, or contributed. Changed through a reviewed pull request.",
  },
  {
    ref: "uplink/preview/*",
    where: "Local",
    role: "Where rebuild --branch writes a preview. Does not move main or change the queue.",
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
  args -->|"none"| hydrate["Hydrate: fetch origin uplink/state,<br/>uplink/upstream and main,<br/>re-add remotes from stored URLs.<br/>Stops if origin has no uplink/upstream"]
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
    Note right of CI: apply on upstream + declared deps<br/>+ tooling + queued upstream,<br/>run preflight.sh (step with no token)
  end
  CI-->>PR: report comment, required checks
`;

const IMPORT_CHART = `
sequenceDiagram
  actor Dev as Developer
  participant Main as company main
  participant Pre as preflight job
  participant CI as import job
  participant State as uplink/state
  Dev->>Main: merge PR
  Main->>Pre: pull request merged
  Pre->>Pre: git uplink preflight --json (preflight.sh, read-only token)
  Pre->>CI: result
  CI->>State: git uplink add --preflight-result --gate (patch = PR base..head)
  alt passes, or labelled internal-only
    opt upstream-bound and internal[] not empty
      CI->>Main: rebuild so the patch sits under internal[]
    end
  else upstream-bound, fails assess or preflight
    CI->>State: recorded internal-only
    CI->>Main: push uplink/transfer-to-upstream/id plus -work, open transfer PR
  end
  CI->>State: git uplink push (restack if origin moved)
`;

const SUBMIT_CHART = `
sequenceDiagram
  actor Op as Operator
  participant Packet as assess + packet jobs
  participant Pre as preflight job
  participant Env as to-upstream
  participant Submit as submit job
  participant Fork as contribution fork
  participant Up as upstream
  Op->>Packet: dispatch Uplink submit (id)
  Packet->>Packet: git uplink assess --package, run the hook on it, then git uplink report
  Packet->>Submit: needs
  Op->>Pre: same dispatch
  Pre->>Pre: git uplink preflight id --json (preflight.sh, read-only token)
  Pre->>Submit: result
  Submit->>Env: wait for IP reviewer
  Env-->>Submit: approved
  Submit->>Submit: git uplink approve
  Submit->>Submit: git uplink submit --preflight-result (export commit on uplink/upstream)
  Submit->>Fork: contrib_commit.py (Git Database API, signed by GitHub)
  Submit->>Up: open public PR
  Submit->>Submit: git uplink submitted (record PR on the queue)
`;

const SYNC_CHART = `
sequenceDiagram
  participant Up as upstream
  participant Inspect as inspect job
  participant Env as from-upstream
  participant Pre as preflight job
  participant Apply as apply job
  participant Main as company main
  Inspect->>Up: which recorded public PRs are merged?
  Up->>Inspect: git uplink sync --merged-pr (fetch, do not move uplink/upstream)
  alt our merged patches explain every new commit
    Inspect->>Main: promote, mark merged, rebuild
  else anything else
    Inspect->>Inspect: write incoming.md packet
    Inspect->>Env: wait for inbound reviewer
    Env-->>Pre: approved
    Pre->>Up: accept-upstream --fetch-only (upstream token, this step only)
    Pre->>Pre: accept-upstream --preflight-only (preflight.sh, no token)
    Pre->>Apply: result
    Apply->>Main: git uplink accept-upstream --sha (promote, mark merged, rebuild)
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
    Resolve->>Resolve: dispatch submit (amended and a delta only if added or removed lines changed)
  end
`;

const TRANSFER_CHART = `
sequenceDiagram
  actor Op as Operator
  participant Pre as preflight job
  participant Job as transfer job
  participant Base as uplink/transfer-to-*/id
  actor Owner as Patch owner
  participant State as uplink/state
  Op->>Pre: dispatch Uplink transfer (id, direction)
  Pre->>Pre: git uplink transfer --preflight-only (read-only token)
  Pre->>Job: result
  Job->>Job: git uplink transfer
  alt applies, passes assess (to-upstream) and preflight
    Job->>State: move the patch, rebuild main
  else needs changes
    Job->>Base: push protected base plus -work, open transfer PR
    Owner->>Base: fix on -work, Uplink gate passes, merge
    Base->>Pre: pull request closed (merged)
    Pre->>Pre: git uplink transfer --complete --preflight-only
    Pre->>Job: result
    Job->>State: git uplink transfer --complete, rebuild main
  end
  opt to-internal of a submitted patch
    Job->>Job: dispatch Uplink abandon contrib
  end
`;

const AMEND_CHART = `
sequenceDiagram
  actor Op as Operator
  participant Job as amend job
  participant Base as uplink/amend/id
  actor Owner as Patch owner
  participant Pre as preflight job
  participant State as uplink/state
  Op->>Job: dispatch Uplink amend (id)
  Job->>Job: git uplink amend (replay the queue up to and including the patch)
  Job->>Base: push protected base plus -work, open draft PR
  Owner->>Base: push to -work, edit title and description
  Owner->>Base: Uplink gate passes, mark ready, merge
  Base->>Pre: pull request closed (merged)
  Pre->>Pre: git uplink amend --complete --preflight-only (read-only token)
  Pre->>Job: result
  Job->>State: git uplink amend --complete (same id), rebuild main
  opt already submitted
    Job->>Job: dispatch Uplink submit (delta for IP, or export again)
  end
`;

const ABANDON_CHART = `
sequenceDiagram
  participant Transfer as transfer job
  participant Job as abandon job
  participant Env as abandon-contrib
  participant Up as upstream
  participant Fork as contribution fork
  Transfer->>Job: dispatch Uplink abandon contrib (id), do not wait
  Job->>Env: wait for reviewer
  Env-->>Job: approved
  Job->>Up: close the public PR (upstream credential)
  Job->>Fork: delete uplink/id (contrib credential)
`;

const VERIFY_CHART = `
sequenceDiagram
  actor Op as Operator
  participant Pre as preflight job
  participant Job as verify job
  participant Base as uplink/conflict/id
  Op->>Pre: dispatch Uplink verify
  Pre->>Pre: git uplink rebuild --verify --preflight-only (read-only token)
  Note right of Pre: preflight.sh on the rebuilt queue,<br/>git bisect when it fails
  Pre->>Job: result
  Job->>Job: git uplink rebuild --verify --json
  alt a patch fails
    Job->>Base: mark conflict, push base plus -work, open gated PR
  else passes
    Job->>Job: nothing changes
  end
`;

const REBASE_CHART = `
sequenceDiagram
  participant Checks as uplink-pr.yml
  participant PR as Internal PR
  participant Job as uplink-rebase.yml
  Checks->>Checks: git uplink rebase --plan (head fetched, never checked out)
  Checks-->>PR: comment with the rebase command
  alt rebase label, or UPLINK_AUTO_REBASE
    Checks->>Job: dispatch with the PR number only
    Job->>Job: git uplink rebase --plan, then rebase in a worktree, hooks off
    Job->>PR: push --force-with-lease on the head it read
  else neither
    PR->>PR: developer runs git uplink rebase and pushes
  end
`;

const FLOWS = [
  ["Pull request checks", "PR to main opened or changed", "uplink-pr.yml", "none (required checks)"],
  ["Import", "PR merged into main", "uplink-import.yml", "the merge itself"],
  ["Submit", "dispatch, by hand or by the pack", "uplink-submit.yml", "to-upstream"],
  ["Sync", "hourly, or dispatch", "uplink-sync.yml", "from-upstream, for foreign commits"],
  ["Resolve", "conflict PR merged", "uplink-resolve.yml", "the gated PR"],
  ["Transfer", "dispatch, or a failed import", "uplink-transfer.yml", "transfer PR, when changes are needed"],
  ["Amend", "dispatch", "uplink-amend.yml", "the draft PR"],
  ["Withdraw", "transfer to-internal of a submitted patch", "uplink-abandon.yml", "abandon-contrib"],
  ["Verify", "dispatch", "uplink-verify.yml", "none; a failing patch gets a conflict PR"],
  ["Rebase", "rebuild of main, label uplink:rebase, or dispatch", "uplink-rebase.yml", "none"],
  ["Gate check", "PR into a gated uplink/ base", "uplink-gate.yml", "required check on that PR"],
];

const GATE_CHECKS = [
  ["uplink/conflict/<id>", "upstream", "Assess the resolution with the patch's stored message"],
  ["uplink/amend/<id>", "upstream", "Assess the whole amended patch with the PR title and description"],
  ["uplink/transfer-to-upstream/<id>", "internal, moving", "Export preflight with the PR title and description as the message"],
  ["uplink/transfer-to-internal/<id>", "upstream, moving", "preflight.sh only"],
  ["uplink/conflict/<id>, uplink/amend/<id>", "internal", "preflight.sh only"],
];

const GATE_CHART = `
flowchart LR
  a["Job 1<br/>command A"] -->|"packet or JSON,<br/>exit 0"| review{{"Human review<br/>Environment or PR"}}
  review -->|"approve / merge"| b["Job 2<br/>command B"]
  review -->|"reject / close"| stop(["nothing changes"])
`;

const GATES = [
  ["Contribution (IP)", "git uplink report", "to-upstream Environment", "git uplink approve + submit"],
  ["Inbound upstream", "git uplink sync", "from-upstream Environment", "git uplink accept-upstream"],
  ["Conflict", "git uplink sync / resolve / rebuild --verify", "gated PR into uplink/conflict/<id>", "git uplink resolve"],
  ["Transfer", "git uplink transfer / add --gate", "transfer PR", "git uplink transfer --complete"],
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

function FlowTable() {
  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[640px] text-left text-sm">
        <thead className="text-xs tracking-wide uppercase">
          <tr className="border-b">
            <th className="py-2 pr-3 font-medium">Flow</th>
            <th className="py-2 pr-3 font-medium">Starts on</th>
            <th className="py-2 pr-3 font-medium">Workflow</th>
            <th className="py-2 font-medium">Human gate</th>
          </tr>
        </thead>
        <tbody>
          {FLOWS.map(([flow, trigger, workflow, gate]) => (
            <tr key={flow} className="border-b border-border/60 align-top">
              <td className="py-2 pr-3 whitespace-nowrap text-foreground">{flow}</td>
              <td className="py-2 pr-3">{trigger}</td>
              <td className="py-2 pr-3 font-mono text-xs whitespace-nowrap">{workflow}</td>
              <td className="py-2">{gate}</td>
            </tr>
          ))}
        </tbody>
      </table>
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
      <Section title="Every flow">
        <FlowTable />
        <p>
          The first five are drawn in <a href="#lifecycle">From pull request to upstream and back</a>, the rest in{" "}
          <a href="#other-flows">Other flows</a>. Which credential each job holds is on{" "}
          <Link to="/security">Security</Link>.
        </p>
      </Section>

      <Section title="What init does">
        <p>
          <code>git uplink init --upstream &lt;url&gt; --contrib &lt;url&gt; --forge github</code> runs once per
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

      <Section id="lifecycle" title="From pull request to upstream and back">
        <Step n={1} title="Pull request opened" workflow="uplink-pr.yml">
          <MermaidDiagram chart={PR_CHART} title="Assess and preflight run in parallel on every upstream-bound PR" />
        </Step>
        <Step n={2} title="Merge: the product gate" workflow="uplink-import.yml">
          <MermaidDiagram chart={IMPORT_CHART} title="Import records the merged change as a queued patch" />
          <p>
            The merge already put the change on <code>main</code>. Import only records it. An internal-only
            import, or an upstream import with nothing in <code>internal[]</code>, does not rebuild.
          </p>
          <p>
            A change that fails is still recorded: it is on <code>main</code> already, and leaving it out of the
            queue would drop it at the next rebuild. An upstream-bound change that fails its assessment or its
            export preflight is recorded internal-only, and a transfer to upstream is opened for it. Merge that
            PR to move it to the upstream queue, or close it to keep it internal-only.
          </p>
        </Step>
        <Step n={3} title="Submit: the IP gate" workflow="uplink-submit.yml">
          <MermaidDiagram chart={SUBMIT_CHART} title="Packet, wait for to-upstream, then approve and submit in the same run" />
          <p>
            The packet (<code>.uplink/reports/&lt;id&gt;/assessment.md</code>) is committed to{" "}
            <code>uplink/state</code> before the wait, so reviewers and auditors read the same bytes. Fork-write
            credentials exist only in the job after the Environment approval.
          </p>
          <p>
            <code>preflight.sh</code> builds and runs product code, so it never runs in a job that can write. Import,
            submit, amend, transfer, resolve, verify and the apply of an approved upstream each run it in a job of
            its own with a read-only token, and the writing job takes the result with{" "}
            <code>--preflight-result</code>. The result names the tree and the hooks it was
            tested with. If <code>uplink/upstream</code> or <code>uplink/hooks</code> moved while submit waited for
            approval, submit stops; dispatch it again.
          </p>
        </Step>
        <Step n={4} title="Upstream merges" workflow="maintainers">
          <p>
            Maintainers merge the public PR however they like. The PR number is on the queue, so a squash, a
            rebase, or a merge the maintainer edited is still recognized. The exported commit also carries an{" "}
            <code>Uplink-Patch-Id</code> trailer, which labels the commit but proves nothing by itself.
          </p>
        </Step>
        <Step n={5} title="Sync: the inbound gate" workflow="uplink-sync.yml">
          <MermaidDiagram chart={SYNC_CHART} title="Flow-back applies at once; foreign commits wait for from-upstream" />
          <p>
            A patch is merged when an upstream commit has its <code>git patch-id --stable</code>, or when its
            recorded public PR is merged. Patch ids are public, so a trailer on any other diff is only a claim:
            the packet lists it and the reviewer decides. The rebuild also marks an upstream-bound patch that
            applies empty. Merged patches are sticky: never applied again.
          </p>
        </Step>
        <Step n={6} title="When a patch no longer applies" workflow="uplink-sync.yml → uplink-resolve.yml">
          <MermaidDiagram chart={CONFLICT_CHART} title="Conflicts become a gated PR; merging it runs resolve" />
          <p>
            A blocked patch blocks the rest of the rebuild. There is no skip, so company <code>main</code> stays at
            the last good rebuild until the gated PR merges.
          </p>
          <p>
            A rebuild where every patch applies is then tested with <code>preflight.sh</code>, unless{" "}
            <code>main</code> already has that tree. <code>uplink/upstream</code> is the known good commit (an
            approved upstream is only promoted when the script passes on it) and the rebuilt tree the known bad
            one, so <code>git bisect run</code> finds the first patch the script fails on. It is blocked the same
            way: the base is the queue before it, and <code>-work</code> has the patch applied.
          </p>
        </Step>
      </Section>

      <Section id="other-flows" title="Other flows">
        <Step n={7} title="Transfer between queues" workflow="uplink-transfer.yml">
          <MermaidDiagram chart={TRANSFER_CHART} title="A transfer moves at once, or through a gated PR when the patch has to change" />
          <p>
            Closing the transfer PR without merging deletes both branches and leaves the queue unchanged. A
            failed import enters this flow at the transfer PR.
          </p>
        </Step>
        <Step n={8} title="Amend a patch" workflow="uplink-amend.yml">
          <MermaidDiagram chart={AMEND_CHART} title="Amend replays the queue up to the patch and takes the change through a draft PR" />
          <p>
            The patch keeps its id. Closing the PR without merging cancels. An amend of a submitted patch goes
            back to IP only when it changes the lines the patch adds or removes, or its public title or message.
          </p>
        </Step>
        <Step n={9} title="Withdraw a contribution" workflow="uplink-abandon.yml">
          <MermaidDiagram chart={ABANDON_CHART} title="Withdrawing waits on abandon-contrib, then closes the public PR and deletes the fork branch" />
          <p>
            The queue already says internal-only while this run waits. The public PR and{" "}
            <code>uplink/&lt;id&gt;</code> on the fork remain until it is approved.
          </p>
        </Step>
        <Step n={10} title="Verify main" workflow="uplink-verify.yml">
          <MermaidDiagram chart={VERIFY_CHART} title="Verify tests the rebuilt queue although main already has that tree" />
          <p>
            For a <code>main</code> that turned out broken when no rebuild noticed: a rebuild is preflighted only
            when it changes the tree, and the rebuild of an import not at all. <code>main</code> is not changed
            here. If <code>uplink/upstream</code> itself fails the script, the run fails and no patch is blamed.
          </p>
        </Step>
        <Step n={11} title="Rebase a branch a rebuild left behind" workflow="uplink-pr.yml → uplink-rebase.yml">
          <MermaidDiagram chart={REBASE_CHART} title="The commit to rebase from is worked out in the job, never read from the dispatch" />
          <p>
            The pull request number is the only input: a wrong base makes a rebase drop or repeat commits. A push
            the developer made meanwhile wins. Pull requests from forks and branches named <code>main</code> or{" "}
            <code>uplink/*</code> are left alone. The committer becomes the bot and commit signatures are lost.
          </p>
        </Step>
        <Step n={12} title="The check on a gated pull request" workflow="uplink-gate.yml">
          <p>
            Every PR into a gated base runs <strong>Uplink gate</strong>. It fails if conflict markers remain, or
            if the PR changes pack files or anything under <code>.uplink/</code>. What else it runs depends on the
            base and on the queue the patch is in:
          </p>
          <div className="overflow-x-auto">
            <table className="w-full min-w-[640px] text-left text-sm">
              <thead className="text-xs tracking-wide uppercase">
                <tr className="border-b">
                  <th className="py-2 pr-3 font-medium">Base</th>
                  <th className="py-2 pr-3 font-medium">Patch is</th>
                  <th className="py-2 font-medium">Runs</th>
                </tr>
              </thead>
              <tbody>
                {GATE_CHECKS.map(([base, queue, runs]) => (
                  <tr key={base + queue} className="border-b border-border/60 align-top">
                    <td className="py-2 pr-3 font-mono text-xs text-foreground">{base}</td>
                    <td className="py-2 pr-3 whitespace-nowrap">{queue}</td>
                    <td className="py-2">{runs}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p>
            The workflow is <code>pull_request_target</code>: its YAML comes from the default branch, so the{" "}
            <code>-work</code> branch cannot replace it, and the gated tree is only data. The step that assesses
            and runs <code>preflight.sh</code> has no token.
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
            commit queue status together with <code>.uplink/previous-main.json</code>: the tip of the{" "}
            <code>main</code> it replaced and the commits of it that carry no <code>Uplink-Patch-Id</code> trailer.
          </li>
        </ol>
        <p>
          Every commit of the replay is new, so a branch cut from the old <code>main</code> no longer shares it.{" "}
          <code>git uplink rebase</code> finds the newest commit of an old <code>main</code> in the branch (one
          with the trailer, or one a revision of <code>previous-main.json</code> lists) and runs{" "}
          <code>git rebase --onto origin/main</code> from it.
        </p>
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
