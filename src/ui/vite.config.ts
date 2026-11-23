import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// The dev server proxies the API so the browser sees one origin, exactly as
// in production where forge-server serves the built UI itself.
// Ports come from the local registry (a local port registry): 55130 is the local
// forge-server (scripts/dev/local_lab.py), 55132 this dev server.
const target = process.env.FORGE_API ?? "http://127.0.0.1:55130";
const port = Number(process.env.FORGE_UI_PORT ?? 55132);

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server: {
    port,
    strictPort: true,
    proxy: {
      "/api": { target, changeOrigin: false, ws: true },
      "/oauth": { target, changeOrigin: false },
    },
  },
  build: {
    target: "es2022",
    sourcemap: false,
    // Budget is 200 KB *gzipped* initial JS (~100 KB today); this limit is raw size.
    chunkSizeWarningLimit: 400,
  },
});
