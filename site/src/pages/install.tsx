import { Link } from "react-router-dom";
import { AppShell } from "../components/app-shell";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Button } from "../components/ui/button";
import { GITHUB_BLOB, GITHUB_REPO } from "../lib/links";

export function InstallPage() {
  return (
    <AppShell>
      <article className="mx-auto max-w-3xl space-y-10">
        <header className="space-y-3">
          <p className="text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">Install</p>
          <h1 className="text-4xl font-semibold tracking-tight">Get git uplink on PATH</h1>
          <p className="text-lg leading-8 text-muted-foreground">
            The binary is <code className="rounded bg-muted px-1.5 py-0.5 text-[15px] text-foreground">git-uplink</code>,
            so Git treats it as <code className="rounded bg-muted px-1.5 py-0.5 text-[15px] text-foreground">git uplink</code>.
            Building the crate needs Rust 1.98+ (<code className="rounded bg-muted px-1.5 py-0.5 text-foreground">rust-toolchain.toml</code>{" "}
            pins 1.98.1), Node.js 22 (for the embedded operator UI), and npm.
          </p>
        </header>

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">From a clone</h2>
          <pre className="overflow-x-auto rounded-lg bg-black/40 p-4 font-mono text-xs leading-6 text-zinc-200">{`git clone ${GITHUB_REPO}.git
cd git-uplink
cargo install --path .
git uplink -h`}</pre>
          <p className="text-[15px] leading-7 text-muted-foreground">
            For a local binary without installing into Cargo&apos;s bin directory:
          </p>
          <pre className="overflow-x-auto rounded-lg bg-black/40 p-4 font-mono text-xs leading-6 text-zinc-200">{`cargo build --release
export PATH="$PWD/target/release:$PATH"`}</pre>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Use <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">git-uplink -h</code> or{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">git uplink -h</code>. Plain{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">git uplink --help</code> goes through
            Git&apos;s man-page path, not clap.
          </p>
        </section>

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">Operator UI vs this site</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            This GitHub Pages site is the public playbook, lab, and install docs.{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">git uplink web-ui</code> is the local
            dashboard for the checkout you started in: it reads{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">.uplink/queue.json</code> from{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">uplink/state</code>.
          </p>
          <pre className="overflow-x-auto rounded-lg bg-black/40 p-4 font-mono text-xs leading-6 text-zinc-200">{`git uplink web-ui
git uplink web-ui --no-open
git uplink web-ui --port 43721 --bind 127.0.0.1`}</pre>
        </section>

        <Card>
          <CardHeader>
            <CardTitle>Commands</CardTitle>
          </CardHeader>
          <CardContent>
            <pre className="overflow-x-auto rounded-lg bg-black/40 p-4 font-mono text-xs leading-6 text-zinc-200">{`git uplink init [--upstream <url>] [--contrib <url>] [--forge ghec|example-github]
            [--upgrade] [--adopt-groups <file>]
            [--upstream-remote-name <name>] [--upstream-branch <branch>]
            [--contrib-remote-name <name>] [--internal-branch <branch>]
git uplink add --title <text> [--message <text> | --message-file <path>]
            [--from <ref>] [--head <ref>] [--internal-only]
            [--pr <n>] [--pr-url <url>] [--depends-on <id>]...
git uplink push [--push-remote <remote>]
git uplink reset
git uplink preflight [<id>] [--from <ref>] [--head <ref>] [--title <text>]
            [--message <text> | --message-file <path>]
            [--depends-on <id>]...
git uplink assess [--from <ref>] [--head <ref>] [--title <text>]
            [--message <text> | --message-file <path>]
            [--internal-only]
git uplink report <id> [--out <file>] [--extra-dir <path>]
git uplink status [--json]
git uplink approve <id> [--out <file>]
git uplink submit <id>
git uplink submitted <id> --pr-url <url> [--pr <n>] [--push-remote origin]
git uplink sync
git uplink accept-upstream
git uplink gated <id> --pr-url <url> [--pr <n>] [--push-remote origin]
git uplink merged <id> [--via pr|trailer|patch-id|empty-rebase|manual] [--sha <sha>]
git uplink drop <id> [--reason <text>]
git uplink rebuild [--branch <name>] [--push] [--push-remote <remote>]
git uplink resolve <id>
git uplink transfer <id> --to-upstream|--to-internal [--complete]
git uplink web-ui [--port 43721] [--bind 127.0.0.1] [--no-open]`}</pre>
          </CardContent>
        </Card>

        <p className="text-sm text-muted-foreground">
          Full notes live in the{" "}
          <a href={`${GITHUB_BLOB}/README.md`} className="text-primary underline-offset-4 hover:underline">
            README
          </a>
          . After install,{" "}
          <Link to="/setup" className="text-primary underline-offset-4 hover:underline">
            forge packs
          </Link>{" "}
          and the{" "}
          <Link to="/examples" className="text-primary underline-offset-4 hover:underline">
            GitHub example
          </Link>{" "}
          walk through a product repo.
        </p>
        <Button asChild variant="outline" size="sm">
          <a href={GITHUB_REPO}>Source on GitHub</a>
        </Button>
      </article>
    </AppShell>
  );
}
