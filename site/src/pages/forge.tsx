import { Link } from "react-router-dom";
import { DocPage, Section } from "../components/doc-page";

/** [workflow, what starts it, what it does, git uplink commands it runs] */
const WORKFLOWS: [string, string, string, string][] = [
  [
    "Uplink PR checks",
    "Pull request to main",
    "Assesses the PR as the public commit and runs preflight.sh on upstream plus its dependencies; comments both.",
    "assess, preflight",
  ],
  [
    "Uplink import",
    "Pull request merged into main",
    "Records the change as a queued patch, rebuilds main when an internal patch must stay on top, comments the patch id.",
    "preflight, add, push",
  ],
  [
    "Uplink submit",
    "Developer dispatch, or the forge with UPLINK_AUTO_SUBMIT",
    "Writes the assessment packet, waits on the to-upstream review, exports the patch and opens the public PR.",
    "assess, report, preflight, waiting, approve, submit, submitted, rejected",
  ],
  [
    "Uplink sync",
    "Hourly, daily and weekly schedules; operator dispatch",
    "Accounts for new upstream commits, drops merged patches, rebuilds main; asks the from-upstream review when upstream brings changes that are not ours; opens a conflict PR on a conflict.",
    "sync, accept-upstream, gated",
  ],
  [
    "Uplink resolve",
    "Conflict PR merged",
    "Puts the resolved patch back in the queue and rebuilds main.",
    "resolve, gated",
  ],
  [
    "Uplink amend",
    "Developer dispatch; amend PR closed",
    "Opens a draft amend PR at the patch, then folds the merged PR into the patch.",
    "amend, gated",
  ],
  [
    "Uplink transfer",
    "Developer dispatch; transfer PR closed",
    "Moves a patch between the internal and upstream queues, through a transfer PR when it needs changes.",
    "transfer, gated",
  ],
  [
    "Uplink gate",
    "Pull request into a conflict, amend or transfer base",
    "Assesses and preflights what the gated PR would make the patch.",
    "assess, preflight",
  ],
  [
    "Uplink rebase",
    "uplink:rebase label, UPLINK_AUTO_REBASE, dispatch",
    "Comments how to rebase a PR whose main was replaced, and rebases it when asked.",
    "rebase --plan",
  ],
  [
    "Uplink verify",
    "Operator dispatch",
    "Tests main with preflight.sh and gates the patch it fails on.",
    "rebuild --verify, gated",
  ],
  [
    "Uplink abandon contrib",
    "Transfer to internal of a submitted patch",
    "Closes the public PR after the abandon-contrib review.",
    "none (gh only)",
  ],
];

export function ForgePage() {
  return (
    <DocPage
      eyebrow="Forge"
      title="The workflows do the bookkeeping."
      lead={
        <>
          The forge is the pack of workflows <code>git uplink init</code>{" "}
          installs on company <code>main</code>. It runs <code>git uplink</code>{" "}
          so that nobody else has to: developers use pull requests, and the{" "}
          <Link to="/operator">operator</Link> only sets things up and repairs.
        </>
      }
    >
      <Section title="What the forge does">
        <ul>
          <li>
            <strong>Imports</strong> every merged pull request as a queued
            patch, and tells the pull request its patch id.
          </li>
          <li>
            <strong>Syncs</strong> upstream on a schedule, drops patches
            upstream merged, and rebuilds <code>main</code>.
          </li>
          <li>
            <strong>Submits</strong> approved patches to the contribution fork,
            after the <code>to-upstream</code> review.
          </li>
          <li>
            <strong>Rebases</strong> pull requests a rebuild left behind, when
            asked.
          </li>
          <li>
            <strong>Gates</strong> every change to a patch through a pull
            request: conflicts, amends and transfers each get a draft PR from a{" "}
            <code>-work</code> branch, checked by <strong>Uplink gate</strong>.
            Merging it finishes the change; closing it cancels.
          </li>
        </ul>
        <p>
          Reviewers approve the forge&apos;s Environments (
          <code>to-upstream</code>, <code>from-upstream</code>,{" "}
          <code>abandon-contrib</code>) in the forge; they run nothing.
        </p>
      </Section>

      <Section id="workflows" title="Workflows">
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead>
              <tr className="border-b border-border text-foreground">
                <th className="py-2 pr-3">Workflow</th>
                <th className="py-2 pr-3">Started by</th>
                <th className="py-2 pr-3">Does</th>
                <th className="py-2">Runs</th>
              </tr>
            </thead>
            <tbody>
              {WORKFLOWS.map(([name, trigger, does, runs]) => (
                <tr key={name} className="border-b border-border/60 align-top">
                  <td className="py-2 pr-3 text-foreground">{name}</td>
                  <td className="py-2 pr-3">{trigger}</td>
                  <td className="py-2 pr-3">{does}</td>
                  <td className="py-2">
                    <code>{runs}</code>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <p>
          What each workflow needs (tokens, labels, Environments) is in the{" "}
          <Link to="/setup?forge=github&amp;view=workflows">
            workflow reference
          </Link>
          . Every flow, step by step, is in{" "}
          <Link to="/internals">Internals</Link>. The commands are grouped as{" "}
          <em>Forge</em> in the <Link to="/cli">CLI</Link> reference.
        </p>
      </Section>

      <Section id="rules" title="Rules the forge keeps">
        <ul>
          <li>
            Workflows that write the queue run one at a time (concurrency group{" "}
            <code>uplink-mutate</code>), and none is dropped.
          </li>
          <li>
            <code>preflight.sh</code> runs in a job with no write credential;
            the job that writes takes its result.
          </li>
          <li>
            Every job runs the <code>git-uplink</code> release that wrote the
            pack, and checks its signature.
          </li>
          <li>
            Gated pull requests are opened by the internal App, so the
            repository can keep Actions from creating pull requests.
          </li>
        </ul>
      </Section>
    </DocPage>
  );
}
