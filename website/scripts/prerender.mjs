// Writes one static HTML file per route from the client build (dist/) and
// the SSR build (dist-ssr/), plus sitemap.xml and robots.txt.
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const dist = join(root, "dist");
const ssrDir = join(root, "dist-ssr");
const { render, head, routes } = await import(pathToFileURL(join(ssrDir, "entry-server.js")).href);
const SITE_URL = "https://forge.orbyntech.com";

const template = await readFile(join(dist, "index.html"), "utf8");
for (const route of routes) {
  const html = template.replace("<!--head-->", head(route)).replace("<!--app-->", render(route.path));
  const file = route.path === "/404" ? join(dist, "404.html") : join(dist, route.path, "index.html");
  await mkdir(dirname(file), { recursive: true });
  await writeFile(file, html);
  console.log(`prerendered ${route.path}`);
}

const urls = routes
  .filter((r) => r.path !== "/404")
  .map((r) => `  <url><loc>${SITE_URL}${r.path}</loc></url>`)
  .join("\n");
// The docs publish their own sitemap under /docs/.
await writeFile(
  join(dist, "sitemap.xml"),
  `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls}\n</urlset>\n`,
);
await writeFile(
  join(dist, "robots.txt"),
  `User-agent: *\nAllow: /\n\nSitemap: ${SITE_URL}/sitemap.xml\nSitemap: ${SITE_URL}/docs/sitemap-index.xml\n`,
);
await rm(ssrDir, { recursive: true, force: true });
