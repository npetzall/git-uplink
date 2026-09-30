import { createContext, useContext, useState } from "react";
import type React from "react";
import { Check, Copy } from "lucide-react";
import { cn } from "../../lib/utils";
import { useFill } from "./values";

export type Mode = "manual" | "gh";

export const ModeContext = createContext<Mode>("manual");

export function ModeToggle({ mode, onChange }: { mode: Mode; onChange: (mode: Mode) => void }) {
  const options: [Mode, string][] = [
    ["manual", "Manual (web UI)"],
    ["gh", "gh CLI"],
  ];
  return (
    <div role="radiogroup" aria-label="How to run each step" className="inline-flex rounded-lg border border-border p-0.5">
      {options.map(([value, label]) => (
        <button
          key={value}
          type="button"
          role="radio"
          aria-checked={mode === value}
          onClick={() => onChange(value)}
          className={cn(
            "rounded-md px-3 py-1.5 text-sm transition-colors focus-visible:outline-2 focus-visible:outline-primary",
            mode === value ? "bg-primary/15 text-primary" : "text-muted-foreground hover:text-foreground",
          )}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

/** A copyable code block. `{{name}}` placeholders are filled from the reader's values. */
export function Command({ children }: { children: string }) {
  const fill = useFill();
  const text = fill(children.trim());
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard can be blocked; the text is still selectable.
    }
  }
  return (
    <div className="relative">
      <pre className="overflow-x-auto rounded-lg bg-black/40 p-4 pr-12 font-mono text-xs leading-6 text-zinc-200">{text}</pre>
      <button
        type="button"
        onClick={copy}
        aria-label={copied ? "Copied" : "Copy"}
        className="absolute top-2 right-2 rounded-md p-1.5 text-zinc-400 hover:bg-white/10 hover:text-zinc-100"
      >
        {copied ? <Check className="size-4" /> : <Copy className="size-4" />}
      </button>
    </div>
  );
}

/** Inline value from the reader's form, e.g. <V>{"{{companyRepo}}"}</V>. */
export function V({ children }: { children: string }) {
  const fill = useFill();
  return <code>{fill(children)}</code>;
}

export function Step({
  n,
  id,
  title,
  manual,
  gh,
  alternative,
}: {
  n: number;
  id: string;
  title: string;
  manual: React.ReactNode;
  gh?: React.ReactNode;
  alternative?: { summary: string; body: React.ReactNode };
}) {
  const mode = useContext(ModeContext);
  const body = mode === "gh" && gh ? gh : manual;
  return (
    <section
      id={id}
      className="scroll-mt-20 space-y-4 border-l-2 border-border pl-5 text-[15px] leading-7 text-muted-foreground [&_a]:text-primary [&_a]:underline-offset-4 hover:[&_a]:underline [&_code]:rounded [&_code]:bg-muted [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:text-[13px] [&_code]:text-foreground [&_pre_code]:bg-transparent [&_ol]:space-y-2 [&_ol>li]:ms-5 [&_ol>li]:list-decimal [&_strong]:text-foreground [&_ul]:space-y-2 [&_ul>li]:ms-5 [&_ul>li]:list-disc [&_table]:w-full [&_table]:text-sm [&_td]:border-b [&_td]:border-border/60 [&_td]:py-2 [&_td]:pr-3 [&_td]:align-top [&_th]:border-b [&_th]:py-2 [&_th]:pr-3 [&_th]:text-left [&_th]:text-xs [&_th]:font-medium [&_th]:uppercase"
    >
      <h2 className="flex items-baseline gap-3 text-xl font-semibold text-foreground">
        <span className="font-mono text-sm text-teal-400">{String(n).padStart(2, "0")}</span>
        <a href={`#${id}`} className="!text-foreground !no-underline">
          {title}
        </a>
      </h2>
      {mode === "gh" && !gh ? (
        <p className="rounded-lg border border-amber-500/20 bg-amber-500/5 px-3 py-2 text-sm text-amber-100/90">
          No gh command for this step. Use the web UI.
        </p>
      ) : null}
      {body}
      {alternative ? (
        <details className="rounded-lg border border-border px-4 py-2">
          <summary className="cursor-pointer text-sm text-foreground">{alternative.summary}</summary>
          <div className="mt-3 space-y-3">{alternative.body}</div>
        </details>
      ) : null}
    </section>
  );
}
