import { AppShell } from "../components/app-shell";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";

export function PlaybookPage() {
  return (
    <AppShell>
      <article className="mx-auto max-w-3xl space-y-10">
        <header className="space-y-3">
          <p className="text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">
            System playbook
          </p>
          <h1 className="text-4xl font-semibold tracking-tight">
            Uplink: contributing to public GitHub from an EMU enterprise
          </h1>
          <p className="text-lg leading-8 text-muted-foreground">
            A patch-queue operating model plus a small bot. Third-party tools fill the airlock and
            the GitHub API; they do not replace the queue. Developers keep one branch per change.
            Company builds stay current. Merged contributions fall out of the internal build so they
            cannot revert a later upstream fix.
          </p>
        </header>

        <Section title="Why this shape">
          <p>
            Enterprise Managed Users can read public GitHub.com and cannot write to it: no forks, no
            pull requests, no comments, including via the API. That is a hard platform constraint,
            not a policy choice. The company already decided the public project will host a private
            fork in the upstream organisation and hand over either a fine-grained token for a
            machine user or a GitHub App that can push branches and open pull requests. That
            decision is correct for secrecy. It is not, by itself, a product-build strategy.
          </p>
          <p>
            GitHub&apos;s Private Mirrors App solves the airlock if the company also has a public
            github.com organisation and is willing to show a public fork. It does not rebuild a
            company mainline from upstream plus unmerged work, does not drop patches after merge,
            and does not amend a pending contribution when a sync conflict is resolved. Google
            Copybara is excellent at 1:1 repo transforms and weaker at an ordered, drop-on-merge
            queue. StGit implements the queue semantics; developers should not have to run it.
            Uplink takes StGit&apos;s model and binds it to GitHub Enterprise review, the
            upstream-owned fork, and merge detection.
          </p>
        </Section>

        <Section title="The invariant">
          <p>
            There is exactly one object per logical change: a patch with a stable id
            (<code>upl_…</code>) stored in <code>.uplink/</code>. Every other git ref is derived.
          </p>
          <ul>
            <li>Internal PR → creates or amends that patch.</li>
            <li>
              Company <code>main</code> → replay of <code>upstream/main</code> plus every patch that
              is not merged or dropped.
            </li>
            <li>
              Contribution fork branch <code>uplink/&lt;id&gt;</code> → the same patch applied onto
              current upstream (or onto a submitted dependency).
            </li>
            <li>
              Public pull request → opened from that generated branch after the internal IP gate.
            </li>
          </ul>
          <p>
            Developers never maintain a second branch for the public side. When they fix a conflict
            or address review, they amend the patch. If it is already submitted, the bot
            force-pushes the fork branch and the open PR updates.
          </p>
        </Section>

        <Section title="Repository topology">
          <p>
            <strong>Company product repo</strong> lives in the EMU enterprise. It is a mirror, not a
            GitHub fork network member — EMU cannot fork public repositories. Humans open PRs only
            here. <code>main</code> is bot-owned and may be force-updated on sync. Branch from
            latest <code>main</code>; rebase in-flight work after a sync the same way you would
            after any integration branch update.
          </p>
          <p>
            <strong>Contribution fork</strong> is a private fork of the public project, owned by the
            upstream organisation. Until a pull request is opened, the public cannot see the work.
            Upstream maintainers can, which is the intended private review channel. Do not put this
            fork in a company-owned public org if the goal is to hide that the company is preparing
            a contribution.
          </p>
          <p>
            <strong>Canonical upstream</strong> remains the public repository. Maintainers merge
            with their normal button. Prefer squash or rebase merges; Uplink records the PR number
            and writes <code>Uplink-Patch-Id</code> into the exported commit message so detection
            still works.
          </p>
        </Section>

        <Section title="Multiple developers on company main">
          <p>
            Yes. Company <code>main</code> is a shared integration branch, but humans do not push
            it. Any number of developers branch from latest <code>main</code>, open ordinary
            internal PRs, and review each other.               The Uplink bot is the only writer of{" "}
              <code>main</code>. After each import or sync it may force-update that branch; packet
              commits of <code>.uplink/reports/</code> are fast-forwards. In-flight PRs rebase onto
              the new main the same way they would after any integration-branch update.
          </p>
          <p>
            Ruleset: require a pull request from humans; allow the Uplink bot (and only that bot)
            to bypass and force-push. Developers keep one feature branch per change. They never
            maintain a second branch for upstream.
          </p>
        </Section>

        <Section title="Two approval gates">
          <p>
            Internal product approval and contribution approval are different events. Mixing them
            would block the company build on legal review.
          </p>
          <ol>
            <li>
              <strong>Prepare for upstream (on the internal PR).</strong> A second workflow rewrites
              the change as a contribution: public commit message above the cutoff, export author
              (machine user or <code>Uplink-Export-Author</code>), affiliation scan (company name,
              internal emails in tests and diffs). The report is posted on the PR so IP can approve
              from results instead of reconstructing the public patch.
            </li>
            <li>
              <strong>Internal product (status <code>queued</code>).</strong> Engineering review on
              the internal PR, then label <code>uplink:import</code> (or merge). Actions runs{" "}
              <code>git uplink add</code>. The change is now on company <code>main</code>. That is
              when it is approved for the internal — the product build includes it, and the next
              developer who branches from main gets it. Default intent is upstream; label{" "}
              <code>uplink:internal-only</code> for the escape hatch. IP has not run yet. Nothing
              has left EMU.
            </li>
            <li>
              <strong>Contribution / IP (status <code>approved</code>, then{" "}
              <code>submitted</code>).</strong> Dispatch <code>Uplink submit</code>. IP reviews
              the packet on <code>GITHUB_STEP_SUMMARY</code> and in{" "}
              <code>.uplink/reports/&lt;id&gt;/prepare.md</code>, then approves the GitHub
              Environment named <code>oss</code>. GitHub records that review (Deployments +
              enterprise audit log). The same run writes <code>approval.md</code>, then{" "}
              <code>git uplink approve</code> / <code>git uplink submit</code>. App credentials that can
              push the public fork exist only on that environment. Internal-only patches are
              refused here.
            </li>
          </ol>
        </Section>

        <Section title="OSS Environment as the IP gate">
          <p>
            On GitHub Enterprise Cloud, contribution approval is a GitHub Environment named{" "}
            <code>oss</code>, not a sidecar process. Required reviewers are IP/legal. The GitHub App
            that can push the upstream-owned private fork lives as <em>environment</em> secrets, so
            those credentials do not exist in a job until the deployment is approved.
          </p>
          <p>
            Dispatch <code>Uplink submit</code> from company <code>main</code>. A packet job writes{" "}
            <code>.uplink/reports/&lt;id&gt;/prepare.md</code>, appends{" "}
            <code>GITHUB_STEP_SUMMARY</code>, and fast-forwards that file onto <code>main</code>.
            Reports stay under <code>.uplink/</code>, so queue rebuilds copy them. The submit job
            then waits on <code>environment: oss</code>. After review, it commits{" "}
            <code>approval.md</code> and runs <code>git uplink approve</code> then{" "}
            <code>git uplink submit</code> in the same workflow.
          </p>
          <p>
            The dispatcher is not the IP approver. GitHub records the environment reviewer on the
            Deployments tab and in the enterprise audit log. Turn on Prevent self-review. Do not put
            the write App secrets at repo or org level, or a job without the environment can still
            mint a token. Packet and submit use job-level <code>uplink-mutate</code> concurrency so
            the environment wait does not freeze imports.
          </p>
          <p>
            Setup: <code>templates/README.md</code>. Workflow:{" "}
            <code>templates/emu-workflows/uplink-submit.yml</code>.
          </p>
        </Section>

        <Section title="Concurrent adds">
          <p>
            Two PRs can be reviewed at the same time. They must not both rewrite{" "}
            <code>.uplink/queue.json</code> from a stale checkout, or the later force-push would
            drop the earlier patch.
          </p>
          <p>Uplink handles that in three layers:</p>
          <ul>
            <li>
              GitHub Actions <code>concurrency: uplink-mutate</code> on import and sync (workflow
              level) and on the submit packet/submit jobs (job level, so the <code>oss</code>{" "}
              environment wait does not freeze imports). One mutation at a time; later jobs wait
              rather than cancel.
            </li>
            <li>
              A process lock in <code>.git/uplink.lock</code> so two CLI processes in the same
              checkout cannot interleave queue writes.
            </li>
            <li>
              Each import isolates the PR&apos;s unique diff from the PR&apos;s own{" "}
              <code>base.sha..head.sha</code> (not from live main), then fetches the latest queue,
              appends, rebuilds, and pushes with <code>--force-with-lease</code>. If another import
              landed first, the lease fails, the job refreshes, and it retries. Re-importing the
              same internal PR number is a no-op.
            </li>
          </ul>
          <p>
            If the two changes overlap and the second patch does not apply onto the first, the
            queue stops in <code>conflict</code>. Rebase the still-open PR onto the new main and
            import again — or resolve on <code>uplink/conflict/&lt;id&gt;</code>. Independent
            files land without developer coordination beyond the usual rebase-after-main-moved.
          </p>
        </Section>

        <Section title="Export preflight">
          <p>
            Branching from company <code>main</code> does not record <code>dependsOn</code>. A
            change that only makes sense on that tree would otherwise become an incomplete public
            PR. Before import, and again before submit, Uplink applies the candidate onto public
            upstream plus declared dependencies and runs <code>UPLINK_PREFLIGHT</code> (the
            product build and test) on that export tree — not on company main.
          </p>
          <p>
            Failure blocks import, does not push the contribution fork, and does not open an
            upstream PR. The internal PR is commented with suggested{" "}
            <code>Uplink-Depends-On</code> lines. Copy{" "}
            <code>templates/emu-workflows/uplink-preflight.yml</code> and make it a required check.
            Set Actions variable <code>UPLINK_PREFLIGHT</code> to the command that must pass for a
            contribution (for example <code>npm test</code>).
          </p>
        </Section>

        <Section title="Developer workflow">
          <ol>
            <li>Branch from company <code>main</code> (already includes unmerged patches).</li>
            <li>
              Write the public rationale above{" "}
              <code>----- Uplink: internal below this line -----</code>. Tickets and{" "}
              <code>Uplink-Export-Author</code> go below it. <code>git uplink init</code> installs that
              git commit template.
            </li>
            <li>
              Open an internal PR. CI runs prepare (scrub, author, affiliation), export preflight,
              and company-tree tests. If prepare fails, remove company names from the diff (including
              tests) or move internal notes below the cutoff.
            </li>
            <li>
              Labels: default intent is upstream. Escape hatch:{" "}
              <code>uplink:internal-only</code>.
            </li>
            <li>
              Engineering review (required reviewers / CODEOWNERS). This is code review, not IP.
            </li>
            <li>
              Label <code>uplink:import</code>. That is internal product approval. Actions runs{" "}
              <code>git uplink add</code>, extracts the product diff, excluding{" "}
              <code>.uplink/</code>, and rebuilds main.
            </li>
            <li>
              When legal signs off, an operator dispatches <code>Uplink submit</code>. IP approves
              the <code>oss</code> Environment on the waiting run. The same workflow then{" "}
              <code>git uplink approve</code> and <code>git uplink submit</code>. Submit is the first time
              bytes leave EMU.
            </li>
          </ol>
          <p>
            Stacking is implicit: a PR opened on a main that already carries patch A becomes patch B
            depending on A. An upstream-bound patch may not depend on an internal-only patch; the
            engine rejects that so you cannot export something that only applies on secret code.
          </p>
        </Section>

        <Section title="Merge detection — and why later fixes survive">
          <p>
            Once a patch is marked <code>merged</code>, it is never applied again. That is sticky.
            Detection order:
          </p>
          <ol>
            <li>
              GitHub API: the recorded pull request is merged. Authoritative even if upstream edited
              the diff.
            </li>
            <li>
              Trailer: <code>git log --grep=&apos;Uplink-Patch-Id: upl_…&apos;</code> on
              upstream/main.
            </li>
            <li>
              <code>git patch-id --stable</code> match against recent upstream commits.
            </li>
            <li>
              Empty apply / reverse-apply check: the tree already contains an identical change.
            </li>
          </ol>
          <p>
            Empty-apply is a hint, not the primary signal. If upstream merged your change and then
            modified the same lines, re-applying your original patch would resurrect the pre-fix
            version. Trailer and PR number exist so that case drops the patch instead of
            conflicting or reverting. Operators can still run{" "}
            <code>git uplink merged &lt;id&gt; --via manual</code> if a maintainer cherry-picked without
            the trailer and without using the PR.
          </p>
        </Section>

        <Section title="Sync conflicts">
          <p>
            Hourly (and on demand) the bot fetches public upstream, drops merged patches, and
            replays the rest. If apply fails, it stops on that patch, points{" "}
            <code>HEAD</code> at <code>uplink/conflict/&lt;id&gt;</code>, and opens an internal PR.
            The developer resolves the files, <code>git add</code>, and runs{" "}
            <code>git uplink resolve &lt;id&gt;</code>. That refreshes the single patch. Remaining
            patches then replay. If the patch was already submitted, the next submit/sync
            force-pushes the contribution branch so the upstream PR is amended.
          </p>
        </Section>

        <Section title="Credentials on GHEC EMU">
          <p>
            Prefer a GitHub App registered on public github.com and installed on the private fork
            (contents: write) and the public parent (pull requests: write, contents: read). Store
            the App ID and private key in EMU org secrets. Mint a one-hour installation token in
            Actions with <code>actions/create-github-app-token</code> and pass it as{" "}
            <code>UPLINK_GITHUB_TOKEN</code>. An EMU-created App may be enterprise-scoped and unable
            to talk to repositories outside the enterprise, so do not assume the company can
            register this App from an EMU admin account. Upstream, or a non-EMU admin, should own
            it.
          </p>
          <p>
            A machine-user fine-grained PAT also works: contents write on the fork, pull requests
            write on the public parent. It is worse to rotate and is tied to a person. Use it only
            if the App cannot be installed on the public parent.
          </p>
          <p>
            Fetching public upstream does not need a token. EMU Actions on GitHub-hosted runners
            can clone github.com anonymously. Pushing back into the EMU repo uses the default{" "}
            <code>GITHUB_TOKEN</code> with contents write and a ruleset exception for the bot.
          </p>
        </Section>

        <Section title="Internal-only escape hatch">
          <p>
            Label <code>uplink:internal-only</code> or <code>git uplink add --internal-only</code>.
            These patches rebase with the queue, appear in the company build, and are refused by{" "}
            <code>git uplink approve</code>/<code>git uplink submit</code>. Drop them with{" "}
            <code>git uplink drop</code> when the company-specific behaviour is retired. Goal remains:
            almost every internal contribution is also contributed upstream. Internal-only is
            explicitly the odd case.
          </p>
        </Section>

        <Section title="What to install">
          <ul>
            <li>
              This crate: the <code>git-uplink</code> binary, the git engine, this operator dashboard (`git uplink web-ui`), and <code>way-of-working.md</code> (developer stories).
            </li>
            <li>
              Copy <code>templates/emu-workflows/</code> into the company product repo. Make
              prepare and export preflight required checks. Create Environment <code>oss</code> with
              IP/legal as required reviewers and the GitHub App secrets on that environment only.
              Set <code>UPLINK_REDACT_KEYWORDS</code> and <code>UPLINK_EXPORT_AUTHOR</code>.
            </li>
            <li>
              Protect company <code>main</code>: only the Uplink bot / GitHub Actions may push.
              Require PRs from humans. Allow Actions to fast-forward{" "}
              <code>.uplink/reports/</code> and to force-push rebuilds.
            </li>
            <li>
              Optional: Private Mirrors App if you later want a company-owned public fork as well.
              It is complementary, not a substitute, and weaker for secrecy than the
              upstream-owned fork you already chose.
            </li>
          </ul>
        </Section>

        <Card>
          <CardHeader>
            <CardTitle>Commands</CardTitle>
          </CardHeader>
          <CardContent>
            <pre className="overflow-x-auto rounded-lg bg-black/40 p-4 font-mono text-xs leading-6 text-zinc-200">{`git uplink init --upstream https://github.com/org/proj.git --contrib https://github.com/org/proj-company.git
git uplink add --title "Use SHA-256 for tokens"
git uplink add --title "Vendor hook" --internal-only
git uplink report upl_ab12cd34ef
git uplink approve upl_ab12cd34ef
git uplink submit upl_ab12cd34ef
git uplink sync
git uplink resolve upl_ab12cd34ef
git uplink merged upl_ab12cd34ef --via pr
git uplink drop upl_ab12cd34ef --reason "retired"
git uplink web-ui`}</pre>
          </CardContent>
        </Card>
      </article>
    </AppShell>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="space-y-3 text-[15px] leading-7 text-muted-foreground [&_code]:rounded [&_code]:bg-muted [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:text-[13px] [&_code]:text-foreground [&_li]:ms-5 [&_li]:list-disc [&_ol]:space-y-2 [&_ol>li]:ms-5 [&_ol>li]:list-decimal [&_p]:text-pretty [&_strong]:text-foreground [&_ul]:space-y-2">
      <h2 className="text-xl font-semibold text-foreground">{title}</h2>
      {children}
    </section>
  );
}
