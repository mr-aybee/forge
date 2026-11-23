import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Pages are pre-rendered to static HTML by scripts/prerender.mjs after the
// client and SSR builds, so crawlers and link previews see real content.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    // `npm run dev` here; run the docs with `cd ../docs && npm run dev` and
    // links under /docs/ are proxied to it.
    proxy: { "/docs": "http://localhost:4321" },
  },
});
