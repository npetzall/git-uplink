import type React from "react";
import { Link } from "react-router-dom";
import { DocPage, Section, Code } from "../components/doc-page";

const ROLES = [
  {
    role: "Internal",
    remote: "origin",
    env: "UPLINK_INTERNAL_TOKEN / UPLINK_INTERNAL_KEY",
    can: "Push company main, uplink/state and the gated branches; open the gated pull requests",
    where: "Repository secret. Only in jobs that write, never in a job that runs preflight.sh",
  },
  {
    role: "Upstream",
    remote: "upstream",
    env: "UPLINK_UPSTREAM_TOKEN / UPLINK_UPSTREAM_KEY",
    can: "Read public upstream, open and close the public pull request. No write access to code",
    where: "Repository secret. Optional for an https:// upstream, which is fetched anonymously",
  },
  {
    role: "Contrib",
    remote: "contrib",
    env: "UPLINK_CONTRIB_TOKEN / UPLINK_CONTRIB_KEY",
    can: "Push and delete uplink/<id> on the contribution fork",
    where: "Environment secret on to-upstream and abandon-contrib only: it exists after a reviewer approved",
  },
];

const LIMITS = [
  [
    "The preflight job can read company source",
    "It holds a read-only Actions token and a clone of the company repository. Code the script builds and runs, public upstream's included, can read both. It cannot write, and cannot reach the App tokens or Environment secrets.",
  ],
  [
    "The preflight verdict is an exit code",
    "Code the script runs could force it to 0. Preflight checks that a change builds and passes its tests. It is not a defence against hostile code in the tree: the from-upstream review is.",
  ],
  [
    "Assess is a keyword scan",
    "It finds the words and email domains in uplink.toml, in the message and the diff. It does not read binary files (it warns), and it does not understand what code reveals. The IP reviewer does.",
  ],
  [
    "A PAT commit is unverified",
    "GitHub signs the contribution commit it creates through the API, and marks it Verified only for a GitHub App token.",
  ],
  [
    "Releases are checksummed, not signed",
    "SHA256SUMS is published with the binaries, so it protects against a damaged or swapped download, not against a compromised release. Mirror releases you have reviewed with UPLINK_SRC if that matters to you.",
  ],
  [
    "Locally, preflight runs as you",
    "The token names are removed from the script's environment, but it can read what your user can: files, credential stores, other processes. The refusal to run next to a credential applies in CI only.",
  ],
];

