import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type React from "react";

/** One input on a setup guide's values form. `token` is the placeholder shown until it is filled. */
export type Field = { key: string; label: string; token: string; initial: string; help?: string };

type Values = Record<string, string>;
/** Extra template names computed from the filled values. `get` returns a value or its `<TOKEN>`. */
export type Derive = (get: (key: string) => string, values: Values) => Record<string, string>;

function load(fields: Field[], storageKey: string): Values {
  const base = Object.fromEntries(fields.map((f) => [f.key, f.initial]));
  try {
    const raw = window.localStorage.getItem(storageKey);
    if (raw) return { ...base, ...(JSON.parse(raw) as Values) };
  } catch {
    // Storage may be unavailable (private window); the form still works for this visit.
  }
  return base;
}

type Ctx = { fields: Field[]; derive?: Derive; values: Values; set: (key: string, value: string) => void };

const SetupValues = createContext<Ctx | null>(null);

export function SetupValuesProvider({
  fields,
  storageKey,
  derive,
  children,
}: {
  fields: Field[];
  storageKey: string;
  derive?: Derive;
  children: React.ReactNode;
}) {
  const [values, setValues] = useState<Values>(() => load(fields, storageKey));
  useEffect(() => {
    try {
      window.localStorage.setItem(storageKey, JSON.stringify(values));
    } catch {
      // Ignore: values just won't survive a reload.
    }
  }, [values, storageKey]);
  const ctx = useMemo(
    () => ({
      fields,
      derive,
      values,
      set: (key: string, value: string) => setValues((v) => ({ ...v, [key]: value.trim() })),
    }),
    [fields, derive, values],
  );
  return <SetupValues.Provider value={ctx}>{children}</SetupValues.Provider>;
}

function useCtx() {
  const ctx = useContext(SetupValues);
  if (!ctx) throw new Error("SetupValuesProvider missing");
  return ctx;
}

/** Resolve `{{name}}` templates. Empty values render as visible `<TOKEN>` placeholders. */
export function useFill(): (template: string) => string {
  const { fields, derive, values } = useCtx();
  return useMemo(() => {
    const token = Object.fromEntries(fields.map((f) => [f.key, f.token]));
    const get = (key: string) => values[key] || `<${token[key]}>`;
    const derived = derive ? derive(get, values) : {};
    return (template: string) =>
      template.replace(/\{\{(\w+)\}\}/g, (_, name: string) =>
        name in derived ? derived[name] : name in token ? get(name) : `{{${name}}}`,
      );
  }, [fields, derive, values]);
}

export function SetupValuesForm() {
  const { fields, values, set } = useCtx();
  return (
    <details open className="rounded-xl border border-border bg-card p-4">
      <summary className="cursor-pointer text-sm font-medium text-foreground">
        Your values{" "}
        <span className="font-normal text-muted-foreground">
          — commands below update as you type. Stored only in this browser.
        </span>
      </summary>
      <div className="mt-4 grid gap-3 sm:grid-cols-2">
        {fields.map((field) => (
          <label key={field.key} className="space-y-1 text-sm">
            <span className="text-foreground">{field.label}</span>
            <input
              value={values[field.key] ?? ""}
              onChange={(e) => set(field.key, e.target.value)}
              placeholder={field.token}
              spellCheck={false}
              className="w-full rounded-md border border-border bg-background px-2.5 py-1.5 font-mono text-xs text-foreground placeholder:text-muted-foreground/60 focus-visible:outline-2 focus-visible:outline-primary"
            />
            {field.help ? <span className="block text-xs text-muted-foreground">{field.help}</span> : null}
          </label>
        ))}
      </div>
    </details>
  );
}
