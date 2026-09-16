import { createHash } from "node:crypto";

function sha1(value) {
  return createHash("sha1").update(String(value)).digest("hex");
}

function sha256(value) {
  return createHash("sha256").update(String(value)).digest("hex");
}

export function hash(value) {
  return sha1(value);
}

export function ttl() {
  return 3600;
}
