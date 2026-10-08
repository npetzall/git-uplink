import { Link } from "react-router-dom";
import { DocPage } from "../components/doc-page";
import { Button } from "../components/ui/button";
import { GITHUB_BLOB, GITHUB_REPO } from "../lib/links";

const code = "rounded bg-muted px-1.5 py-0.5 text-foreground";
const pre = "overflow-x-auto rounded-lg bg-black/40 p-4 font-mono text-xs leading-6 text-zinc-200";
const link = "text-primary underline-offset-4 hover:underline";

export function InstallPage() {
  return (
    <DocPage
      eyebrow="Install"
      title="Get git uplink on PATH"
      lead={
        <>
          The binary is <code>git-uplink</code>, so Git treats it as <code>git uplink</code>. Download a release
          binary for your platform and put it on <code>PATH</code>.
        </>
      }
    >

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">macOS and Linux</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            With the <a href="https://cli.github.com/" className={link}>GitHub CLI</a>: download, verify the checksum
            and the <a href="https://docs.sigstore.dev/cosign/system_config/installation/" className={link}>cosign</a> signature,
            make it executable, and move it onto <code className={code}>PATH</code>.
          </p>
          <pre className={pre}>{`target=aarch64-apple-darwin   # or x86_64-unknown-linux-musl, aarch64-unknown-linux-musl
gh release download --repo npetzall/git-uplink --pattern "git-uplink-$target" --pattern "git-uplink-$target.sigstore.json" --pattern SHA256SUMS
grep " git-uplink-$target\\$" SHA256SUMS | shasum -a 256 -c
cosign verify-blob "git-uplink-$target" --bundle "git-uplink-$target.sigstore.json" \\
  --certificate-identity https://github.com/npetzall/git-uplink/.github/workflows/release.yml@refs/heads/main \\
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
chmod +x "git-uplink-$target"
mkdir -p ~/.local/bin && mv "git-uplink-$target" ~/.local/bin/git-uplink
git uplink version`}</pre>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Make sure <code className={code}>~/.local/bin</code> is on your <code className={code}>PATH</code>. Without{" "}
            <code className={code}>gh</code>, download the same files from{" "}
            <a href={`${GITHUB_REPO}/releases`} className={link}>Releases</a>.
          </p>
        </section>

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">Windows</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Download <code className={code}>git-uplink-x86_64-pc-windows-msvc.exe</code> from{" "}
            <a href={`${GITHUB_REPO}/releases`} className={link}>Releases</a>, rename it to{" "}
            <code className={code}>git-uplink.exe</code>, and place it in a directory on{" "}
            <code className={code}>PATH</code>.
          </p>
        </section>

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">Check it</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Use <code className={code}>git-uplink -h</code> or <code className={code}>git uplink -h</code>. Plain{" "}
            <code className={code}>git uplink --help</code> goes through Git&apos;s man-page path, not clap.{" "}
            <code className={code}>git uplink web-ui</code> opens the local operator dashboard for the checkout you
            start it in (<code className={code}>--no-open</code>, <code className={code}>--port 43721</code>).
          </p>
        </section>

        <section className="space-y-3">
          <h2 className="text-xl font-semibold">Next</h2>
          <p className="text-[15px] leading-7 text-muted-foreground">
            Installing only adds the command. To see it work, follow{" "}
            <Link to="/examples" className={link}>
              Try it yourself
            </Link>{" "}
            on three GitHub repositories. To wire up a real product repository, go to{" "}
            <Link to="/setup" className={link}>
              Production setup
            </Link>
            . Every command and flag is in the{" "}
            <Link to="/cli" className={link}>
              CLI reference
            </Link>
            . Building from source is in{" "}
            <a href={`${GITHUB_BLOB}/CONTRIBUTING.md`} className={link}>
              CONTRIBUTING.md
            </a>
            .
          </p>
        </section>

        <Button asChild variant="outline" size="sm">
          <a href={GITHUB_REPO}>Source on GitHub</a>
        </Button>
    </DocPage>
  );
}
