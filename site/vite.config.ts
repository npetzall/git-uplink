import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const repoRoot = fileURLToPath(new URL("..", import.meta.url));

function pagesBase() {
  const raw = process.env.PAGES_BASE ?? "/";
  return raw.endsWith("/") ? raw : `${raw}/`;
}

function pagesSpaFallback() {
  return {
    name: "pages-spa-fallback",
    closeBundle() {
      const dist = path.resolve(fileURLToPath(new URL("./dist", import.meta.url)));
      const index = path.join(dist, "index.html");
      if (!fs.existsSync(index)) return;
      fs.copyFileSync(index, path.join(dist, "404.html"));
      fs.writeFileSync(path.join(dist, ".nojekyll"), "");
    },
  };
}

export default defineConfig({
  base: pagesBase(),
  plugins: [react(), tailwindcss(), pagesSpaFallback()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    fs: {
      allow: [repoRoot],
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    assetsDir: "assets",
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
