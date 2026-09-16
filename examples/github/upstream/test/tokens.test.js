import assert from "node:assert/strict";
import { test } from "node:test";
import { hash, ttl } from "../src/tokens.js";

test("hash returns a hex digest", () => {
  const digest = hash("tokenkit");
  assert.equal(typeof digest, "string");
  assert.match(digest, /^[0-9a-f]+$/);
  assert.ok(digest.length >= 32);
});

test("ttl is a positive number of seconds", () => {
  assert.equal(typeof ttl(), "number");
  assert.ok(ttl() > 0);
});
