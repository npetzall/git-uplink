import { useState } from "react";
import { NavLink } from "react-router-dom";
import { GitFork, Menu, X } from "lucide-react";
import { Button } from "./ui/button";
import { cn } from "../lib/utils";

const LINKS = [
  { href: "/", label: "Control room" },
  { href: "/queue", label: "This repo" },
  { href: "/collaboration", label: "Collaboration" },
  { href: "/lab", label: "Live lab" },
  { href: "/working", label: "Way of working" },
  { href: "/playbook", label: "System playbook" },
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
              "rounded-md px-3 py-2 text-sm transition-colors",
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
          <nav className="hidden items-center gap-1 md:flex">
            <NavLinks />
          </nav>
          <Button
            variant="ghost"
            size="icon"
            className="md:hidden"
            onClick={() => setOpen((value) => !value)}
            aria-label={open ? "Close menu" : "Open menu"}
          >
            {open ? <X /> : <Menu />}
          </Button>
        </div>
        {open ? (
          <nav className="flex flex-col gap-1 border-t border-border px-4 py-3 md:hidden">
            <NavLinks onClick={() => setOpen(false)} />
          </nav>
        ) : null}
      </header>
      <div className="mx-auto flex w-full max-w-7xl flex-1 flex-col px-4 py-6 md:py-10">{children}</div>
    </div>
  );
}
