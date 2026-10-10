export type LineKind = "add" | "del" | "ctx" | "meta";

export type DiffLine = {
  kind: LineKind;
  text: string;
  oldNo?: number;
  newNo?: number;
};

export type Hunk = {
  header: string;
  lines: DiffLine[];
};

export type FileStatus = "added" | "deleted" | "renamed" | "copied" | "mode" | "modified";

export type PatchFile = {
  /** The path to show: the new one, or the old one of a deleted file. */
  path: string;
  oldPath?: string;
  newPath?: string;
  status: FileStatus;
  binary: boolean;
  /** File-level lines such as `new file mode 100644`. */
  meta: string[];
  hunks: Hunk[];
  additions: number;
  deletions: number;
};

export type ParsedPatch = {
  from?: string;
  date?: string;
  subject?: string;
  message: string;
  files: PatchFile[];
};

const HEADER = /^(From|Date|Subject): (.*)$/;
const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;
const META = [
  "new file mode ",
  "deleted file mode ",
  "old mode ",
  "new mode ",
  "similarity index ",
  "rename from ",
  "rename to ",
  "copy from ",
  "copy to ",
];

/** Where the diff starts: the first `diff --git` line after the `---` that ends the message. */
function diffStart(lines: string[]): number {
  const separator = lines.indexOf("---");
  const start = lines.findIndex(
    (line, index) => index > separator && line.startsWith("diff --git "),
  );
  return start === -1 ? lines.length : start;
}

function parsePreamble(lines: string[], patch: ParsedPatch) {
  let index = 0;
  // The mbox line: `From <sha> Mon Sep 17 00:00:00 2001`.
  if (/^From [0-9a-f]+ /.test(lines[0] ?? "")) {
    index = 1;
  }
  let last: "from" | "date" | "subject" | undefined;
  for (; index < lines.length; index += 1) {
    const line = lines[index];
    const header = HEADER.exec(line);
    if (header) {
      last = header[1].toLowerCase() as "from" | "date" | "subject";
      patch[last] = header[2];
    } else if (last && /^[ \t]/.test(line)) {
      // A folded header line.
      patch[last] = `${patch[last]} ${line.trim()}`;
    } else if (/^[A-Za-z-]+: /.test(line)) {
      last = undefined;
    } else {
      break;
    }
  }
  patch.subject = patch.subject?.replace(/^\[PATCH[^\]]*\] /, "");
  const separator = lines.indexOf("---");
  const end = separator === -1 ? lines.length : separator;
  patch.message = lines.slice(index, Math.max(index, end)).join("\n").trim();
}

/** `a/x b/x` in a `diff --git` line. Only splits cleanly when neither path was renamed. */
function headerPaths(header: string): [string, string] {
  const names = header.slice("diff --git ".length);
  const half = (names.length - 1) / 2;
  if (Number.isInteger(half) && names[half] === " ") {
    const left = names.slice(0, half);
    const right = names.slice(half + 1);
    if (left.slice(2) === right.slice(2)) {
      return [left.slice(2), right.slice(2)];
    }
  }
  const split = names.lastIndexOf(" b/");
  if (split === -1) {
    return [names, names];
  }
  return [names.slice(2, split), names.slice(split + 3)];
}

