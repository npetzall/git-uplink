import { useEffect, useState } from "react";
import { NavLink, useLocation } from "react-router-dom";
import { ExternalLink, GitFork, Menu, X } from "lucide-react";
import { Button } from "./ui/button";
import { cn } from "../lib/utils";
import { GITHUB_REPO } from "../lib/links";

const LINKS = [
  { href: "/", label: "Overview" },
  { href: "/why", label: "Why" },
  { href: "/how", label: "How" },
  { href: "/day-to-day", label: "Day to day" },
  { href: "/internals", label: "Internals" },
  { href: "/security", label: "Security" },
  { href: "/lab", label: "Lab" },
  { href: "/install", label: "Install" },
  { href: "/cli", label: "CLI" },
  { href: "/examples", label: "Try it yourself" },
  { href: "/setup", label: "Production setup" },
];

function NavLinks({ onClick }: { onClick?: () => void }) {
  return (
    <>
      {LINKS.map((link) => (
        <NavLink
          key={link.href}
          to={link.href}
          end={link.href === "/"}
          onClick={onClick}
          className={({ isActive }) =>
            cn(
              "rounded-md px-2 py-2 text-sm transition-colors",
              isActive
                ? "bg-primary/15 text-primary"
                : "text-muted-foreground hover:bg-muted hover:text-foreground",
            )
          }
        >
          {link.label}
        </NavLink>
      ))}
    </>
  );
}

/**
 * Scrolls to `#fragment` once the page has rendered it. Links such as the
 * assess report's `/day-to-day#assess-fails` load before the section exists,
 * so the browser's own fragment scroll misses it.
 */
function useScrollToHash() {
  const { pathname, hash } = useLocation();
  useEffect(() => {
    const id = decodeURIComponent(hash.slice(1));
    if (!id) return;
    let frame = 0;
    let tries = 0;
    const attempt = () => {
      const target = document.getElementById(id);
      if (target) {
        target.scrollIntoView();
      } else if (tries++ < 30) {
        frame = requestAnimationFrame(attempt);
      }
    };
    attempt();
    return () => cancelAnimationFrame(frame);
  }, [pathname, hash]);
}

export function AppShell({ children }: { children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
  useScrollToHash();
  return (
    <div className="flex min-h-full flex-col">
      <header className="sticky top-0 z-20 border-b border-border/80 bg-background/80 backdrop-blur-md">
        <div className="mx-auto flex h-14 w-full max-w-7xl items-center justify-between px-4">
          <NavLink to="/" className="flex items-center gap-2 font-semibold tracking-tight">
            <span className="flex size-8 items-center justify-center rounded-md bg-primary/15 text-primary">
              <GitFork className="size-4" />
            </span>
            git uplink
          </NavLink>
          <nav className="hidden items-center gap-0.5 xl:flex">
            <NavLinks />
            <a
              href={GITHUB_REPO}
              className="ms-2 rounded-md px-3 py-2 text-sm text-muted-foreground hover:bg-muted hover:text-foreground"
            >
              GitHub
            </a>
          </nav>
          <Button
            variant="ghost"
            size="icon"
            className="xl:hidden"
            onClick={() => setOpen((value) => !value)}
            aria-label={open ? "Close menu" : "Open menu"}
          >
            {open ? <X /> : <Menu />}
          </Button>
        </div>
        {open ? (
          <nav className="flex flex-col gap-1 border-t border-border px-4 py-3 xl:hidden">
            <NavLinks onClick={() => setOpen(false)} />
            <a
              href={GITHUB_REPO}
              className="rounded-md px-3 py-2 text-sm text-muted-foreground hover:bg-muted hover:text-foreground"
            >
              GitHub
            </a>
          </nav>
        ) : null}
      </header>
      <div className="mx-auto flex w-full max-w-7xl flex-1 flex-col px-4 py-6 md:py-10">{children}</div>
      <footer className="border-t border-border/80">
        <div className="mx-auto flex w-full max-w-7xl flex-wrap items-center gap-x-6 gap-y-2 px-4 py-4 text-sm text-muted-foreground">
          <a href={GITHUB_REPO} className="inline-flex items-center gap-1 hover:text-foreground">
            GitHub <ExternalLink className="size-3" />
          </a>
          <NavLink to="/day-to-day" className="hover:text-foreground">
            Day to day
          </NavLink>
          <span>
            Local queue UI:{" "}
            <code className="rounded bg-muted px-1.5 py-0.5 text-foreground">git uplink web-ui</code>
          </span>
        </div>
      </footer>
    </div>
  );
}