function Table({ head, children, min = 640 }: { head: string[]; children: React.ReactNode; min?: number }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm" style={{ minWidth: min }}>
        <thead className="text-xs tracking-wide uppercase">
          <tr className="border-b">
            {head.map((h) => (
              <th key={h} className="py-2 pr-3 font-medium">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>{children}</tbody>
      </table>
    </div>
  );
}

export function SecurityPage() {
  return (
    <DocPage
      eyebrow="Security"
      title="What Uplink guards, and what is left to you"
      lead={
        <>
          Uplink moves code between a private forge and the public. Two things must not happen: company code or
          text leaving before it is approved, and foreign code or a pull request gaining the bot&apos;s write
          access. This page lists what the tool does about both, where it stops, and what the operator has to
          set up.
        </>
      }
    >
      <Section id="credentials" title="One credential per role, never yours">
        <p>
          Every network <code>git</code> call picks its credential by remote, from the environment of that one
          invocation. Nothing else is tried.
        </p>
        <Table head={["Role", "Remote", "Environment", "Can", "Where the pack keeps it"]} min={820}>
          {ROLES.map((r) => (
            <tr key={r.role} className="border-b border-border/60 align-top">
              <td className="py-2 pr-3 text-foreground">{r.role}</td>
              <td className="py-2 pr-3 font-mono text-xs">{r.remote}</td>
              <td className="py-2 pr-3 font-mono text-xs">{r.env}</td>
              <td className="py-2 pr-3">{r.can}</td>
              <td className="py-2">{r.where}</td>
            </tr>
          ))}
        </Table>
        <ul>
          <li>
            <strong>No ambient credentials.</strong> Credential helpers are blanked, global and system git config
            are ignored, <code>GIT_SSH</code> and <code>GIT_SSH_COMMAND</code> are dropped, and SSH runs with{" "}
            <code>BatchMode=yes</code> and <code>IdentitiesOnly=yes</code> on the one key given. Your ssh-agent,
            your <code>gh</code> login and your keychain are never asked.
          </li>
          <li>
            <strong>Fails closed.</strong> A call to <code>origin</code> or <code>contrib</code> without that
            role&apos;s credential is an error, not a prompt. Only an <code>https://</code> upstream is fetched
            without one.
          </li>
          <li>
            <strong>The checkout token is overridden.</strong> <code>actions/checkout</code> can leave{" "}
            <code>GITHUB_TOKEN</code> in <code>.git/config</code>. Uplink blanks that header for its own calls
            and sends only the role&apos;s token.
          </li>
          <li>
            <strong>Tokens stay out of the process list.</strong> They reach git through{" "}
            <code>GIT_CONFIG_*</code> environment variables, not <code>-c</code> arguments.
          </li>
          <li>
            <strong>Your git setup is not used either.</strong> Repository hooks are off (
            <code>core.hooksPath=/dev/null</code>), external diff drivers are off, commits are made as the bot
            and never signed with your key. <code>init</code> does not rewrite your clone&apos;s config.
          </li>
        </ul>
      </Section>

      <Section id="product-code" title="Product code never runs next to a write token">
        <p>
          <code>preflight.sh</code> builds and runs the product, and public upstream with it. That is the one
          place where code nobody at the company reviewed executes.
        </p>
        <ul>
          <li>
            <code>GITHUB_TOKEN</code>, <code>GH_TOKEN</code>, the Actions runtime tokens and every{" "}
            <code>UPLINK_*_TOKEN</code> / <code>UPLINK_*_KEY</code> are removed from the script&apos;s
            environment, and stored git credentials are blanked for git the script runs.
          </li>
          <li>
            In CI that is not enough: a process can read its parent, the runner, or <code>.git/config</code>.
            So <code>git uplink</code> <strong>refuses to run the script</strong> in a CI job that holds any of
            those credentials.
          </li>
          <li>
            The pack therefore splits every flow that needs a verdict: a preflight job with a read-only token,
            no App token, no Environment and no credentials in the checkout prints a result, and the job that
            writes takes it with <code>--preflight-result</code> and runs no script.
          </li>
          <li>
            The result names the tree and the hooks it was tested with. If either changed in between, the
            writing job refuses it.
          </li>
          <li>
            The toolchain hook runs only in those same read-only jobs.
          </li>
        </ul>
      </Section>

      <Section id="outbound" title="Nothing leaves without an approval for exactly those bytes">
        <ul>
          <li>
            <strong>The fork-write credential exists only after the gate.</strong> It is an Environment secret
            on <code>to-upstream</code>, so no job can read it before an IP reviewer approved that run.
          </li>
          <li>
            <strong>The approval is bound to content.</strong> The packet carries a review token over the lines
            the patch adds and removes, its public title and its public message. <code>approve --reviewed</code>{" "}
            stops when the patch is no longer what was reviewed.
          </li>
          <li>
            <strong>Reviewers and auditors read the same bytes.</strong> The packet is committed to{" "}
            <code>uplink/state</code> before the wait, and the Environment links that commit.
          </li>
          <li>
            <strong>The message is rebuilt, not trusted.</strong> Everything below the cutoff is removed, the
            author becomes the export identity, and the result is scanned for the words and email domains in{" "}
            <code>uplink.toml</code>. A missing or broken <code>uplink.toml</code> fails the assessment rather
            than scanning for nothing.
          </li>
          <li>
            <strong>Internal-only patches are never exported</strong>, and an upstream-bound patch may not
            depend on one.
          </li>
          <li>
            <strong>Resolutions and amends are assessed again</strong> before they merge, and a change to the
            approved lines of a submitted patch goes back to IP as a delta.
          </li>
          <li>
            The public pull request is opened with <code>maintainer_can_modify</code> off, and withdrawing one
            waits on its own Environment, <code>abandon-contrib</code>.
          </li>
        </ul>
      </Section>

      <Section id="inbound" title="Nothing arrives unreviewed">
        <ul>
          <li>
            <strong>Foreign upstream commits wait.</strong> Sync fetches without moving{" "}
            <code>uplink/upstream</code>. Anything that is not one of our own merged patches is written to a
            packet and held on <code>from-upstream</code> until a reviewer approves that exact commit.
          </li>
          <li>
            <strong>A trailer is not proof.</strong> Patch ids are public, so anyone can write{" "}
            <code>Uplink-Patch-Id</code> on a commit. A commit is ours only when its{" "}
            <code>git patch-id --stable</code> matches, or the recorded public PR is merged. A commit that only
            claims a patch is listed for the reviewer.
          </li>
          <li>
            <strong>An approved upstream is tested before it is promoted</strong>, in a job without write
            credentials.
          </li>
        </ul>
      </Section>

      <Section id="pull-requests" title="A pull request cannot reach the bot">
        <ul>
          <li>
            Every workflow starts from <code>permissions: {"{}"}</code> and each job declares what it needs.
            Jobs that only read check out with <code>persist-credentials: false</code>.
          </li>
          <li>
            Workflows started by a gated pull request use <code>pull_request_target</code>: the YAML comes
            from the default branch, and the gated tree is data that is never executed with a token.
          </li>
          <li>
            The script and the settings are read from <code>uplink/hooks</code>, never from the pull request, so
            a PR cannot change what CI runs or what counts as a leak.
          </li>
          <li>
            <strong>Uplink gate</strong> fails a gated PR that changes pack files or anything under{" "}
            <code>.uplink/</code>.
          </li>
          <li>
            Gated pull requests are opened by the internal App, so Actions needs no right to create or approve
            pull requests.
          </li>
          <li>
            The rebase job takes only a pull request number and works out the base itself. It rebases in a
            worktree with hooks off and pushes with <code>--force-with-lease</code>.
          </li>
          <li>Third-party actions in the pack are pinned to a full commit SHA.</li>
        </ul>
        <p>
          Which job holds what, flow by flow, is in <Link to="/setup?forge=github&amp;view=workflows">Workflows</Link>{" "}
          under <em>Requires</em>. The flows themselves are on <Link to="/internals">Internals</Link>.
        </p>
      </Section>

      <Section id="binary" title="The binary and the local UI">
        <ul>
          <li>
            Release binaries come with <code>SHA256SUMS</code> and CycloneDX and SPDX SBOMs. The pack&apos;s
            install action checks the checksum on every job, and installs the release that wrote the pack
            unless you set <code>UPLINK_VERSION</code>.
          </li>
          <li>
            Dependencies are checked with <code>cargo deny</code>, npm audit and Socket on every change.
          </li>
          <li>
            <code>git uplink web-ui</code> listens on <code>127.0.0.1</code> only and rejects a request whose{" "}
            <code>Host</code> or <code>Origin</code> is not loopback, which stops DNS rebinding and other sites
            in your browser.
          </li>
        </ul>
      </Section>

      <Section id="limits" title="Known limits">
        <Table head={["Limit", "What it means"]}>
          {LIMITS.map(([limit, meaning]) => (
            <tr key={limit} className="border-b border-border/60 align-top">
              <td className="py-2 pr-3 text-foreground">{limit}</td>
              <td className="py-2">{meaning}</td>
            </tr>
          ))}
        </Table>
      </Section>

      <Section id="local" title="Your part: running git uplink locally">
        <p>
          Locally there is no Environment and no job boundary. The only limit on what a command can do is the
          credential you hand it, so hand it a small one.
        </p>
        <ul>
          <li>
            <strong>Use a fine-grained token for <code>UPLINK_INTERNAL_TOKEN</code>, limited to the one product
            repository.</strong> Then Uplink cannot change any other repository, whatever the queue or a
            remote URL says. Not a classic token, and not your <code>gh</code> token.
          </li>
          <li>
            <strong>Give it only what the command needs.</strong> <em>Contents: read</em> is enough for{" "}
            <code>status</code>, <code>rebase</code>, <code>preflight</code> and <code>web-ui</code>. Pushing (<code>push</code>, <code>rebuild --push</code>) needs{" "}
            <em>Contents: write</em>, and <em>Workflows: write</em> when the push carries the pack (
            <code>init</code>, <code>init --upgrade</code>).
          </li>
          <li>
            <strong>Set it for the command, not for the shell.</strong> A token in your profile is in the
            environment of everything you run.
          </li>
        </ul>
        <Code>{`UPLINK_INTERNAL_TOKEN="$(pass show uplink/internal-read)" git uplink status`}</Code>
        <ul>
          <li>
            <strong>Leave <code>UPLINK_UPSTREAM_TOKEN</code> unset</strong> for an <code>https://</code>{" "}
            upstream. It is fetched anonymously.
          </li>
          <li>
            <strong>Do not hold <code>UPLINK_CONTRIB_TOKEN</code> on a workstation.</strong> With it,{" "}
            <code>git uplink submit --push</code> publishes without the IP gate. Publishing belongs to the
            submit workflow.
          </li>
          <li>
            <strong>A <code>*_KEY</code> must be passwordless</strong>, so it is only as safe as the file. Use
            a deploy key made for this, on that one repository, read-only unless you push.
          </li>
          <li>
            <strong>Treat local preflight like any local build.</strong> It runs the product and public
            upstream as you. For a tree you do not trust, let CI run it or use a container.
          </li>
          <li>
            <strong>Nobody pushes company <code>main</code> by hand</strong> after setup. The ruleset enforces
            it; do not add yourself to its bypass list to get unstuck.
          </li>
        </ul>
      </Section>

      <Section id="forge" title="Your part: the forge">
        <p>
          The pack assumes the settings from <Link to="/setup">Production setup</Link>. These are the ones the
          model depends on:
        </p>
        <ul>
          <li>
            <strong>One GitHub App per role, installed only on its repositories.</strong> Internal on the
            product repository, contrib on the fork, upstream on upstream and the fork with no contents write.
          </li>
          <li>
            <strong>Contrib secrets on the two Environments only</strong>, never at repository or organization
            level, where every job could read them.
          </li>
          <li>
            <strong>Environments with required reviewers, <em>Prevent self-review</em>, and deployment branch{" "}
            <code>main</code>.</strong> Without the branch limit, a workflow on any branch could ask for the
            secrets.
          </li>
          <li>
            <strong>All four rulesets active:</strong> company <code>main</code>, the gated bases, the pack
            files, and <code>uplink/hooks</code>. Only the internal App bypasses, and nothing bypasses{" "}
            <code>uplink/hooks</code>.
          </li>
          <li>
            <strong>Protect <code>uplink/hooks</code> like code that runs in CI</strong>, because it is: it
            holds <code>preflight.sh</code>, the hooks, and the list of what counts as a leak. See{" "}
            <Link to="/setup?forge=github&amp;view=hooks">Hooks</Link>.
          </li>
          <li>
            <strong>Actions default permission read</strong>, and <em>Allow GitHub Actions to create and
            approve pull requests</em> off.
          </li>
          <li>
            <strong>If you use PATs, use a machine user</strong>, never a person&apos;s account, and
            fine-grained tokens with the permissions of the App they replace. Keep the fork in the organization
            that owns upstream: otherwise only a classic PAT can open the public PR, and a classic PAT reaches
            everything its account can.
          </li>
          <li>
            <strong>Keep hooks small.</strong> The toolchain hook installs pinned tools and runs no product
            code. The assessment hook reads a package of kind <code>pr</code> as data: it describes an
            unreviewed change, so do not build it in a job that has secrets.
          </li>
          <li>
            <strong>Do not set <code>UPLINK_VERSION</code> to <code>latest</code>.</strong> Leave it unset, so
            jobs run the release that wrote the pack.
          </li>
          <li>
            <strong>Know what the opt-ins change.</strong> <code>UPLINK_AUTO_SUBMIT</code> only dispatches; IP
            still approves each run. <code>UPLINK_AUTO_REBASE</code> rewrites developers&apos; branches as the
            bot and drops commit signatures.
          </li>
          <li>
            <strong>Delete the downloaded <code>.pem</code> files</strong> once the secrets are stored, and
            rotate App keys like any other credential.
          </li>
        </ul>
      </Section>
    </DocPage>
  );
}
