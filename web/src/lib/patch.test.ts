import { describe, expect, it } from "vitest";
import {
  deltaLineSide,
  parseFileDelta,
  parsePatch,
  patchLineSide,
  splitRows,
  type DiffLine,
} from "./patch";

// A removed line whose content is the mbox signature marker `-- `.
const SIGNATURE_LIKE = "--- ";

const PATCH = `From 1111111111111111111111111111111111111111 Mon Sep 17 00:00:00 2001
From: Asha <asha@example.com>
Date: Thu, 1 Oct 2026 10:00:00 +0200
Subject: [PATCH] Extend the token TTL and
 tidy up

diff --git a/README.md b/README.md is quoted in the message

Signed-off-by: Asha <asha@example.com>
---
 src/tokens.js | 2 +-
 1 file changed, 1 insertion(+), 1 deletion(-)

diff --git a/src/tokens.js b/src/tokens.js
index 1111111..2222222 100644
--- a/src/tokens.js
+++ b/src/tokens.js
@@ -10,4 +10,5 @@ function ttl() {
 function ttl() {
-  return 3600;
+  return 7200;
+diff --git a/fake b/fake

 }
@@ -30 +31 @@
-old
+new
\\ No newline at end of file
diff --git a/src/new.js b/src/new.js
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/src/new.js
@@ -0,0 +1,2 @@
+one
+two
diff --git a/src/gone.js b/src/gone.js
deleted file mode 100644
index 4444444..0000000
--- a/src/gone.js
+++ /dev/null
@@ -1,2 +0,0 @@
-one
${SIGNATURE_LIKE}
diff --git a/old name.js b/new name.js
similarity index 100%
rename from old name.js
rename to new name.js
diff --git a/run.sh b/run.sh
old mode 100644
new mode 100755
diff --git a/logo.png b/logo.png
new file mode 100644
index 0000000..5555555
GIT binary patch
literal 4
Lc$\{NkU|;|M00aO5

literal 0
HcmV?d00001

--
2.50.0
`;

describe("parsePatch", () => {
  const patch = parsePatch(PATCH);

  it("reads the commit header", () => {
    expect(patch.from).toBe("Asha <asha@example.com>");
    expect(patch.date).toBe("Thu, 1 Oct 2026 10:00:00 +0200");
    expect(patch.subject).toBe("Extend the token TTL and tidy up");
    expect(patch.message).toBe(
      "diff --git a/README.md b/README.md is quoted in the message\n\nSigned-off-by: Asha <asha@example.com>",
    );
  });

  it("lists each file on its own", () => {
    expect(patch.files.map((file) => [file.path, file.status])).toEqual([
      ["src/tokens.js", "modified"],
      ["src/new.js", "added"],
      ["src/gone.js", "deleted"],
      ["new name.js", "renamed"],
      ["run.sh", "mode"],
      ["logo.png", "added"],
    ]);
  });

  it("numbers the lines of a modified file and keeps look-alike content", () => {
    const file = patch.files[0];
    expect(file.additions).toBe(3);
    expect(file.deletions).toBe(2);
    expect(file.hunks).toHaveLength(2);
    expect(file.hunks[0].lines).toEqual<DiffLine[]>([
      { kind: "ctx", text: "function ttl() {", oldNo: 10, newNo: 10 },
      { kind: "del", text: "  return 3600;", oldNo: 11 },
      { kind: "add", text: "  return 7200;", newNo: 11 },
      { kind: "add", text: "diff --git a/fake b/fake", newNo: 12 },
      { kind: "ctx", text: "", oldNo: 12, newNo: 13 },
      { kind: "ctx", text: "}", oldNo: 13, newNo: 14 },
    ]);
    expect(file.hunks[1].lines).toEqual<DiffLine[]>([
      { kind: "del", text: "old", oldNo: 30 },
      { kind: "add", text: "new", newNo: 31 },
      { kind: "meta", text: "\\ No newline at end of file" },
    ]);
  });

  it("gives an added file only new line numbers", () => {
    const file = patch.files[1];
    expect(file.oldPath).toBeUndefined();
    expect(file.newPath).toBe("src/new.js");
    expect(file.hunks[0].lines).toEqual<DiffLine[]>([
      { kind: "add", text: "one", newNo: 1 },
      { kind: "add", text: "two", newNo: 2 },
    ]);
  });

  it("gives a deleted file only old line numbers, signature look-alike included", () => {
    const file = patch.files[2];
    expect(file.newPath).toBeUndefined();
    expect(file.oldPath).toBe("src/gone.js");
    expect(file.hunks[0].lines).toEqual<DiffLine[]>([
      { kind: "del", text: "one", oldNo: 1 },
      { kind: "del", text: "-- ", oldNo: 2 },
    ]);
  });

  it("keeps both names of a rename", () => {
    const file = patch.files[3];
    expect(file.oldPath).toBe("old name.js");
    expect(file.newPath).toBe("new name.js");
    expect(file.hunks).toEqual([]);
  });

  it("marks a binary file and drops its data and the mbox signature", () => {
    const file = patch.files[5];
    expect(file.binary).toBe(true);
    expect(file.hunks).toEqual([]);
    expect(file.meta).toEqual(["new file mode 100644"]);
  });

  it("parses a bare diff", () => {
    const bare = parsePatch("diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-a\n+b\n");
    expect(bare.subject).toBeUndefined();
    expect(bare.files).toHaveLength(1);
    expect(bare.files[0].hunks[0].lines).toHaveLength(2);
  });

  it("returns text that is not a patch as the message", () => {
    expect(parsePatch("not a patch\n")).toEqual({ message: "not a patch", files: [] });
    expect(parsePatch("")).toEqual({ message: "", files: [] });
  });
});