function sidePath(line: string): string | undefined {
  const name = line.slice(4).replace(/\t.*$/, "");
  return name === "/dev/null" ? undefined : name.replace(/^[ab]\//, "");
}

function newFile(header: string): PatchFile {
  const [oldPath, newPath] = headerPaths(header);
  return {
    path: newPath,
    oldPath,
    newPath,
    status: "modified",
    binary: false,
    meta: [],
    hunks: [],
    additions: 0,
    deletions: 0,
  };
}

function finishFile(file: PatchFile) {
  const has = (prefix: string) => file.meta.some((line) => line.startsWith(prefix));
  if (has("new file mode ")) {
    file.status = "added";
    file.oldPath = undefined;
  } else if (has("deleted file mode ")) {
    file.status = "deleted";
    file.newPath = undefined;
  } else if (has("rename to ")) {
    file.status = "renamed";
  } else if (has("copy to ")) {
    file.status = "copied";
  } else if (has("new mode ") && !file.hunks.length && !file.binary) {
    file.status = "mode";
  }
  file.path = file.newPath ?? file.oldPath ?? file.path;
}

/**
 * Parses `git format-patch` text (or a bare `git diff`) into the commit
 * header and one entry per file. Text without a diff gives no files and the
 * whole text as the message.
 */
export function parsePatch(text: string): ParsedPatch {
  const lines = text.split("\n");
  const start = diffStart(lines);
  const patch: ParsedPatch = { message: "", files: [] };
  parsePreamble(lines.slice(0, start), patch);

  let file: PatchFile | undefined;
  let hunk: Hunk | undefined;
  // Lines left in the current hunk, from its `@@` counts. Counting keeps a
  // content line that looks like `diff --git` or the mbox signature inside.
  let oldLeft = 0;
  let newLeft = 0;
  let oldNo = 0;
  let newNo = 0;
  let inBinary = false;

  for (const line of lines.slice(start)) {
    if (oldLeft > 0 || newLeft > 0) {
      const mark = line[0];
      if (mark === "+" && newLeft > 0) {
        hunk!.lines.push({ kind: "add", text: line.slice(1), newNo });
        file!.additions += 1;
        newNo += 1;
        newLeft -= 1;
        continue;
      }
      if (mark === "-" && oldLeft > 0) {
        hunk!.lines.push({ kind: "del", text: line.slice(1), oldNo });
        file!.deletions += 1;
        oldNo += 1;
        oldLeft -= 1;
        continue;
      }
      // A mail client may have stripped the single space of an empty context line.
      if ((mark === " " || line === "") && oldLeft > 0 && newLeft > 0) {
        hunk!.lines.push({ kind: "ctx", text: line.slice(1), oldNo, newNo });
        oldNo += 1;
        newNo += 1;
        oldLeft -= 1;
        newLeft -= 1;
        continue;
      }
      if (mark === "\\") {
        hunk!.lines.push({ kind: "meta", text: line });
        continue;
      }
      // The hunk is shorter than its header says.
      oldLeft = 0;
      newLeft = 0;
    }
    if (line.startsWith("diff --git ")) {
      if (file) {
        finishFile(file);
      }
      file = newFile(line);
      patch.files.push(file);
      hunk = undefined;
      inBinary = false;
      continue;
    }
    if (!file || inBinary) {
      continue;
    }
    const counts = HUNK.exec(line);
    if (counts) {
      oldNo = Number(counts[1]);
      oldLeft = counts[2] === undefined ? 1 : Number(counts[2]);
      newNo = Number(counts[3]);
      newLeft = counts[4] === undefined ? 1 : Number(counts[4]);
      hunk = { header: line, lines: [] };
      file.hunks.push(hunk);
    } else if (line.startsWith("\\") && hunk) {
      hunk.lines.push({ kind: "meta", text: line });
    } else if (hunk) {
      // Between hunks only `@@` and `diff --git` matter; the rest is the mbox signature.
    } else if (line === "GIT binary patch" || line.startsWith("Binary files ")) {
      file.binary = true;
      inBinary = true;
    } else if (line.startsWith("--- ")) {
      file.oldPath = sidePath(line);
    } else if (line.startsWith("+++ ")) {
      file.newPath = sidePath(line);
    } else if (META.some((prefix) => line.startsWith(prefix))) {
      file.meta.push(line);
      if (line.startsWith("rename from ") || line.startsWith("copy from ")) {
        file.oldPath = line.slice(line.indexOf(" from ") + 6);
      } else if (line.startsWith("rename to ") || line.startsWith("copy to ")) {
        file.newPath = line.slice(line.indexOf(" to ") + 4);
      }
    }
  }
  if (file) {
    finishFile(file);
  }
  if (!patch.files.length && !patch.subject) {
    patch.message = text.trim();
  }
  return patch;
}

export type DeltaState = "added" | "removed" | "changed" | "unchanged";

/** One file of `GET /api/patches/{id}/diff`. */
export type FileDelta = {
  path: string;
  header: string;
  state: DeltaState;
  diff?: string;
  lines?: string;
};

/** Whether a line is in both revisions, only the newer, or only the older. */
export type Outer = "same" | "added" | "removed";

export type DeltaLine = {
  outer: Outer;
  /** What the patch line itself does: `meta` is a file-level line. */
  kind: "add" | "del" | "meta";
  text: string;
};

function innerLine(outer: Outer, line: string): DeltaLine {
  if (line.startsWith("+")) {
    return { outer, kind: "add", text: line.slice(1) };
  }
  if (line.startsWith("-")) {
    return { outer, kind: "del", text: line.slice(1) };
  }
  return { outer, kind: "meta", text: line };
}

function bodyLines(text: string | undefined): string[] {
  const lines = (text ?? "").split("\n");
  if (lines.at(-1) === "") {
    lines.pop();
  }
  return lines;
}

/** The changed lines of one file in a revision comparison. */
export function parseFileDelta(file: FileDelta): DeltaLine[] {
  if (file.state === "added" || file.state === "removed") {
    const outer = file.state;
    return bodyLines(file.lines).map((line) => innerLine(outer, line));
  }
  return bodyLines(file.diff).map((line) => {
    const outer = line[0] === "+" ? "added" : line[0] === "-" ? "removed" : "same";
    return innerLine(outer, line.slice(1));
  });
}

export type Side = "left" | "right" | "both";

export type SplitRow<T> = {
  left?: T;
  right?: T;
};

/**
 * Rows for a side-by-side view. A run of left-only lines followed by
 * right-only lines is paired up row by row; a line on both sides gets a row
 * of its own.
 */
export function splitRows<T>(lines: T[], side: (line: T) => Side): SplitRow<T>[] {
  const rows: SplitRow<T>[] = [];
  let left: T[] = [];
  let right: T[] = [];
  const flush = () => {
    for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
      rows.push({ left: left[index], right: right[index] });
    }
    left = [];
    right = [];
  };
  for (const line of lines) {
    const at = side(line);
    if (at === "both") {
      flush();
      rows.push({ left: line, right: line });
    } else if (at === "left") {
      if (right.length) {
        flush();
      }
      left.push(line);
    } else {
      right.push(line);
    }
  }
  flush();
  return rows;
}

export function patchLineSide(line: DiffLine): Side {
  return line.kind === "del" ? "left" : line.kind === "add" ? "right" : "both";
}

export function deltaLineSide(line: DeltaLine): Side {
  return line.outer === "removed" ? "left" : line.outer === "added" ? "right" : "both";
}
