import { Fragment, useMemo, useState, type ReactNode } from "react";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { cn } from "../lib/utils";
import {
  deltaLineSide,
  parseFileDelta,
  parsePatch,
  patchLineSide,
  splitRows,
  type DeltaLine,
  type DeltaState,
  type DiffLine,
  type FileDelta,
  type FileStatus,
  type PatchFile,
} from "../lib/patch";

export type DiffLayout = "unified" | "split";

const LAYOUT_KEY = "uplink.diff.layout";

/** The diff layout, remembered in this browser. */
export function useDiffLayout(): [DiffLayout, (layout: DiffLayout) => void] {
  const [layout, setLayout] = useState<DiffLayout>(() => {
    try {
      return localStorage.getItem(LAYOUT_KEY) === "split" ? "split" : "unified";
    } catch {
      return "unified";
    }
  });
  const choose = (next: DiffLayout) => {
    setLayout(next);
    try {
      localStorage.setItem(LAYOUT_KEY, next);
    } catch {
      // Storage is blocked: the choice lasts until reload.
    }
  };
  return [layout, choose];
}

export function LayoutToggle({
  layout,
  onChange,
}: {
  layout: DiffLayout;
  onChange: (layout: DiffLayout) => void;
}) {
  return (
    <div className="inline-flex rounded-lg border border-border p-0.5" role="group" aria-label="Diff layout">
      {(["unified", "split"] as const).map((value) => (
        <Button
          key={value}
          size="sm"
          variant={layout === value ? "secondary" : "ghost"}
          aria-pressed={layout === value}
          className="capitalize"
          onClick={() => onChange(value)}
        >
          {value}
        </Button>
      ))}
    </div>
  );
}

const GREEN = "border-emerald-500/30 bg-emerald-500/10 text-emerald-300";
const RED = "border-rose-500/30 bg-rose-500/10 text-rose-300";
const AMBER = "border-amber-500/30 bg-amber-500/10 text-amber-300";
const GREY = "border-zinc-500/30 bg-zinc-500/10 text-zinc-300";

const STATUS_STYLE: Record<FileStatus, string> = {
  added: GREEN,
  deleted: RED,
  renamed: AMBER,
  copied: AMBER,
  mode: GREY,
  modified: GREY,
};

const TABLE = "w-full border-collapse font-mono text-xs leading-5";
const NUM = "w-12 px-2 text-right align-top text-muted-foreground/60 select-none";
const MARK = "w-5 text-center align-top select-none";
const EMPTY_SIDE = "bg-muted/40";

function FileSection({
  path,
  badge,
  badgeStyle,
  counts,
  children,
}: {
  path: ReactNode;
  badge: string;
  badgeStyle: string;
  counts?: ReactNode;
  children: ReactNode;
}) {
  return (
    <details open className="overflow-hidden rounded-md border border-border">
      <summary className="flex cursor-pointer flex-wrap items-center gap-2 bg-muted px-3 py-2 text-sm">
        <span className="font-mono text-xs break-all">{path}</span>
        <Badge className={badgeStyle}>{badge}</Badge>
        {counts ? <span className="ml-auto font-mono text-xs">{counts}</span> : null}
      </summary>
      <div className="overflow-x-auto">{children}</div>
    </details>
  );
}

const LINE_STYLE: Record<DiffLine["kind"], string> = {
  add: "bg-emerald-500/10 text-emerald-200",
  del: "bg-rose-500/10 text-rose-200",
  ctx: "text-zinc-300",
  meta: "text-muted-foreground italic",
};

const LINE_MARK: Record<DiffLine["kind"], string> = { add: "+", del: "-", ctx: "", meta: "" };

function HunkHeader({ text, span }: { text: string; span: number }) {
  return (
    <tr className="bg-sky-500/10 text-sky-300">
      <td colSpan={span} className="px-3 whitespace-pre">
        {text}
      </td>
    </tr>
  );
}

/** One column of line numbers when the file exists on one side only. */
function UnifiedHunks({ file }: { file: PatchFile }) {
  const showOld = file.status !== "added";
  const showNew = file.status !== "deleted";
  const span = 2 + Number(showOld) + Number(showNew);
  return (
    <table className={TABLE}>
      <tbody>
        {file.hunks.map((hunk, index) => (
          <Fragment key={index}>
            <HunkHeader text={hunk.header} span={span} />
            {hunk.lines.map((line, row) => (
              <tr key={row} className={LINE_STYLE[line.kind]}>
                {showOld ? <td className={NUM}>{line.oldNo}</td> : null}
                {showNew ? <td className={NUM}>{line.newNo}</td> : null}
                <td className={MARK}>{LINE_MARK[line.kind]}</td>
                <td className="w-full pr-3 whitespace-pre">{line.text || " "}</td>
              </tr>
            ))}
          </Fragment>
        ))}
      </tbody>
    </table>
  );
}

