import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type React from "react";

/** Values the reader fills in once; every command on the page is rendered from them. */
export const FIELDS = [
  { key: "host", label: "Company GitHub host", token: "COMPANY_HOST", initial: "github.com", help: "github.com, or your GHE.com subdomain" },
  { key: "companyOrg", label: "Company organization", token: "COMPANY_ORG", initial: "" },
  { key: "productRepo", label: "Product repository", token: "PRODUCT_REPO", initial: "" },
  { key: "upstreamUrl", label: "Public upstream URL", token: "UPSTREAM_URL", initial: "", help: "https://github.com/owner/project.git" },
  { key: "contribOrg", label: "Contribution fork organization", token: "CONTRIB_ORG", initial: "", help: "On public github.com; same org as upstream to use Apps" },
  { key: "contribRepo", label: "Contribution fork name", token: "CONTRIB_REPO", initial: "" },
  { key: "ipTeam", label: "IP reviewer team (slug)", token: "IP_TEAM", initial: "" },
  { key: "inboundTeam", label: "Inbound reviewer team (slug)", token: "INBOUND_TEAM", initial: "" },
  { key: "abandonTeam", label: "Withdraw reviewer team (slug)", token: "ABANDON_TEAM", initial: "" },
  { key: "internalAppId", label: "Internal App ID", token: "INTERNAL_APP_ID", initial: "", help: "From step 2" },
  { key: "upstreamAppId", label: "Upstream App ID", token: "UPSTREAM_APP_ID", initial: "", help: "From step 2" },
  { key: "contribAppId", label: "Contrib App ID", token: "CONTRIB_APP_ID", initial: "", help: "From step 2" },
  { key: "uplinkVersion", label: "git-uplink release", token: "UPLINK_VERSION", initial: "latest" },
] as const;

type Key = (typeof FIELDS)[number]["key"];
type Values = Record<Key, string>;

const STORAGE_KEY = "uplink-setup-values";

function load(): Values {
  const base = Object.fromEntries(FIELDS.map((f) => [f.key, f.initial])) as Values;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw) return { ...base, ...(JSON.parse(raw) as Partial<Values>) };
  } catch {
    // Storage may be unavailable (private window); the form still works for this visit.
  }
  return base;
}

const Ctx = createContext<{ values: Values; set: (key: Key, value: string) => void } | null>(null);

export function SetupValuesProvider({ children }: { children: React.ReactNode }) {
  const [values, setValues] = useState<Values>(load);
  useEffect(() => {
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(values));
    } catch {
      // Ignore: values just won't survive a reload.
    }
  }, [values]);
  const ctx = useMemo(
    () => ({ values, set: (key: Key, value: string) => setValues((v) => ({ ...v, [key]: value.trim() })) }),
    [values],
  );
  return <Ctx.Provider value={ctx}>{children}</Ctx.Provider>;
}

function useCtx() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("SetupValuesProvider missing");
  return ctx;
}

function upstreamSlug(url: string): string {
  const m = url.match(/github\.com[:/]([^/]+)\/([^/]+?)(\.git)?\/?$/);
  return m ? `${m[1]}/${m[2]}` : "";
}

/** Resolve `{{name}}` templates. Empty values render as visible `<TOKEN>` placeholders. */
export function useFill(): (template: string) => string {
  const { values } = useCtx();
  return useMemo(() => {
    const token = Object.fromEntries(FIELDS.map((f) => [f.key, f.token])) as Record<Key, string>;
    const get = (key: Key) => values[key] || `<${token[key]}>`;
    const derived: Record<string, string> = {
      companyRepo: `${get("companyOrg")}/${get("productRepo")}`,
      ghRepo:
        (values.host && values.host !== "github.com" ? `${values.host}/` : "") +
        `${get("companyOrg")}/${get("productRepo")}`,
      companyUrl: `https://${get("host")}/${get("companyOrg")}/${get("productRepo")}.git`,
      contribUrl: `https://github.com/${get("contribOrg")}/${get("contribRepo")}.git`,
      upstreamSlug: upstreamSlug(values.upstreamUrl) || "<UPSTREAM_OWNER>/<UPSTREAM_REPO>",
    };
    return (template: string) =>
      template.replace(/\{\{(\w+)\}\}/g, (_, name: string) =>
        name in derived ? derived[name] : name in token ? get(name as Key) : `{{${name}}}`,
      );
  }, [values]);
}

export function SetupValuesForm() {
  const { values, set } = useCtx();
  return (
    <details open className="rounded-xl border border-border bg-card p-4">
      <summary className="cursor-pointer text-sm font-medium text-foreground">
        Your values <span className="font-normal text-muted-foreground">— commands below update as you type. Stored only in this browser.</span>
      </summary>
      <div className="mt-4 grid gap-3 sm:grid-cols-2">
        {FIELDS.map((field) => (
          <label key={field.key} className="space-y-1 text-sm">
            <span className="text-foreground">{field.label}</span>
            <input
              value={values[field.key]}
              onChange={(e) => set(field.key, e.target.value)}
              placeholder={field.token}
              spellCheck={false}
              className="w-full rounded-md border border-border bg-background px-2.5 py-1.5 font-mono text-xs text-foreground placeholder:text-muted-foreground/60 focus-visible:outline-2 focus-visible:outline-primary"
            />
            {"help" in field ? <span className="block text-xs text-muted-foreground">{field.help}</span> : null}
          </label>
        ))}
      </div>
    </details>
  );
}