describe("parseFileDelta", () => {
  const base = { path: "a.rs", header: "diff --git a/a.rs b/a.rs" };

  it("reads both markers of a changed file", () => {
    const lines = parseFileDelta({
      ...base,
      state: "changed",
      diff: "+deleted file mode 100644\n -a\n-+b\n++c\n",
    });
    expect(lines).toEqual([
      { outer: "added", kind: "meta", text: "deleted file mode 100644" },
      { outer: "same", kind: "del", text: "a" },
      { outer: "removed", kind: "add", text: "b" },
      { outer: "added", kind: "add", text: "c" },
    ]);
  });

  it("puts every line of an added or removed file on one side", () => {
    const added = parseFileDelta({ ...base, state: "added", lines: "new file mode 100644\n+a\n" });
    expect(added).toEqual([
      { outer: "added", kind: "meta", text: "new file mode 100644" },
      { outer: "added", kind: "add", text: "a" },
    ]);
    const removed = parseFileDelta({ ...base, state: "removed", lines: "-a\n" });
    expect(removed).toEqual([{ outer: "removed", kind: "del", text: "a" }]);
    expect(splitRows(added, deltaLineSide).every((row) => !row.left)).toBe(true);
    expect(splitRows(removed, deltaLineSide).every((row) => !row.right)).toBe(true);
  });

  it("has no lines for an unchanged file", () => {
    expect(parseFileDelta({ ...base, state: "unchanged" })).toEqual([]);
  });
});

describe("splitRows", () => {
  const line = (kind: DiffLine["kind"], text: string): DiffLine => ({ kind, text });

  it("pairs removed lines with the added lines that follow", () => {
    const rows = splitRows(
      [
        line("ctx", "a"),
        line("del", "b"),
        line("del", "c"),
        line("add", "B"),
        line("ctx", "d"),
        line("add", "e"),
        line("del", "f"),
      ],
      patchLineSide,
    );
    expect(rows.map((row) => [row.left?.text, row.right?.text])).toEqual([
      ["a", "a"],
      ["b", "B"],
      ["c", undefined],
      ["d", "d"],
      [undefined, "e"],
      ["f", undefined],
    ]);
  });
});
