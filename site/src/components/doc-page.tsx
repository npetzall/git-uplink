import type React from "react";
import { AppShell } from "./app-shell";
import { cn } from "../lib/utils";

const PROSE =
  "text-[15px] leading-7 text-muted-foreground [&_code]:rounded [&_code]:bg-muted [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:text-[13px] [&_code]:text-foreground [&_pre_code]:bg-transparent [&_pre_code]:p-0 [&_li]:ms-5 [&_ol]:space-y-2 [&_ol>li]:list-decimal [&_p]:text-pretty [&_strong]:text-foreground [&_ul]:space-y-2 [&_ul>li]:list-disc [&_a]:text-primary [&_a]:underline-offset-4 hover:[&_a]:underline";

/** Shared layout for every documentation page: one width, one header style. */
export function DocPage({
  eyebrow,
  title,
  lead,
  children,
}: {
  eyebrow: string;
  title?: string;
  lead?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <AppShell>
      <article className="mx-auto w-full max-w-4xl space-y-10">
        <header className="space-y-3">
          <p className="text-xs font-medium tracking-[0.25em] text-teal-400 uppercase">{eyebrow}</p>
          {title ? <h1 className="text-4xl font-semibold tracking-tight text-balance">{title}</h1> : null}
          {lead ? (
            <p className="text-lg leading-8 text-pretty text-muted-foreground [&_code]:rounded [&_code]:bg-muted [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:text-[15px] [&_code]:text-foreground">
              {lead}
            </p>
          ) : null}
        </header>
        {children}
      </article>
    </AppShell>
  );
}

export function Section({
  id,
  title,
  children,
  className,
}: {
  id?: string;
  title: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section id={id} className={cn("scroll-mt-20 space-y-4", PROSE, className)}>
      <h2 className="text-xl font-semibold text-foreground">
        {id ? (
          <a href={`#${id}`} className="!text-foreground !no-underline">
            {title}
          </a>
        ) : (
          title
        )}
      </h2>
      {children}
    </section>
  );
}

export function Code({ children }: { children: string }) {
  return (
    <pre className="overflow-x-auto rounded-lg bg-black/40 p-4 font-mono text-xs leading-6 text-zinc-200">
      {children}
    </pre>
  );
}
