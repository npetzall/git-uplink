import { marked } from "marked";
import { GITHUB_BLOB } from "./links";

/**
 * Render a markdown file from this repository for the site: drop its `# Title` (the page supplies one)
 * and point relative links at the file's location on GitHub.
 */
export function renderRepoMarkdown(markdown: string, source: string): string {
  const base = `${GITHUB_BLOB}/${source}`;
  const html = marked.parse(markdown.replace(/^# .*\n/, ""), { async: false }) as string;
  return html.replace(/href="(?![a-z]+:|#|\/)([^"]+)"/g, (_, href: string) => `href="${new URL(href, base)}"`);
}
