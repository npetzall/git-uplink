import { rebuild, type SimFileMap, type SimPatch, type SimState } from "../simulator";

export const README = "tokenkit\n";

export function tree(tokens: string, extra: SimFileMap = {}): SimFileMap {
  return { "src/tokens.js": tokens, "README.md": README, ...extra };
}

export function withVendor(tokens: string): SimFileMap {
  return tree(
    `${tokens.trimEnd()}\n\nexport function vendorTelemetry() {\n  return "internal-only";\n}\n`,
  );
}

export const TOKENS_SHA1 = `export function hash(value) {
  return sha1(value);
}

export function ttl() {
  return 3600;
}
`;

export const TOKENS_SHA256 = `export function hash(value) {
  return sha256(value);
}

export function ttl() {
  return 3600;
}
`;

export const TOKENS_SHA256_LOGS = `export function hash(value) {
  console.log("hash", value);
  return sha256(value);
}

export function ttl() {
  return 3600;
}
`;

export const TOKENS_SALTED = `export function hash(value) {
  return saltedSha256(value);
}

export function ttl() {
  return 3600;
}
`;

export const TOKENS_SALTED_LOGS = `export function hash(value) {
  console.log("hash", value);
  return saltedSha256(value);
}

export function ttl() {
  return 3600;
}
`;

export const TOKENS_DIGEST = `export function hash(value) {
  const digest = saltedSha256(value);
  return digest;
}

export function ttl() {
  return 3600;
}
`;

export const TOKENS_CONFLICT = `export function hash(value) {
<<<<<<< HEAD (upstream/main)
  const digest = saltedSha256(value);
  return digest;
=======
  console.log("hash", value);
  return saltedSha256(value);
>>>>>>> upl_logs
}

export function ttl() {
  return 3600;
}
`;

export const TOKENS_RESOLVED = `export function hash(value) {
  const digest = saltedSha256(value);
  console.log("hash", value);
  return digest;
}

export function ttl() {
  return 3600;
}
`;

export const TOKENS_TTL_7200 = `export function hash(value) {
  return sha1(value);
}

export function ttl() {
  return 7200;
}
`;

export const TOKENS_SHA256_TTL_7200 = `export function hash(value) {
  return sha256(value);
}

export function ttl() {
  return 7200;
}
`;

export const TOKENS_TTL_1800 = `export function hash(value) {
  return sha1(value);
}

export function ttl() {
  return 1800;
}
`;

export const TOKENS_TTL_CONFLICT = `export function hash(value) {
  return sha1(value);
}

export function ttl() {
<<<<<<< HEAD (upstream/main)
  return 1800;
=======
  return 7200;
>>>>>>> upl_ben
}
`;

export const TOKENS_CAM = `export function hash(value) {
  return sha256(value);
}

export function ttl() {
  return 7200;
}

export function describeToken(value) {
  return \`ttl=\${ttl()} hash=\${hash(value)}\`;
}
`;

export const TOKENS_TELEMETRY = `export function hash(value) {
  companyTelemetry();
  return sha1(value);
}

export function ttl() {
  return 3600;
}
`;

export const TOOLING_FILES: SimFileMap = {
  ".github/pull_request_template.md": "<!-- installed by git uplink init --forge ghec -->\n",
};

export const TOOLING_PATCH: SimPatch = {
  id: "upl_tooling",
  title: "Uplink tooling",
  queue: "internal",
  status: "queued",
  dependsOn: [],
  files: TOOLING_FILES,
};

export function emptyMirror(): SimState {
  return {
    stepId: "start",
    upstream: tree(TOKENS_SHA1),
    company: tree(TOKENS_SHA1),
    contrib: [],
    patches: [],
    log: ["Company mirror created from public upstream."],
  };
}

export function exampleSeed(): SimState {
  return rebuild({
    ...emptyMirror(),
    patches: [TOOLING_PATCH],
    log: [
      "Company mirror created from public upstream.",
      "Init installed the internal-only tooling patch.",
    ],
  });
}