function SplitCell({ line, number }: { line?: DiffLine; number?: number }) {
  if (!line) {
    return <td colSpan={3} className={EMPTY_SIDE} />;
  }
  return (
    <>
      <td className={cn(NUM, LINE_STYLE[line.kind])}>{number}</td>
      <td className={cn(MARK, LINE_STYLE[line.kind])}>{LINE_MARK[line.kind]}</td>
      <td className={cn("pr-3 break-all whitespace-pre-wrap", LINE_STYLE[line.kind])}>
        {line.text || " "}
      </td>
    </>
  );
}

function SplitHunks({ file }: { file: PatchFile }) {
  return (
    <table className={cn(TABLE, "table-fixed")}>
      <colgroup>
        <col className="w-12" />
        <col className="w-5" />
        <col />
        <col className="w-12" />
        <col className="w-5" />
        <col />
      </colgroup>
      <tbody>
        {file.hunks.map((hunk, index) => (
          <Fragment key={index}>
            <HunkHeader text={hunk.header} span={6} />
            {splitRows(hunk.lines, patchLineSide).map((row, at) => (
              <tr key={at}>
                <SplitCell line={row.left} number={row.left?.oldNo} />
                <SplitCell line={row.right} number={row.right?.newNo} />
              </tr>
            ))}
          </Fragment>
        ))}
      </tbody>
    </table>
  );
}

function PatchFileView({ file, layout }: { file: PatchFile; layout: DiffLayout }) {
  const oneSided = file.status === "added" || file.status === "deleted";
  const moved = file.oldPath && file.newPath && file.oldPath !== file.newPath;
  return (
    <FileSection
      path={moved ? `${file.oldPath} → ${file.newPath}` : file.path}
      badge={file.binary ? `${file.status} · binary` : file.status}
      badgeStyle={STATUS_STYLE[file.status]}
      counts={
        file.binary ? null : (
          <>
            <span className="text-emerald-300">+{file.additions}</span>{" "}
            <span className="text-rose-300">−{file.deletions}</span>
          </>
        )
      }
    >
      {file.meta.length ? (
        <pre className="px-3 py-1 text-xs text-muted-foreground">{file.meta.join("\n")}</pre>
      ) : null}
      {file.binary ? (
        <p className="px-3 py-2 text-xs text-muted-foreground">Binary file, content not shown.</p>
      ) : null}
      {!file.hunks.length ? null : layout === "split" && !oneSided ? (
        <SplitHunks file={file} />
      ) : (
        <UnifiedHunks file={file} />
      )}
    </FileSection>
  );
}

/** A `git format-patch` text as a commit header and one diff per file. */
export function PatchView({ text, layout }: { text: string; layout: DiffLayout }) {
  const patch = useMemo(() => parsePatch(text), [text]);
  if (!text.trim()) {
    return <p className="text-sm text-muted-foreground">(empty)</p>;
  }
  const additions = patch.files.reduce((sum, file) => sum + file.additions, 0);
  const deletions = patch.files.reduce((sum, file) => sum + file.deletions, 0);
  return (
    <div className="space-y-3">
      {patch.subject || patch.message ? (
        <div className="space-y-1 rounded-md bg-muted p-3 text-sm">
          {patch.subject ? <p className="font-medium">{patch.subject}</p> : null}
          {patch.from || patch.date ? (
            <p className="text-xs text-muted-foreground">
              {[patch.from, patch.date].filter(Boolean).join(" · ")}
            </p>
          ) : null}
          {patch.message ? (
            <pre className="pt-1 font-mono text-xs whitespace-pre-wrap">{patch.message}</pre>
          ) : null}
        </div>
      ) : null}
      {patch.files.length ? (
        <p className="text-xs text-muted-foreground">
          {patch.files.length} {patch.files.length === 1 ? "file" : "files"} changed,{" "}
          <span className="text-emerald-300">+{additions}</span>{" "}
          <span className="text-rose-300">−{deletions}</span>
        </p>
      ) : null}
      {patch.files.map((file, index) => (
        <PatchFileView key={`${index}-${file.path}`} file={file} layout={layout} />
      ))}
    </div>
  );
}

