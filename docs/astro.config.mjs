// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";
import sitemap from "@astrojs/sitemap";
import { satteri } from "@astrojs/markdown-satteri";
import { diagrams } from "./src/plugins/diagrams.mjs";

export default defineConfig({
  // Served under forge.orbyntech.com/docs/ next to the website (website/).
  // Links in content are written with the /docs/ prefix.
  site: "https://forge.orbyntech.com",
  base: "/docs",
  // ```diagram blocks become HTML diagrams (src/plugins/diagrams.mjs).
  markdown: { processor: satteri({ mdastPlugins: [diagrams] }) },
  integrations: [
    sitemap(),
    starlight({
      title: "Orbyn Forge",
      logo: { src: "./src/assets/forge-mark.svg" },
      favicon: "/favicon.svg",
      customCss: ["./src/styles/forge.css", "./src/styles/diagrams.css"],
      components: { SiteTitle: "./src/components/SiteTitle.astro", SocialIcons: "./src/components/SiteNav.astro" },
      social: [
        { icon: "github", label: "Releases on GitHub", href: "https://github.com/mr-aybee/forge" },
      ],
      description: "One secure workspace to access, deploy and operate applications across Windows and Linux.",
      lastUpdated: false,
      // Fails the build on a link to a page or heading that does not exist.
      plugins: [starlightLinksValidator()],
      sidebar: [
        { label: "Getting started", items: [{ autogenerate: { directory: "getting-started" } }] },
        { label: "Concepts", items: [{ autogenerate: { directory: "concepts" } }] },
        { label: "Guides", items: [{ autogenerate: { directory: "guides" } }] },
        { label: "Reference", items: [{ autogenerate: { directory: "reference" } }] },
        { label: "Security", items: [{ autogenerate: { directory: "security" } }] },
        { label: "Development", items: [{ autogenerate: { directory: "development" } }] },
      ],
    }),
  ],
});
