import type React from "react";
import { ModeContext, ModeToggle, type Mode } from "./step";
import { SetupValuesForm, SetupValuesProvider, type Derive, type Field } from "./values";

/** Values form, Manual / gh CLI toggle, and the steps of one setup guide. */
export function SetupGuide({
  fields,
  storageKey,
  derive,
  mode,
  onMode,
  intro,
  children,
}: {
  fields: Field[];
  storageKey: string;
  derive?: Derive;
  mode: Mode;
  onMode: (mode: Mode) => void;
  intro: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <SetupValuesProvider fields={fields} storageKey={storageKey} derive={derive}>
      <ModeContext.Provider value={mode}>
        <div className="space-y-8">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="text-sm text-muted-foreground [&_code]:rounded [&_code]:bg-muted [&_code]:px-1.5 [&_code]:py-0.5 [&_code]:text-[13px] [&_code]:text-foreground">
              {intro}
            </p>
            <ModeToggle mode={mode} onChange={onMode} />
          </div>
          <SetupValuesForm />
          {children}
        </div>
      </ModeContext.Provider>
    </SetupValuesProvider>
  );
}