const STATE_LABEL: Record<Exclude<DeltaState, "unchanged">, string> = {
  added: "new in this revision",
  removed: "dropped from the patch",
  changed: "changed",
};

const STATE_STYLE: Record<Exclude<DeltaState, "unchanged">, string> = {
  added: GREEN,
  removed: RED,
  changed: AMBER,
};

const OUTER_STYLE: Record<DeltaLine["outer"], string> = {
  added: "bg-emerald-500/15",
  removed: "bg-rose-500/15",
  same: "opacity-60",
};

const OUTER_MARK: Record<DeltaLine["outer"], string> = { added: "+", removed: "-", same: "" };

const INNER_STYLE: Record<DeltaLine["kind"], string> = {
  add: "text-emerald-300",
  del: "text-rose-300",
  meta: "text-muted-foreground italic",
};

const INNER_MARK: Record<DeltaLine["kind"], string> = { add: "+", del: "-", meta: "" };

/** One patch line. In the split layout each side is tinted by itself, not the row. */
function DeltaText({ line, wrap }: { line?: DeltaLine; wrap: boolean }) {
  if (!line) {
    return <td colSpan={2} className={EMPTY_SIDE} />;
  }
  const tint = wrap ? OUTER_STYLE[line.outer] : "";
  return (
    <>
      <td className={cn(MARK, tint, INNER_STYLE[line.kind])}>{INNER_MARK[line.kind]}</td>
      <td
        className={cn(
          "pr-3",
          wrap ? "break-all whitespace-pre-wrap" : "w-full whitespace-pre",
          tint,
          INNER_STYLE[line.kind],
        )}
      >
        {line.text || " "}
      </td>
    </>
  );
}

function UnifiedDelta({ lines }: { lines: DeltaLine[] }) {
  return (
    <table className={TABLE}>
      <tbody>
        {lines.map((line, index) => (
          <tr key={index} className={OUTER_STYLE[line.outer]}>
            <td className={cn(MARK, "border-r border-border font-bold text-foreground")}>
              {OUTER_MARK[line.outer]}
            </td>
            <DeltaText line={line} wrap={false} />
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function SplitDelta({ lines }: { lines: DeltaLine[] }) {
  return (
    <table className={cn(TABLE, "table-fixed")}>
      <colgroup>
        <col className="w-5" />
        <col />
        <col className="w-5" />
        <col />
      </colgroup>
      <tbody>
        {splitRows(lines, deltaLineSide).map((row, index) => (
          <tr key={index}>
            <DeltaText line={row.left} wrap />
            <DeltaText line={row.right} wrap />
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function FileDeltaView({ file, layout }: { file: FileDelta; layout: DiffLayout }) {
  const lines = useMemo(() => parseFileDelta(file), [file]);
  if (file.state === "unchanged") {
    return null;
  }
  return (
    <FileSection path={file.path} badge={STATE_LABEL[file.state]} badgeStyle={STATE_STYLE[file.state]}>
      {layout === "split" && file.state === "changed" ? (
        <SplitDelta lines={lines} />
      ) : (
        <UnifiedDelta lines={lines} />
      )}
    </FileSection>
  );
}

/** What a patch adds and removes in one revision compared with another, file by file. */
export function RevisionDiffView({ files, layout }: { files: FileDelta[]; layout: DiffLayout }) {
  const unchanged = files.filter((file) => file.state === "unchanged").length;
  if (unchanged === files.length) {
    return (
      <p className="text-sm text-muted-foreground">
        Both revisions add and remove the same lines.
      </p>
    );
  }
  return (
    <div className="space-y-3">
      <p className="text-xs text-muted-foreground">
        Only the lines the patch adds and removes are compared, not the unchanged lines around
        them.{" "}
        {layout === "split" ? (
          <>The older revision is on the left, the newer on the right.</>
        ) : (
          <>
            The first column says what happened to a patch line:{" "}
            <span className="rounded bg-emerald-500/15 px-1 font-mono text-foreground">+</span> new
            in the newer revision,{" "}
            <span className="rounded bg-rose-500/15 px-1 font-mono text-foreground">-</span> gone
            from it. Dimmed lines are in both.
          </>
        )}
      </p>
      {files.map((file) => (
        <FileDeltaView key={file.header} file={file} layout={layout} />
      ))}
      {unchanged ? (
        <p className="text-xs text-muted-foreground">
          {unchanged} {unchanged === 1 ? "file is" : "files are"} the same in both revisions.
        </p>
      ) : null}
    </div>
  );
}
