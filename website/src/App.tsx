import type { ComponentType } from "react";
import { Layout } from "./components/Layout";
import { Home } from "./pages/Home";
import { Features } from "./pages/Features";
import { UseCases } from "./pages/UseCases";
import { Security } from "./pages/Security";
import { Download } from "./pages/Download";
import { License } from "./pages/License";
import { NotFound } from "./pages/NotFound";

// Each path is pre-rendered to its own HTML file; links are plain <a> tags,
// so there is no client router. React hydrates for the interactive bits.
export const PAGES: Record<string, ComponentType> = {
  "/": Home,
  "/features/": Features,
  "/use-cases/": UseCases,
  "/security/": Security,
  "/download/": Download,
  "/license/": License,
  "/404": NotFound,
};

export function normalize(path: string) {
  if (path === "/" || path === "/404") return path;
  return path.endsWith("/") ? path : `${path}/`;
}

export function App({ path }: { path: string }) {
  const Page = PAGES[normalize(path)] ?? NotFound;
  return (
    <Layout path={path}>
      <Page />
    </Layout>
  );
}
