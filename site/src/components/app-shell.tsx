import { useState } from "react";
import { NavLink } from "react-router-dom";
import { ExternalLink, GitFork, Menu, X } from "lucide-react";
import { Button } from "./ui/button";
import { cn } from "../lib/utils";
import { GITHUB_REPO } from "../lib/links";

const LINKS = [
  { href: "/", label: "Overview" },
  { href: "/lab", label: "Lab" },
  { href: "/working", label: "Way of working" },
  { href: "/playbook", label: "Playbook" },
  { href: "/internals", label: "Internals" },
  { href: "/collaboration", label: "Collaboration" },
  { href: "/install", label: "Install" },
  { href: "/setup", label: "Forge packs" },
  { href: "/examples", label: "Examples" },
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

export function AppShell({ children }: { children: React.ReactNode }) {
  const [open, setOpen] = useState(false);
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
          <NavLink to="/working" className="hover:text-foreground">
            Way of working
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
