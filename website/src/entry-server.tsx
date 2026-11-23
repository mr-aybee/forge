import { StrictMode } from "react";
import { renderToString } from "react-dom/server";
import { App } from "./App";
import { NOT_FOUND, ROUTES, SITE, type Route } from "./content/site";

export const routes: Route[] = [...ROUTES, NOT_FOUND];

export function render(path: string) {
  return renderToString(
    <StrictMode>
      <App path={path} />
    </StrictMode>,
  );
}

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");

export function head(route: Route) {
  const url = `${SITE.url}${route.path === "/404" ? "/" : route.path}`;
  const ld = {
    "@context": "https://schema.org",
    "@type": "SoftwareApplication",
    name: SITE.name,
    applicationCategory: "DeveloperApplication",
    operatingSystem: "Linux, Windows",
    offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
    url: SITE.url,
    downloadUrl: SITE.releasesUrl,
    publisher: { "@type": "Organization", name: SITE.company, url: SITE.companyUrl },
  };
  return [
    `<title>${esc(route.title)}</title>`,
    `<meta name="description" content="${esc(route.description)}" />`,
    route.path === "/404" ? `<meta name="robots" content="noindex" />` : `<link rel="canonical" href="${url}" />`,
    `<meta property="og:type" content="website" />`,
    `<meta property="og:site_name" content="${SITE.name}" />`,
    `<meta property="og:title" content="${esc(route.title)}" />`,
    `<meta property="og:description" content="${esc(route.description)}" />`,
    `<meta property="og:url" content="${url}" />`,
    `<meta property="og:image" content="${SITE.url}/og.png" />`,
    `<meta name="twitter:card" content="summary_large_image" />`,
    route.path === "/" ? `<script type="application/ld+json">${JSON.stringify(ld)}</script>` : "",
  ].join("\n    ");
}
